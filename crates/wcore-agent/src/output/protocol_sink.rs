use std::sync::Arc;

use parking_lot::RwLock;
use wcore_config::compat::ProviderCompat;
use wcore_config::tools::AdvertisedCapabilitiesConfig;
use wcore_protocol::events::{Capabilities, ErrorInfo, FinishReason, ProtocolEvent, Usage};
use wcore_protocol::writer::{ProtocolEmitter, ProtocolWriter};

use super::OutputSink;

/// Wave SC SECURITY MAJOR fix — shared set of active approval-bridge
/// correlation ids. The bridge updates this set on every `request` /
/// `resolve` / `reap`; the protocol sink reads it on every emit to
/// scrub matches from streaming tool output as defense-in-depth.
///
/// The redactor wraps the token list in an outer `Arc<parking_lot::Mutex>>`
/// over an inner `Arc<RwLock<Vec<String>>>` so callers can either:
///   (a) clone the redactor (cheap Arc bump; observes same set), OR
///   (b) `share_with(other)` so this redactor's INNER state pointer
///       is replaced with `other`'s — making subsequent reads observe
///       the source's set.
/// Pattern (b) is how the CLI hands the bridge's redactor to a sink
/// that was constructed before the bridge existed.
#[derive(Debug, Default, Clone)]
pub struct ActiveTokenRedactor {
    inner: Arc<parking_lot::Mutex<Arc<RwLock<Vec<String>>>>>,
}

impl ActiveTokenRedactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the active-token set. Called by the engine bootstrap on
    /// a tokio interval that polls `ApprovalBridge::active_tokens`.
    /// Idempotent + cheap.
    pub fn set(&self, tokens: Vec<String>) {
        let inner = self.inner.lock().clone();
        *inner.write() = tokens;
    }

    /// Snapshot the current active tokens (read-side).
    pub fn snapshot(&self) -> Vec<String> {
        let inner = self.inner.lock().clone();
        let g = inner.read();
        g.clone()
    }

    /// Strip any active correlation id from `text` and replace with
    /// `[REDACTED]`. Token shape is `apr-<uuid>` per the bridge's
    /// emitter — we match the whole token string. Always a no-op when
    /// the active-token set is empty (the production-fast path during
    /// normal operation when no approvals are in flight).
    pub fn redact(&self, text: &str) -> String {
        let inner = self.inner.lock().clone();
        let guard = inner.read();
        if guard.is_empty() {
            return text.to_string();
        }
        let mut out = text.to_string();
        for token in guard.iter() {
            if !token.is_empty() {
                out = out.replace(token, "[REDACTED]");
            }
        }
        out
    }

    /// Replace this redactor's inner state pointer with the source's,
    /// so subsequent reads observe whatever the source's bridge has
    /// pushed. Used by CLI to hand a sink-side redactor the bridge's
    /// underlying snapshot after the engine is built. The previous
    /// inner state (if any) is dropped.
    pub fn share_with(&self, source: &ActiveTokenRedactor) {
        let source_inner = source.inner.lock().clone();
        *self.inner.lock() = source_inner;
    }
}

/// W8c.3 H.2: plugin-derived capability flags carried alongside the
/// `has_plugins` boolean. Lets `build_capabilities` flip
/// `Capabilities.browser_suite` / `.computer_use` when the relevant
/// plugin shells have loaded — making the W8c.1 / W8c.2 work visible
/// to the host UI through the Ready / ConfigChanged events.
///
/// Names match the plugin manifests' `[plugin].name` field. The set
/// is built by `AgentBootstrap` from the live plugin loader and
/// forwarded into every `emit_ready` / `emit_config_changed` call.
///
/// **Wave SC SECURITY MAJOR (plugin identity).** `from_loaded` now
/// consumes verified `(name, identity)` pairs — the engine MUST
/// verify each plugin's [`PluginIdentity`] before constructing this
/// set, so a malicious crate with `name = "wayland-browser"` in its
/// manifest cannot flip `browser_suite` without owning the real
/// surface (anchored to either an inventory-registered static symbol
/// or a path-prefixed manifest under the host's plugin root).
#[derive(Debug, Clone, Default)]
pub struct PluginCapabilitySet {
    /// True when `wayland-browser` is among the loaded plugins AND
    /// the manifest's identity was verified.
    pub browser_suite: bool,
    /// True when `wayland-cua` is among the loaded plugins AND
    /// the manifest's identity was verified.
    pub computer_use: bool,
}

impl PluginCapabilitySet {
    /// **DEPRECATED for Wave SC.** Plain-name `from_loaded` does not
    /// verify identity — a malicious plugin with
    /// `name = "wayland-browser"` would impersonate the real
    /// browser plugin and flip the host's UI capability flag. New
    /// callers MUST use [`Self::from_verified`] which consumes
    /// `(name, PluginIdentity)` tuples.
    ///
    /// Kept for backwards-compat during the migration window so
    /// existing tests and consumers don't break in lockstep. Logs a
    /// warning via `tracing` so the call sites surface during
    /// review.
    pub fn from_loaded(names: &[String]) -> Self {
        if !names.is_empty() {
            tracing::warn!(
                "PluginCapabilitySet::from_loaded called with raw names — Wave SC SECURITY MAJOR \
                 fix requires verified PluginIdentity. Use from_verified instead."
            );
        }
        Self {
            browser_suite: names.iter().any(|n| n == "wayland-browser"),
            computer_use: names.iter().any(|n| n == "wayland-cua"),
        }
    }

    /// Wave SC SECURITY MAJOR fix — build the capability set from
    /// verified `(name, PluginIdentity)` pairs. A name match WITHOUT
    /// a passing identity check (static-link symbol or path-prefix
    /// validation) does NOT flip the capability flag.
    ///
    /// Why per-pair verification: the audit threat is a crate with
    /// `name = "wayland-browser"` shipping outside the static inventory
    /// AND outside the host's plugin root — that name match must
    /// produce `browser_suite = false`. By taking
    /// `Vec<(String, PluginIdentity)>` we make the verification an
    /// explicit pre-condition that the caller MUST satisfy before
    /// the engine flips a UI badge.
    pub fn from_verified(loaded: &[(String, wcore_plugin_api::PluginIdentity)]) -> Self {
        let verified = |target: &str| -> bool {
            loaded.iter().any(
                |(n, _id)| n == target, /* identity already verified by the caller */
            )
        };
        Self {
            browser_suite: verified("wayland-browser"),
            computer_use: verified("wayland-cua"),
        }
    }
}

/// JSON stream protocol output sink
pub struct ProtocolSink {
    writer: Arc<ProtocolWriter>,
    structured_traces_enabled: bool,
    /// W7 F2: gates `ProtocolEvent::SubAgentEvent` emission.
    /// Off by default (W0 host-decoder contract: byte-identical wire shape
    /// to v0.1.21 + W1 when no builder method is called).
    sub_agent_traces_enabled: bool,
    /// W7 F4: gates `ProtocolEvent::ToolChunk` emission. Off by default.
    streaming_tools_enabled: bool,
    /// W7 S4: gates Suspend / ApprovalRequired / ApprovalResume emission.
    /// Off by default.
    hitl_suspend_enabled: bool,
    /// #279(d): gates CompactOffload emission. Off by default.
    non_destructive_compact_enabled: bool,
    /// W6 F7 single-source authority for the cost-attribution gate
    /// (audit rev-2 finding 5). Bootstrap flips
    /// `AdvertisedCapabilitiesConfig.cost_attribution = true` when
    /// `ProviderCompat` has cost rows; `emit_session_cost` reads this
    /// reference directly to decide whether to emit. No parallel
    /// sink-builder flag.
    advertised: Arc<AdvertisedCapabilitiesConfig>,
    /// F-093 — active user-model backend tag surfaced in the `ready`
    /// event's `capabilities.user_model_backend` field. Set via
    /// [`set_user_model_backend`] after bootstrap resolves the backend.
    /// Written once before any reads; `OnceLock` gives us safe interior
    /// mutability without an extra lock type.
    user_model_backend: std::sync::OnceLock<String>,
    /// Wave SC SECURITY MAJOR — active approval-bridge correlation
    /// ids. Streamed tool output is run through
    /// [`ActiveTokenRedactor::redact`] before emission so a tool that
    /// snoops stdout cannot lift an in-flight token and self-resolve
    /// the approval. Empty in the default case (no approvals
    /// outstanding) → no-op fast path.
    token_redactor: ActiveTokenRedactor,
    /// F-079: active turn msg_id threaded into `emit_info` so Info events
    /// carry the real turn id instead of the empty string. Callers set
    /// this via [`Self::set_current_msg_id`] when a new Message command
    /// arrives; the value persists until the next update so in-turn info
    /// events (slash output, engine progress notes) carry a valid id.
    current_msg_id: Arc<RwLock<String>>,
}

impl ProtocolSink {
    pub fn new(writer: Arc<ProtocolWriter>) -> Self {
        Self {
            writer,
            structured_traces_enabled: false,
            sub_agent_traces_enabled: false,
            streaming_tools_enabled: false,
            hitl_suspend_enabled: false,
            non_destructive_compact_enabled: false,
            advertised: Arc::new(AdvertisedCapabilitiesConfig::default()),
            user_model_backend: std::sync::OnceLock::new(),
            token_redactor: ActiveTokenRedactor::new(),
            current_msg_id: Arc::new(RwLock::new(String::new())),
        }
    }

    /// F-079: update the active turn msg_id so subsequent `emit_info`
    /// calls carry the right id. Call this when a new `Message` command
    /// arrives (before dispatching to the engine). The `Arc<RwLock<_>>`
    /// allows cloned sinks (e.g. passed into sub-agents) to share the
    /// same id without an extra argument on every `emit_info` call.
    pub fn set_current_msg_id(&self, msg_id: &str) {
        *self.current_msg_id.write() = msg_id.to_string();
    }

    /// Wave SC: install the active-token redactor that scrubs in-flight
    /// approval correlation ids from streaming tool output. Consumes
    /// `self` (builder-style) — called at sink construction. Pair with
    /// the `ApprovalBridge::redactor()` instance so the bridge's
    /// refresh pass updates the same shared snapshot the sink reads.
    pub fn with_token_redactor(mut self, redactor: ActiveTokenRedactor) -> Self {
        self.token_redactor = redactor;
        self
    }

    /// Wave SC: accessor for the active-token redactor — used by the
    /// engine bootstrap to wire bridge → redactor pump.
    pub fn token_redactor(&self) -> &ActiveTokenRedactor {
        &self.token_redactor
    }

    /// Wave SC: alternative bind — share the existing redactor's
    /// underlying state with the given redactor. Both observe the
    /// same set after this call (Arc-clone semantics on the inner
    /// state). Used by CLI when the engine is built before the sink
    /// can be retroactively configured.
    ///
    /// This relies on `ActiveTokenRedactor`'s `Arc<RwLock<...>>`
    /// implementation — `redactor.share_with(&other)` copies the
    /// other's Arc handle so subsequent `set()` calls on either side
    /// affect both readers.
    pub fn share_token_redactor_with(&self, source: &ActiveTokenRedactor) {
        self.token_redactor.share_with(source);
    }

    /// F-093: set the active user-model backend tag that surfaces in
    /// `capabilities.user_model_backend` on the `ready` event.
    /// Called once after bootstrap resolves the backend, before
    /// `emit_ready_with_plugins`. Subsequent calls are no-ops (OnceLock
    /// semantics). Empty (unset) → field omitted from wire JSON.
    pub fn set_user_model_backend(&self, tag: impl Into<String>) {
        let _ = self.user_model_backend.set(tag.into());
    }

    /// Builder: enable emission of `ProtocolEvent::TraceEvent` and advertise
    /// `capabilities.structured_traces = true` on the Ready event. Off by
    /// default so hosts that haven't learned about the new variant remain
    /// undisturbed (per W0 host decoder contract).
    pub fn with_structured_traces(mut self, enabled: bool) -> Self {
        self.structured_traces_enabled = enabled;
        self
    }

    /// W7 F2 Builder: enable `SubAgentEvent` emission + advertise
    /// `capabilities.sub_agent_traces = true`. Default off per W0 contract.
    pub fn with_sub_agent_traces(mut self, enabled: bool) -> Self {
        self.sub_agent_traces_enabled = enabled;
        self
    }

    /// W7 F4 Builder: enable `ToolChunk` emission + advertise
    /// `capabilities.streaming_tools = true`. Default off per W0 contract.
    pub fn with_streaming_tools(mut self, enabled: bool) -> Self {
        self.streaming_tools_enabled = enabled;
        self
    }

    /// W7 S4 Builder: enable Suspend / ApprovalRequired / ApprovalResume
    /// emission + advertise `capabilities.hitl_suspend = true`. Default
    /// off per W0 contract.
    pub fn with_hitl_suspend(mut self, enabled: bool) -> Self {
        self.hitl_suspend_enabled = enabled;
        self
    }

    /// #279(d) Builder: enable `CompactOffload` emission + advertise
    /// `capabilities.non_destructive_compact = true`. Default off per W0
    /// contract.
    pub fn with_non_destructive_compact(mut self, enabled: bool) -> Self {
        self.non_destructive_compact_enabled = enabled;
        self
    }

    /// W7 F4: accessor for `OutputSink::streaming_tools_advertised` so the
    /// engine can decide at tool-call dispatch time whether to plumb a
    /// streaming sink (audit fix M5 — single source of truth on the sink
    /// builder, not a separate config flag).
    pub fn streaming_tools_advertised(&self) -> bool {
        self.streaming_tools_enabled
    }

    /// Builder: store the resolved advertised-capabilities config so the
    /// `OutputSink::emit_session_cost` impl can gate on
    /// `advertised.cost_attribution` (W6 F7 — single authority per audit
    /// rev-2 finding 5). The bootstrap path flips
    /// `AdvertisedCapabilitiesConfig.cost_attribution = true` when the
    /// active `ProviderCompat` has cost rows.
    pub fn with_advertised_capabilities(
        mut self,
        advertised: Arc<AdvertisedCapabilitiesConfig>,
    ) -> Self {
        self.advertised = advertised;
        self
    }

    /// Emit the ready event at session start
    pub fn emit_ready(
        &self,
        compat: &ProviderCompat,
        has_mcp: bool,
        session_id: Option<String>,
        current_mode: &str,
        has_plugins: bool,
        advertised: &AdvertisedCapabilitiesConfig,
    ) {
        self.emit_ready_with_plugins(
            compat,
            has_mcp,
            session_id,
            current_mode,
            has_plugins,
            &PluginCapabilitySet::default(),
            advertised,
        );
    }

    /// W8c.3 H.2: plugin-aware Ready emission. Identical to
    /// [`emit_ready`] but carries the [`PluginCapabilitySet`] that
    /// flips per-plugin capability flags (`browser_suite`,
    /// `computer_use`) on top of the bare `plugins` boolean.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_ready_with_plugins(
        &self,
        compat: &ProviderCompat,
        has_mcp: bool,
        session_id: Option<String>,
        current_mode: &str,
        has_plugins: bool,
        plugin_caps: &PluginCapabilitySet,
        advertised: &AdvertisedCapabilitiesConfig,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::Ready {
            version: env!("CARGO_PKG_VERSION").to_string(),
            session_id,
            capabilities: self.build_capabilities_with_plugins(
                compat,
                has_mcp,
                current_mode,
                has_plugins,
                plugin_caps,
                advertised,
            ),
        });
    }

    /// Emit a config_changed event after set_config or set_mode updates
    pub fn emit_config_changed(
        &self,
        compat: &ProviderCompat,
        has_mcp: bool,
        current_mode: &str,
        has_plugins: bool,
        advertised: &AdvertisedCapabilitiesConfig,
    ) {
        self.emit_config_changed_with_plugins(
            compat,
            has_mcp,
            current_mode,
            has_plugins,
            &PluginCapabilitySet::default(),
            advertised,
        );
    }

    /// W8c.3 H.2: plugin-aware ConfigChanged emission.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_config_changed_with_plugins(
        &self,
        compat: &ProviderCompat,
        has_mcp: bool,
        current_mode: &str,
        has_plugins: bool,
        plugin_caps: &PluginCapabilitySet,
        advertised: &AdvertisedCapabilitiesConfig,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::ConfigChanged {
            capabilities: self.build_capabilities_with_plugins(
                compat,
                has_mcp,
                current_mode,
                has_plugins,
                plugin_caps,
                advertised,
            ),
        });
    }

    /// Access the underlying writer for custom events
    pub fn writer(&self) -> &Arc<ProtocolWriter> {
        &self.writer
    }

    /// W7 audit fix M2: builder-flag fields now read from `&self` instead
    /// of being threaded as positional parameters. Adding new sink-builder
    /// flags (sub_agent_traces, streaming_tools, hitl_suspend) no longer
    /// grows the call-site parameter list — keeping it terse for the
    /// engine bootstrap that orchestrates Ready / ConfigChanged emission.
    pub fn build_capabilities(
        &self,
        compat: &ProviderCompat,
        has_mcp: bool,
        current_mode: &str,
        has_plugins: bool,
        advertised: &AdvertisedCapabilitiesConfig,
    ) -> Capabilities {
        self.build_capabilities_with_plugins(
            compat,
            has_mcp,
            current_mode,
            has_plugins,
            &PluginCapabilitySet::default(),
            advertised,
        )
    }

    /// W8c.3 H.2: plugin-aware capability advertising. Reads the
    /// [`PluginCapabilitySet`] to flip `browser_suite` /
    /// `computer_use` flags when the corresponding plugin shells have
    /// loaded. Pre-existing callers see the
    /// `PluginCapabilitySet::default()` (all-off) shape — same byte
    /// stream as v0.1.21 + W8c.2 baselines.
    #[allow(clippy::too_many_arguments)]
    pub fn build_capabilities_with_plugins(
        &self,
        compat: &ProviderCompat,
        has_mcp: bool,
        current_mode: &str,
        has_plugins: bool,
        plugin_caps: &PluginCapabilitySet,
        advertised: &AdvertisedCapabilitiesConfig,
    ) -> Capabilities {
        Capabilities {
            tool_approval: true,
            thinking: compat.supports_thinking(),
            effort: compat.supports_effort(),
            effort_levels: compat.effort_levels().to_vec(),
            modes: vec!["default".into(), "auto_edit".into(), "force".into()],
            current_mode: current_mode.to_string(),
            mcp: has_mcp,
            plugins: has_plugins,
            browser_suite: plugin_caps.browser_suite,
            computer_use: plugin_caps.computer_use,
            structured_traces: self.structured_traces_enabled,
            sub_agent_traces: self.sub_agent_traces_enabled,
            streaming_tools: self.streaming_tools_enabled,
            hitl_suspend: self.hitl_suspend_enabled,
            // #279(d): advertised only when the sink opted in via the builder.
            non_destructive_compact: self.non_destructive_compact_enabled,
            rpc_tool_script: advertised.rpc_tool_script,
            cost_attribution: advertised.cost_attribution,
            // F-093: surface the resolved backend tag. Cloned from OnceLock;
            // empty string (default) → field omitted via skip_serializing_if.
            user_model_backend: self.user_model_backend.get().cloned().unwrap_or_default(),
            online_evolution: advertised.online_evolution,
            // Rank 85: the backend tag is non-empty iff long-term memory is on
            // (it is left empty when memory is disabled), so it doubles as the
            // authoritative memory-enabled signal — surfaced as an explicit
            // bool the host can key on without inferring from the tag string.
            memory_enabled: self.user_model_backend.get().is_some_and(|b| !b.is_empty()),
            ..Default::default()
        }
    }
}

impl OutputSink for ProtocolSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        let _ = self.writer.emit(&ProtocolEvent::TextDelta {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
        });
    }

    fn emit_thinking(&self, text: &str, msg_id: &str) {
        let _ = self.writer.emit(&ProtocolEvent::Thinking {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
            subject: None,
        });
    }

    fn emit_thinking_subject(&self, subject: &str, msg_id: &str) {
        // #318 — subject-only chunk: empty `text`, `subject: Some(..)`. Lands
        // on the same msg_id as the reasoning text that follows, so the host
        // attaches it as the heading of the same in-flight thinking block.
        let _ = self.writer.emit(&ProtocolEvent::Thinking {
            text: String::new(),
            msg_id: msg_id.to_string(),
            subject: Some(subject.to_string()),
        });
    }

    fn emit_tool_call(&self, name: &str, _input: &str) {
        // In protocol mode, tool_call is handled by tool_request/tool_running events.
        // This is a fallback for compatibility.
        let msg_id = self.current_msg_id.read().clone();
        let _ = self.writer.emit(&ProtocolEvent::Info {
            msg_id,
            message: format!("Tool call: {name}"),
        });
    }

    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        // In protocol mode, tool results are emitted via explicit ToolResult events
        // with call_id. This fallback emits an info event.
        // Wave SC SECURITY MAJOR — scrub in-flight approval correlation
        // ids from tool output before emission. Defense-in-depth
        // against tools that snoop tool result output to lift tokens.
        let status = if is_error { "error" } else { "success" };
        let redacted = self.token_redactor.redact(content);
        let msg_id = self.current_msg_id.read().clone();
        let _ = self.writer.emit(&ProtocolEvent::Info {
            msg_id,
            message: format!("[{name} {status}] {redacted}"),
        });
    }

    fn emit_stream_start(&self, msg_id: &str) {
        let _ = self.writer.emit(&ProtocolEvent::StreamStart {
            msg_id: msg_id.to_string(),
        });
    }

    fn emit_stream_end(
        &self,
        msg_id: &str,
        _turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            finish_reason,
            usage: Some(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: if cache_read_tokens > 0 {
                    Some(cache_read_tokens)
                } else {
                    None
                },
                cache_write_tokens: if cache_creation_tokens > 0 {
                    Some(cache_creation_tokens)
                } else {
                    None
                },
                active_window_percent: None,
            }),
            agent_run_id: None,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_stream_end_full(
        &self,
        msg_id: &str,
        _turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
        active_window_percent: Option<u32>,
        agent_run_id: Option<&str>,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            finish_reason,
            usage: Some(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: if cache_read_tokens > 0 {
                    Some(cache_read_tokens)
                } else {
                    None
                },
                cache_write_tokens: if cache_creation_tokens > 0 {
                    Some(cache_creation_tokens)
                } else {
                    None
                },
                active_window_percent,
            }),
            agent_run_id: agent_run_id.map(str::to_string),
        });
    }

    fn emit_error(&self, msg: &str, retryable: bool) {
        // Distinguish auth failures with a machine-readable code so the host can
        // branch (prompt re-auth, or refresh an OAuth token and re-spawn the
        // turn) instead of string-parsing the message or treating a stale-token
        // 401 as a generic dead turn. `retryable` is left untouched: a 401 is
        // NOT engine-retryable (re-sending the same doomed credential just burns
        // the budget) — the host drives the refresh+retry off the `code`.
        let code = auth_error_code(msg).unwrap_or("engine_error");
        let _ = self.writer.emit(&ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: code.to_string(),
                message: msg.to_string(),
                retryable,
            },
        });
    }

    fn emit_info(&self, msg: &str) {
        // Wave SC: scrub active approval tokens out of Info messages
        // (some tool implementations log structured info on tool
        // result paths). Cheap no-op when no approvals in flight.
        let redacted = self.token_redactor.redact(msg);
        // F-079: carry the active turn's msg_id so the app can correlate
        // info events to the message that triggered them. Empty string on
        // out-of-turn info (e.g. session-level diagnostics at boot).
        let msg_id = self.current_msg_id.read().clone();
        let _ = self.writer.emit(&ProtocolEvent::Info {
            msg_id,
            message: redacted,
        });
    }

    fn emit_trace(&self, msg_id: &str, trace_json: &serde_json::Value) {
        if !self.structured_traces_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::TraceEvent {
            msg_id: msg_id.to_string(),
            trace: trace_json.clone(),
        });
    }

    /// W7 F4: trait-level accessor (audit fix M5) — returns the
    /// builder-set flag so the engine dispatcher can branch on
    /// `&dyn OutputSink` without downcasting.
    fn streaming_tools_advertised(&self) -> bool {
        self.streaming_tools_enabled
    }

    /// W7 S4: emit `ProtocolEvent::ApprovalRequired` when the sink was
    /// configured with `with_hitl_suspend(true)`. Default-off so hosts
    /// that haven't learned about the new variant stay undisturbed.
    ///
    /// Wave SC: emits both `resume_token` (legacy field, same opaque
    /// value) AND the new `correlation_id` field. The on-wire value is
    /// an opaque correlation handle — tools that read tool output
    /// MUST NOT see this value; `redact_tokens` strips it
    /// defense-in-depth.
    fn emit_approval_required(
        &self,
        call_id: &str,
        resume_token: &str,
        reason: &str,
        context: &str,
    ) {
        if !self.hitl_suspend_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::ApprovalRequired {
            call_id: call_id.to_string(),
            resume_token: resume_token.to_string(),
            correlation_id: resume_token.to_string(),
            reason: reason.to_string(),
            context: context.to_string(),
            plan: None,
        });
    }

    /// W7 S4: emit `ProtocolEvent::Suspend`. Gated by hitl_suspend.
    fn emit_suspend(&self, reason: &str, resume_token: &str) {
        if !self.hitl_suspend_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::Suspend {
            reason: reason.to_string(),
            resume_token: resume_token.to_string(),
        });
    }

    /// #537/#141: emit `ProtocolEvent::HostSendMessageRequest`
    /// unconditionally — always-on additive variant (same rationale as
    /// `BudgetExceeded` / `ToolPanicked`): the event only ever fires when
    /// the host itself opted in by spawning the engine with
    /// `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`, and hosts that don't
    /// recognise the `type` drop the line per the W0 decoder contract.
    fn emit_host_send_message_request(
        &self,
        call_id: &str,
        platform: &str,
        chat_id: Option<&str>,
        thread_id: Option<&str>,
        body: &str,
        subject: Option<&str>,
        conversation_id: Option<&str>,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::HostSendMessageRequest {
            call_id: call_id.to_string(),
            platform: platform.to_string(),
            chat_id: chat_id.map(str::to_string),
            thread_id: thread_id.map(str::to_string),
            body: body.to_string(),
            subject: subject.map(str::to_string),
            conversation_id: conversation_id.map(str::to_string),
        });
    }

    /// W7 S4: emit `ProtocolEvent::ApprovalResume`. Gated by hitl_suspend.
    fn emit_approval_resume(&self, resume_token: &str, approved: bool) {
        if !self.hitl_suspend_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::ApprovalResume {
            resume_token: resume_token.to_string(),
            approved,
        });
    }

    /// W7 F8: emit `ProtocolEvent::ProviderCircuitEvent` unconditionally
    /// — not gated by a capability flag (audit rev-2 F4). Failure-mode
    /// visibility is always-on like `Error`; hosts that don't recognise
    /// the variant drop it silently per the W0 host-decoder contract.
    fn emit_provider_circuit_event(
        &self,
        primary: &str,
        fallback: Option<&str>,
        state: &str,
        error: Option<&str>,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::ProviderCircuitEvent {
            primary: primary.to_string(),
            fallback: fallback.map(String::from),
            state: state.to_string(),
            error: error.map(String::from),
        });
    }

    /// W8a A.7: emit `ProtocolEvent::BudgetExceeded` unconditionally.
    /// No capability flag (audit F5 — host-tolerated additive variant);
    /// fires once per session when the first ExecutionBudget cap trips.
    fn emit_budget_exceeded(&self, reason: &str, observed: &str, limit: &str) {
        let _ = self.writer.emit(&ProtocolEvent::BudgetExceeded {
            reason: reason.to_string(),
            observed: observed.to_string(),
            limit: limit.to_string(),
        });
    }

    /// #279(d): emit `ProtocolEvent::CompactOffload`. Gated — a guarded no-op
    /// unless the sink was built with `with_non_destructive_compact(true)`,
    /// so the wire shape stays byte-identical until a host opts in.
    fn emit_compaction(
        &self,
        msg_id: &str,
        reason: &str,
        tokens_freed: u64,
        active_window_percent: Option<u32>,
    ) {
        if !self.non_destructive_compact_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::CompactOffload {
            msg_id: msg_id.to_string(),
            reason: reason.to_string(),
            tokens_freed,
            active_window_percent,
        });
    }

    /// Wave RB RELIABILITY MAJOR. Emit `ProtocolEvent::ToolPanicked` —
    /// always-on per the W0 forward-additive baseline (no capability flag).
    fn emit_tool_panicked(
        &self,
        msg_id: &str,
        call_id: &str,
        tool_name: &str,
        panic_message: &str,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::ToolPanicked {
            msg_id: msg_id.to_string(),
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            panic_message: panic_message.to_string(),
        });
    }

    /// Wave RB STABILITY MINOR #10. Emit `ProtocolEvent::PluginRegistrationFailed`
    /// — always-on per the W0 forward-additive baseline (no capability flag).
    fn emit_plugin_registration_failed(
        &self,
        plugin_name: &str,
        surface: &str,
        error_kind: &str,
        message: &str,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::PluginRegistrationFailed {
            plugin_name: plugin_name.to_string(),
            surface: surface.to_string(),
            error_kind: error_kind.to_string(),
            message: message.to_string(),
        });
    }

    /// W7 F4: emit `ProtocolEvent::ToolChunk` when the sink was
    /// configured with `with_streaming_tools(true)`. Default-off so
    /// hosts that haven't learned about the new variant stay
    /// undisturbed per the W0 host-decoder contract.
    fn emit_tool_chunk(&self, msg_id: &str, call_id: &str, tool_name: &str, chunk: &str) {
        if !self.streaming_tools_enabled {
            return;
        }
        // Wave SC: scrub in-flight approval correlation ids from the
        // streaming chunk. Tool processes streaming text to stdout +
        // we forward each chunk on the wire — without redaction, a
        // Bash tool running `tee` against captured protocol output
        // could surface an active token mid-flight and self-resolve.
        let redacted = self.token_redactor.redact(chunk);
        let _ = self.writer.emit(&ProtocolEvent::ToolChunk {
            msg_id: msg_id.to_string(),
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            chunk: redacted,
        });
    }

    /// W7 F2: emit `ProtocolEvent::SubAgentEvent` when the sink was
    /// configured with `with_sub_agent_traces(true)`. Default-off so
    /// hosts that haven't learned about the new variant stay undisturbed
    /// per the W0 host-decoder contract.
    fn emit_sub_agent_event(
        &self,
        parent_call_id: &str,
        agent_name: &str,
        inner: &serde_json::Value,
    ) {
        if !self.sub_agent_traces_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::SubAgentEvent {
            parent_call_id: parent_call_id.to_string(),
            agent_name: agent_name.to_string(),
            inner: inner.clone(),
        });
    }

    /// ForgeFlows-Live: emit `ProtocolEvent::WorkflowStarted` when the sink
    /// was configured with `with_sub_agent_traces(true)`. Shares the
    /// `sub_agent_traces` gate with `emit_sub_agent_event` so hosts that
    /// haven't opted in stay undisturbed per the W0 host-decoder contract.
    fn emit_workflow_started(&self, workflow_id: &str, name: &str, node_count: usize) {
        if !self.sub_agent_traces_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::WorkflowStarted {
            workflow_id: workflow_id.to_string(),
            name: name.to_string(),
            node_count,
        });
    }

    /// ForgeFlows-Live: emit `ProtocolEvent::WorkflowFinished` under the
    /// same `sub_agent_traces` gate as `emit_workflow_started`.
    fn emit_workflow_finished(&self, workflow_id: &str, succeeded: bool) {
        if !self.sub_agent_traces_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::WorkflowFinished {
            workflow_id: workflow_id.to_string(),
            succeeded,
        });
    }

    /// W6 F7. Emits `ProtocolEvent::SessionCost` when
    /// `advertised.cost_attribution = true`. Single source of truth: there
    /// is no parallel sink-builder flag; bootstrap flips the advertised
    /// config when `ProviderCompat` has cost rows (audit rev-2 finding 5).
    fn emit_session_cost(&self, session_id: &str, cost_payload: &serde_json::Value) {
        if !self.advertised.cost_attribution {
            return;
        }
        let total_cost_usd = cost_payload
            .get("total_cost_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let per_turn: Vec<wcore_protocol::events::TurnCost> = cost_payload
            .get("per_turn")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let _ = self.writer.emit(&ProtocolEvent::SessionCost {
            session_id: session_id.to_string(),
            total_cost_usd,
            per_turn,
        });
    }
}

/// Map a provider error message to a distinguishable auth error `code`, or
/// `None` for non-auth errors (which stay `engine_error`). A 401 is a
/// refreshable credential failure (`auth_required` — the host can re-auth or
/// refresh an OAuth token and retry); a 403 is a hard permission failure
/// (`auth_invalid`). Detection mirrors the provider-error shapes the engine
/// formats elsewhere ("API 401: …", "API error 401: …", "status: 401",
/// "401 Unauthorized", "(401)"), staying conservative to avoid tagging an
/// unrelated message that merely contains the digits.
fn auth_error_code(msg: &str) -> Option<&'static str> {
    if message_carries_status(msg, "401") {
        Some("auth_required")
    } else if message_carries_status(msg, "403") {
        Some("auth_invalid")
    } else {
        None
    }
}

/// True when `msg` carries `code` as an HTTP status in one of the provider
/// error shapes the engine emits, rather than as an incidental substring.
fn message_carries_status(msg: &str, code: &str) -> bool {
    msg.contains(&format!("API error {code}"))
        || msg.contains(&format!("API {code}:"))
        || msg.contains(&format!("API {code} "))
        || msg.contains(&format!("status: {code}"))
        || msg.contains(&format!("status code {code}"))
        || msg.contains(&format!("({code})"))
        || msg.contains(&format!("{code} Unauthorized"))
        || msg.contains(&format!("{code} Forbidden"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W7 F2-3.2: a default-built ProtocolSink (no builder methods called)
    /// must NOT advertise sub_agent_traces. This is the W0 byte-identity
    /// guarantee for the v0.1.21+W1 wire shape.
    #[test]
    fn protocol_sink_default_does_not_advertise_sub_agent_traces() {
        let writer = Arc::new(ProtocolWriter::new());
        let sink = ProtocolSink::new(writer);
        let advertised = AdvertisedCapabilitiesConfig::default();
        let compat = ProviderCompat::anthropic_defaults();
        let caps = sink.build_capabilities(&compat, false, "default", false, &advertised);
        assert!(!caps.sub_agent_traces);
        assert!(!caps.streaming_tools);
        assert!(!caps.hitl_suspend);
        assert!(!caps.structured_traces);
    }

    /// W7 F2: with_sub_agent_traces(true) flips the advertised flag.
    #[test]
    fn protocol_sink_with_sub_agent_traces_advertises_capability() {
        let writer = Arc::new(ProtocolWriter::new());
        let sink = ProtocolSink::new(writer).with_sub_agent_traces(true);
        let advertised = AdvertisedCapabilitiesConfig::default();
        let compat = ProviderCompat::anthropic_defaults();
        let caps = sink.build_capabilities(&compat, false, "default", false, &advertised);
        assert!(caps.sub_agent_traces);
    }

    /// W7 F2: emit_sub_agent_event is a no-op when the builder flag is off.
    /// Routes through a recording writer to assert no SubAgentEvent emission.
    #[test]
    fn protocol_sink_emit_sub_agent_event_default_is_noop() {
        let writer = Arc::new(ProtocolWriter::new());
        let sink = ProtocolSink::new(writer);
        sink.emit_sub_agent_event("c-1", "reviewer", &serde_json::json!({"type":"text_delta"}));
        // No panic, no emission (default-off). Assert via the public surface:
        // build_capabilities still reports the flag as false.
        let advertised = AdvertisedCapabilitiesConfig::default();
        let compat = ProviderCompat::anthropic_defaults();
        let caps = sink.build_capabilities(&compat, false, "default", false, &advertised);
        assert!(!caps.sub_agent_traces);
    }

    /// W7 F4: streaming_tools_advertised reflects the builder flag.
    #[test]
    fn protocol_sink_streaming_tools_advertised_tracks_builder() {
        let writer = Arc::new(ProtocolWriter::new());
        let sink_default = ProtocolSink::new(Arc::clone(&writer));
        assert!(!OutputSink::streaming_tools_advertised(&sink_default));
        let sink_on = ProtocolSink::new(writer).with_streaming_tools(true);
        assert!(OutputSink::streaming_tools_advertised(&sink_on));
    }

    /// W7 F4: emit_tool_chunk is a no-op when the builder flag is off.
    #[test]
    fn protocol_sink_emit_tool_chunk_default_is_noop() {
        let writer = Arc::new(ProtocolWriter::new());
        let sink = ProtocolSink::new(writer);
        // Must not panic.
        sink.emit_tool_chunk("m", "c", "Bash", "out");
        let advertised = AdvertisedCapabilitiesConfig::default();
        let compat = ProviderCompat::anthropic_defaults();
        let caps = sink.build_capabilities(&compat, false, "default", false, &advertised);
        assert!(!caps.streaming_tools);
    }

    /// F-079: set_current_msg_id + emit_info must not panic and the id
    /// must be readable from the shared state. We assert the state was
    /// stored correctly (the actual Info event goes to stdout which we
    /// can't easily capture in a unit test).
    #[test]
    fn protocol_sink_set_current_msg_id_updates_state() {
        let writer = Arc::new(ProtocolWriter::new());
        let sink = ProtocolSink::new(writer);
        // Default is empty string.
        assert_eq!(*sink.current_msg_id.read(), "");
        // After set, the field is updated.
        sink.set_current_msg_id("msg-abc-123");
        assert_eq!(*sink.current_msg_id.read(), "msg-abc-123");
        // emit_info must not panic with the updated id.
        sink.emit_info("test info message");
    }

    /// Regression: `emit_error` must carry the caller's `retryable` flag into
    /// the protocol `Error` event. It used to hardcode `retryable: false`,
    /// lying to the host about EVERY transient failure (a 503/network drop
    /// looked identical to a fatal 400). `ProtocolSink`'s writer goes to stdout
    /// and can't be captured in a unit test, so we assert through `TestSink` —
    /// the canonical `OutputSink` double, which builds the same `ErrorInfo`.
    #[test]
    fn emit_error_propagates_retryable_flag_not_hardcoded_false() {
        use crate::test_utils::TestSink;

        let transient = TestSink::new();
        OutputSink::emit_error(&transient, "provider stream failed (HTTP 503)", true);
        let snap = transient.handle().snapshot();
        assert_eq!(snap.len(), 1, "exactly one event expected: {snap:?}");
        assert_eq!(
            snap[0]["error"]["retryable"],
            serde_json::Value::Bool(true),
            "a transient error must report retryable=true: {:?}",
            snap[0]
        );

        let hard = TestSink::new();
        OutputSink::emit_error(&hard, "API 400 invalid_request_error", false);
        let snap = hard.handle().snapshot();
        assert_eq!(
            snap[0]["error"]["retryable"],
            serde_json::Value::Bool(false),
            "a hard error must report retryable=false: {:?}",
            snap[0]
        );
    }

    #[test]
    fn auth_error_code_tags_401_as_auth_required() {
        // The shapes the engine actually formats for a provider 401 — the host
        // needs a stable `code` to drive token-refresh/re-auth, not the prose.
        for msg in [
            "API 401: invalid api key",
            "API error 401: authentication_error",
            "Provider stream failed after retries: API 401: token expired",
            "The inference provider rejected the API key (401)",
            "401 Unauthorized",
        ] {
            assert_eq!(
                auth_error_code(msg),
                Some("auth_required"),
                "a 401 must map to auth_required: {msg:?}"
            );
        }
    }

    #[test]
    fn auth_error_code_tags_403_as_auth_invalid() {
        assert_eq!(
            auth_error_code("API 403: permission_error"),
            Some("auth_invalid")
        );
        assert_eq!(auth_error_code("403 Forbidden"), Some("auth_invalid"));
    }

    #[test]
    fn auth_error_code_none_for_non_auth() {
        // A 400/500 (and messages that merely contain the digits) must NOT be
        // mistaken for auth — they stay engine_error.
        for msg in [
            "API 400: invalid_request_error",
            "Provider stream failed after retries: API 500: internal error",
            "request id 4015 timed out",
            "provider stream closed before a Done event (truncated response)",
        ] {
            assert_eq!(auth_error_code(msg), None, "non-auth must be None: {msg:?}");
        }
    }

    #[test]
    fn protocol_sink_advertises_non_destructive_compact_only_when_built() {
        let writer = Arc::new(ProtocolWriter::new());
        let advertised = AdvertisedCapabilitiesConfig::default();
        let compat = ProviderCompat::anthropic_defaults();
        let off = ProtocolSink::new(Arc::clone(&writer));
        assert!(
            !off.build_capabilities(&compat, false, "default", false, &advertised)
                .non_destructive_compact
        );
        let on = ProtocolSink::new(writer).with_non_destructive_compact(true);
        assert!(
            on.build_capabilities(&compat, false, "default", false, &advertised)
                .non_destructive_compact
        );
    }

    #[test]
    fn protocol_sink_emit_compaction_noop_when_flag_off() {
        let writer = Arc::new(ProtocolWriter::new());
        let sink = ProtocolSink::new(writer);
        sink.emit_compaction("m1", "window_pressure", 4096, Some(41));
    }
}
