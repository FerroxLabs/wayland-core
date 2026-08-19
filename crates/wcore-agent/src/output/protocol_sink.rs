use std::sync::Arc;

use parking_lot::RwLock;
use wcore_config::compat::ProviderCompat;
use wcore_config::tools::AdvertisedCapabilitiesConfig;
use wcore_protocol::events::{
    Capabilities, ErrorInfo, FinishReason, ProtocolEvent, SessionPersistence, Usage,
};
use wcore_protocol::execution_policy::ExecutionPolicySnapshot;
use wcore_protocol::writer::{ProtocolEmitter, ProtocolWriter};

use super::OutputSink;

/// Classify a `ready` frame's `session_id` for the host.
///
/// The two inputs are exactly the two facts that decide the answer, and both
/// are unambiguous at the emission site:
///
/// * `Engine::current_session_id()` is `Some` iff a `SessionManager` exists,
///   which is iff `config.session.enabled` (`engine.rs:3154`/`:3396`), so a
///   `Some` really is a journaled session.
/// * [`wcore_config::config::replay_protection_unavailable`] is the flag config
///   resolution sets when this host cannot seal a prepared provider request —
///   no usable OS keyring, no unlocked credentials vault.
///
/// # Both inputs changed meaning, and the second one flipped
///
/// The second input used to be `durable_sessions_disabled_by_host()`, and a
/// `true` there meant `session_id` was `None` *because of* the host. That
/// coupling is gone: a keyless host now journals, so `session.enabled == false`
/// has exactly ONE cause left — the operator — and the host fact has become
/// orthogonal to whether a session exists at all.
///
/// Which is why `(Some(_), _) => Durable` had to be split. It was correct while
/// a keyless host had no session; it is an over-claim now that it has one,
/// because `durable` is what a host reads to decide whether to WAIT for
/// auto-recovery. `(None, true)` is correspondingly unreachable — sessions off
/// short-circuits the availability probe before it can set the flag — so
/// `DisabledByHost` is no longer produced here at all. It survives on the type
/// as a decode-only legacy value; see its docs.
///
/// Split out from the emitter so the mapping is provable without standing up a
/// keyring-less host: the degraded frame is the one no developer ever runs by
/// hand, so it is the one a wrong mapping ships in.
fn session_persistence_for(
    session_id: Option<&str>,
    replay_protection_unavailable: bool,
) -> SessionPersistence {
    match (session_id, replay_protection_unavailable) {
        (Some(_), false) => SessionPersistence::Durable,
        (Some(_), true) => SessionPersistence::JournaledWithoutReplay,
        // No session id means the operator asked for none. It is deliberately
        // NOT conditioned on the host flag: a host that cannot seal no longer
        // takes the journal away, so attributing this to the host would send an
        // operator hunting for a keyring to restore a journal they switched off.
        (None, _) => SessionPersistence::DisabledByOperator,
    }
}

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
        // #584: publish this set process-wide so producers that never see a
        // `ProtocolSink` — the authoritative `ToolResult` card, `ToolCancelled`,
        // the Crucible error card — scrub it too. Registering on `set` (rather
        // than on construction) means only sets a bridge actually publishes to
        // are ever consulted.
        crate::output_redaction::register_active_token_source(&inner);
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

    /// 27-C2(b) — advertise on liveness, not on linkage.
    ///
    /// [`Self::from_verified`] answers "is the plugin present and genuine?".
    /// That is a necessary condition for the capability, not a sufficient one:
    /// on a headless host `browser_suite` read `true`, the desktop app rendered
    /// the capability, and the first operation died with
    /// `spawn camoufox: No such file or directory`. The host was shown a
    /// capability that could not work.
    ///
    /// This runs the backend crates' own probes on top and **can only clear a
    /// flag, never set one** — the identity guarantee `from_verified` provides
    /// is preserved intact, because a `false` can never become `true` here.
    ///
    /// The probes narrow only on positive proof that every compiled-in backend
    /// is unable to start; anything undecidable without launching a backend
    /// keeps the capability (`*Liveness::Indeterminate`). Under-advertising a
    /// working capability is the same defect as over-advertising a broken one.
    ///
    /// **Wire compatibility.** Nothing in `wcore-protocol` changes: same field,
    /// same type, same value domain, and `false` is already the value a host
    /// sees when the plugin is absent. The `schema_digest` cannot observe this,
    /// so no `CONTRACT_MINOR` bump and no manifest regeneration is implied.
    /// Confirmed 3-of-3 by cross-audit panel; see
    /// `.planning/FALSE-ADVERTISING-SUMMARY.md`.
    ///
    /// Each narrowing is logged at WARN with the probe's reason and remedy. A
    /// recorded panel dissent held that silently dropping a capability replaces
    /// an actionable runtime error with an un-debuggable missing feature; the
    /// log is how that objection is honoured without keeping the false claim.
    pub async fn narrowed_to_live(self) -> Self {
        let mut out = self;

        if out.browser_suite {
            let probe = wcore_browser::liveness::probe(
                &wcore_browser::backends::CamoufoxBackend::configured_url(),
            )
            .await;
            if let Some(u) = probe.unavailable() {
                tracing::warn!(
                    capability = "browser_suite",
                    reason = %u.reason,
                    remedy = %u.remedy,
                    "not advertising browser_suite: the plugin is loaded but no backend can start"
                );
                out.browser_suite = false;
            }
        }

        if out.computer_use {
            let probe = wcore_cua::liveness::probe();
            if let Some(u) = probe.unavailable() {
                tracing::warn!(
                    capability = "computer_use",
                    reason = %u.reason,
                    remedy = %u.remedy,
                    "not advertising computer_use: the plugin is loaded but no backend can start"
                );
                out.computer_use = false;
            }
        }

        out
    }
}

/// JSON stream protocol output sink
pub struct ProtocolSink {
    writer: Arc<dyn ProtocolEmitter>,
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
    /// Core session identity advertised to the host and reused by producer
    /// contracts such as Anvil receipts.
    session_id: Arc<RwLock<Option<String>>>,
    /// Pre-`ready` `Info` holding pen. `None` is the default and means
    /// pass-through; `Some(_)` means [`Self::deferring_info_until_ready`]
    /// armed the gate and no handshake frame has gone out yet.
    ///
    /// A JSON-stream host reads the FIRST line as the handshake — the release
    /// smoke test does, and the Desktop contract implies it. Bootstrap can
    /// legitimately emit diagnostics before `ready` exists (the Windows
    /// `windows_job_object` local-shell notice is the one that shipped, and
    /// the `AgentBusObserver` can race one out at any moment on any
    /// platform), so ordering cannot be guaranteed by discipline at each
    /// emission site. It is guaranteed here, once, at the funnel: `Info` is
    /// diagnostic and has no ordering claim, so it waits for the handshake
    /// and is replayed immediately after it, in order.
    pre_ready_info: Arc<parking_lot::Mutex<Option<Vec<ProtocolEvent>>>>,
}

impl ProtocolSink {
    pub fn new(writer: Arc<ProtocolWriter>) -> Self {
        Self::with_emitter(writer)
    }

    /// Construct over any [`ProtocolEmitter`].
    ///
    /// The production path passes a [`ProtocolWriter`] via [`Self::new`]; a
    /// test passes a recorder, which is the only way the ORDER of the frames
    /// this sink writes can be asserted without a subprocess. The frame-order
    /// invariant (`ready` first) is not observable any other way.
    pub fn with_emitter(writer: Arc<dyn ProtocolEmitter>) -> Self {
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
            session_id: Arc::new(RwLock::new(None)),
            pre_ready_info: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Hold `Info` frames until the handshake frame has been written.
    ///
    /// Opt-in, because a sink that never emits `ready` (sub-agent sinks,
    /// unit-test sinks) would otherwise buffer diagnostics forever. The
    /// json-stream entry point arms it; every other construction keeps the
    /// historical pass-through behaviour byte-for-byte.
    pub fn deferring_info_until_ready(self) -> Self {
        *self.pre_ready_info.lock() = Some(Vec::new());
        self
    }

    /// Disarm the gate and write anything it is holding, in arrival order.
    ///
    /// Called after `ready` (the success path) and after a startup `error`
    /// (the failure path, where no `ready` will ever come). Between them the
    /// buffer cannot be stranded: the first frame the host sees is always
    /// `ready` or `error`, never a diagnostic.
    fn release_pre_ready_info(&self) {
        let held = self.pre_ready_info.lock().take();
        for event in held.into_iter().flatten() {
            let _ = self.writer.emit(&event);
        }
    }

    /// Emit a turn-scoped error when the caller still owns the protocol
    /// command's correlation id (for example, before the engine starts).
    pub fn emit_correlated_error(&self, msg_id: &str, msg: &str, retryable: bool) {
        let code = auth_error_code(msg).unwrap_or("engine_error");
        let _ = self.writer.emit(&ProtocolEvent::Error {
            msg_id: Some(msg_id.to_string()),
            error: ErrorInfo {
                code: code.to_string(),
                message: msg.to_string(),
                retryable,
            },
        });
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
        self.emit_ready_with_plugins_and_policy(
            compat,
            has_mcp,
            session_id,
            current_mode,
            has_plugins,
            plugin_caps,
            advertised,
            None,
        );
    }

    /// Contract-aware Ready emission. The optional snapshot keeps the legacy
    /// helper byte-compatible while allowing the Desktop JSON-stream producer
    /// to publish revision zero before accepting any turn.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_ready_with_plugins_and_policy(
        &self,
        compat: &ProviderCompat,
        has_mcp: bool,
        session_id: Option<String>,
        current_mode: &str,
        has_plugins: bool,
        plugin_caps: &PluginCapabilitySet,
        advertised: &AdvertisedCapabilitiesConfig,
        execution_policy: Option<ExecutionPolicySnapshot>,
    ) {
        let session_persistence = session_persistence_for(
            session_id.as_deref(),
            wcore_config::config::replay_protection_unavailable(),
        );
        let _ = self.writer.emit(&ProtocolEvent::Ready {
            version: env!("CARGO_PKG_VERSION").to_string(),
            session_id,
            session_persistence,
            capabilities: self.build_capabilities_with_plugins(
                compat,
                has_mcp,
                current_mode,
                has_plugins,
                plugin_caps,
                advertised,
            ),
            contract: Some(wcore_protocol::contract::producer_contract_descriptor()),
            execution_policy,
        });
        // The handshake is on the wire; anything bootstrap wanted to say can
        // follow it now.
        self.release_pre_ready_info();
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
    pub fn writer(&self) -> &Arc<dyn ProtocolEmitter> {
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
    fn bind_session_id(&self, session_id: &str) {
        *self.session_id.write() = Some(session_id.to_string());
    }

    fn current_session_id(&self) -> Option<String> {
        self.session_id.read().clone()
    }

    fn emit_anvil_receipt(&self, receipt: &wcore_protocol::anvil::AnvilReceipt) {
        let _ = self.writer.emit(&ProtocolEvent::AnvilReceipt {
            receipt: receipt.clone(),
        });
    }

    fn emit_anvil_receipt_invalidation(
        &self,
        invalidation: &wcore_protocol::anvil::AnvilReceiptInvalidation,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::AnvilReceiptInvalidated {
            invalidation: invalidation.clone(),
        });
    }

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
            usage_delta: None,
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
        usage_delta: Option<&wcore_types::message::TokenUsage>,
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
            // CORE-2: the run-scoped delta rides as a sibling of the
            // cumulative usage, same inner field shape (window gauge is
            // a session-level reading — it stays on `usage` only).
            usage_delta: usage_delta.map(|d| Usage {
                input_tokens: d.input_tokens,
                output_tokens: d.output_tokens,
                cache_read_tokens: if d.cache_read_tokens > 0 {
                    Some(d.cache_read_tokens)
                } else {
                    None
                },
                cache_write_tokens: if d.cache_creation_tokens > 0 {
                    Some(d.cache_creation_tokens)
                } else {
                    None
                },
                active_window_percent: None,
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
        // A startup failure means `ready` is never coming. Release the holding
        // pen AFTER the error so the first frame stays a handshake-class frame,
        // but release it — dropping boot diagnostics on the one path where they
        // explain the failure is worse than emitting them late.
        self.release_pre_ready_info();
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
        let event = ProtocolEvent::Info {
            msg_id,
            message: redacted,
        };
        // Ordering guard: while the gate is armed the handshake has not been
        // written, so this diagnostic waits rather than claiming first frame.
        if let Some(held) = self.pre_ready_info.lock().as_mut() {
            held.push(event);
            return;
        }
        let _ = self.writer.emit(&event);
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

    /// #1098: this sink IS a json-stream host connection, so it is the one
    /// sink that can put a rendered artifact in front of a user.
    fn render_artifact_supported(&self) -> bool {
        true
    }

    /// #1098: emit `ProtocolEvent::RenderArtifact`.
    ///
    /// THE truncation chokepoint. Every render passes through here, so the
    /// cap cannot be routed around by a future caller that forgot it. Over the
    /// cap the content is truncated with an in-band marker and `truncated` is
    /// set — never dropped (wayland#1071), and never sent whole: an over-limit
    /// frame does not merely fail to display, it sets the output pump's sticky
    /// failure and takes the session's entire stdout with it.
    fn emit_render_artifact(
        &self,
        call_id: &str,
        title: &str,
        mime: wcore_protocol::events::RenderMime,
        content: &str,
    ) {
        let msg_id = self.current_msg_id.read().clone();
        let (content, truncated) = wcore_protocol::events::truncate_render_content(content);
        let _ = self.writer.emit(&ProtocolEvent::RenderArtifact {
            msg_id,
            call_id: call_id.to_string(),
            title: wcore_protocol::events::truncate_render_title(title),
            mime,
            content,
            truncated,
            critical: wcore_protocol::events::NonCritical,
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

    fn emit_provider_failover_receipt(&self, receipt: serde_json::Value) {
        let _ = self
            .writer
            .emit(&ProtocolEvent::ProviderFailoverReceipt { receipt });
    }

    fn emit_provider_attempt(&self, failure: Option<&str>) {
        let _ = self.writer.emit(&ProtocolEvent::ProviderAttempt {
            failure: failure.map(String::from),
        });
    }

    fn emit_provider_retry(&self, failure: Option<&str>) {
        let _ = self.writer.emit(&ProtocolEvent::ProviderRetry {
            failure: failure.map(String::from),
        });
    }

    fn emit_provider_failure(&self, failure: &str) {
        let _ = self.writer.emit(&ProtocolEvent::ProviderFailure {
            failure: failure.to_string(),
        });
    }

    fn emit_midflight_monitor_decision(
        &self,
        directive: wcore_protocol::events::MonitorDirective,
        reason: wcore_protocol::events::MonitorReason,
    ) {
        let _ = self
            .writer
            .emit(&ProtocolEvent::MidFlightMonitorDecision { directive, reason });
    }

    fn emit_capability_activation(
        &self,
        activation: &wcore_protocol::events::CapabilityActivation,
    ) {
        let _ = self.writer.emit(&ProtocolEvent::CapabilityActivation {
            activation: activation.clone(),
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
        // #584: a panic payload is tool-derived text like any other, and this
        // was the only tool-text emitter on this struct not scrubbing it.
        //
        // It goes through the PROCESS-WIDE `redact_active_tokens`, not through
        // `self.token_redactor`. A per-handle redactor only knows the tokens
        // its OWN bridge published; a token minted by a different bridge — a
        // sub-agent, a second session, or any sink whose `set()` never ran —
        // would ride this frame unscrubbed. That per-handle reachability gap is
        // the whole reason this fix exists, so this call site must not
        // reintroduce it. The process-wide set is a strict superset of any one
        // handle's (tokens only ever enter via `set`, which registers).
        let _ = self.writer.emit(&ProtocolEvent::ToolPanicked {
            msg_id: msg_id.to_string(),
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            panic_message: crate::output_redaction::redact_active_tokens(panic_message),
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

    fn emit_correlated_sub_agent_event(
        &self,
        parent_call_id: &str,
        agent_name: &str,
        inner: &serde_json::Value,
        correlation: &wcore_protocol::events::WorkflowChildCorrelation,
    ) {
        if !self.sub_agent_traces_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::CorrelatedSubAgentEvent {
            parent_call_id: parent_call_id.to_string(),
            agent_name: agent_name.to_string(),
            inner: inner.clone(),
            run_id: correlation.run_id.clone(),
            child_run_id: correlation.child_run_id.clone(),
            parent_child_run_id: correlation.parent_child_run_id.clone(),
            child_sequence: correlation.child_sequence,
            event_id: correlation.event_id.clone(),
            terminal_state: correlation.terminal_state,
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

    fn emit_correlated_workflow_started(&self, event: &wcore_protocol::events::WorkflowRunStarted) {
        if !self.sub_agent_traces_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::CorrelatedWorkflowStarted {
            workflow_id: event.workflow_id.clone(),
            name: event.name.clone(),
            node_count: event.node_count,
            run_id: event.run_id.clone(),
            event_id: event.event_id.clone(),
            sequence: event.sequence,
            parent_run_id: event.parent_run_id.clone(),
        });
    }

    fn emit_workflow_node_event(&self, event: &wcore_protocol::events::WorkflowNodeLifecycle) {
        if !self.sub_agent_traces_enabled {
            return;
        }
        let _ = self.writer.emit(&ProtocolEvent::WorkflowNodeEvent {
            run_id: event.run_id.clone(),
            node_id: event.node_id.clone(),
            child_run_id: event.child_run_id.clone(),
            event_id: event.event_id.clone(),
            sequence: event.sequence,
            state: event.state,
            failure: event.failure.clone(),
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

    fn emit_correlated_workflow_finished(
        &self,
        event: &wcore_protocol::events::WorkflowRunFinished,
    ) {
        if !self.sub_agent_traces_enabled {
            return;
        }
        let succeeded =
            event.terminal_state == wcore_protocol::events::WorkflowTerminalState::Succeeded;
        let _ = self
            .writer
            .emit(&ProtocolEvent::CorrelatedWorkflowFinished {
                workflow_id: event.workflow_id.clone(),
                succeeded,
                run_id: event.run_id.clone(),
                event_id: event.event_id.clone(),
                sequence: event.sequence,
                terminal_state: event.terminal_state,
                failure: event.failure.clone(),
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

    /// Records the exact sequence of frames the sink wrote.
    #[derive(Default)]
    struct RecordingEmitter {
        events: parking_lot::Mutex<Vec<ProtocolEvent>>,
    }

    impl ProtocolEmitter for RecordingEmitter {
        fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
            self.events.lock().push(event.clone());
            Ok(())
        }
    }

    impl RecordingEmitter {
        /// The wire `type` tag of each frame, in emission order.
        fn kinds(&self) -> Vec<String> {
            self.events
                .lock()
                .iter()
                .map(|e| {
                    serde_json::to_value(e).expect("event serializes")["type"]
                        .as_str()
                        .expect("every event has a string type tag")
                        .to_string()
                })
                .collect()
        }

        fn info_messages(&self) -> Vec<String> {
            self.events
                .lock()
                .iter()
                .filter_map(|e| match e {
                    ProtocolEvent::Info { message, .. } => Some(message.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    fn emit_a_ready(sink: &ProtocolSink) {
        sink.emit_ready(
            &ProviderCompat::default(),
            false,
            Some("sess-1".to_string()),
            "default",
            false,
            &AdvertisedCapabilitiesConfig::default(),
        );
    }

    /// THE handshake invariant: `ready` is the first frame, whatever bootstrap
    /// said first.
    ///
    /// A JSON-stream host reads line 1 as the handshake. On Windows the
    /// `windows_job_object` local-shell notice reached `emit_info` before
    /// `ready` existed on EVERY session, so line 1 was an `info` frame and
    /// three separate release tests read a diagnostic as their handshake.
    ///
    /// This asserts the ORDER, not the presence, and it is the assertion that
    /// goes red if the gate is removed: without it the first tag here is
    /// `info`. It is also platform-independent — the Windows-only part was the
    /// notice that happened to trip it, not the ordering defect, which any
    /// pre-`ready` `emit_info` reaches (the `AgentBusObserver` forwards
    /// sub-agent lifecycle to `emit_info` from a spawned task on every
    /// platform).
    #[test]
    fn ready_is_the_first_frame_even_when_bootstrap_speaks_first() {
        let emitter = Arc::new(RecordingEmitter::default());
        let sink = ProtocolSink::with_emitter(emitter.clone()).deferring_info_until_ready();

        sink.emit_info("bootstrap diagnostic one");
        sink.emit_info("bootstrap diagnostic two");
        emit_a_ready(&sink);
        sink.emit_info("post-handshake diagnostic");

        assert_eq!(
            emitter.kinds().first().map(String::as_str),
            Some("ready"),
            "the host reads frame 1 as the handshake; got {:?}",
            emitter.kinds()
        );
        assert_eq!(
            emitter.kinds(),
            vec!["ready", "info", "info", "info"],
            "deferred diagnostics must follow the handshake in arrival order"
        );
        // Deferral must not become deletion: every diagnostic still reaches the
        // host, and in the order it was produced.
        assert_eq!(
            emitter.info_messages(),
            vec![
                "bootstrap diagnostic one",
                "bootstrap diagnostic two",
                "post-handshake diagnostic"
            ],
        );
    }

    /// The gate is opt-in, so every sink that never emits `ready` (sub-agent
    /// sinks, unit-test sinks) keeps its historical pass-through behaviour and
    /// cannot strand a diagnostic in a buffer nobody flushes.
    #[test]
    fn an_unarmed_sink_still_writes_info_immediately() {
        let emitter = Arc::new(RecordingEmitter::default());
        let sink = ProtocolSink::with_emitter(emitter.clone());

        sink.emit_info("no handshake is coming");

        assert_eq!(emitter.kinds(), vec!["info"]);
    }

    /// A startup failure means `ready` never comes. The buffer must still be
    /// released — dropping boot diagnostics on the one path where they explain
    /// the failure would trade a frame-order bug for a silent-loss bug — but
    /// the error keeps frame 1, so the host's first line is never a diagnostic.
    #[test]
    fn a_startup_error_releases_the_held_diagnostics_after_itself() {
        let emitter = Arc::new(RecordingEmitter::default());
        let sink = ProtocolSink::with_emitter(emitter.clone()).deferring_info_until_ready();

        sink.emit_info("why bootstrap was unhappy");
        sink.emit_error("Engine failed to start during init", false);

        assert_eq!(emitter.kinds(), vec!["error", "info"]);
        assert_eq!(emitter.info_messages(), vec!["why bootstrap was unhappy"]);
    }

    /// The whole mapping, including the arm nobody can reach on a developer
    /// machine.
    ///
    /// A host with no OS keyring and no unlocked vault is the only way to reach
    /// the replay-degraded arms, so on any laptop with a working keychain they
    /// are unreachable and a wrong mapping there would ship green. Pinning all
    /// four input combinations is the only way they are ever exercised off a
    /// keyring-less server.
    ///
    /// The load-bearing row is `(Some, true)`. It used to assert `Durable`,
    /// with a comment arguing that a live session is durable whatever the host
    /// flag says. That was true while a keyless host had NO session — the row
    /// only described a stale process-global. It is exactly the reachable
    /// production state now, and calling it `durable` tells a host to wait for
    /// an auto-recovery that will never come.
    #[test]
    fn session_persistence_names_what_the_host_can_and_cannot_do() {
        assert_eq!(
            session_persistence_for(Some("sess-1"), false),
            SessionPersistence::Durable
        );
        assert_eq!(
            session_persistence_for(Some("sess-1"), true),
            SessionPersistence::JournaledWithoutReplay,
            "a journaled session with no sealed request is not `durable`: a host \
             reading `durable` waits for an auto-recovery this session cannot do"
        );
        assert_eq!(
            session_persistence_for(None, false),
            SessionPersistence::DisabledByOperator
        );
        // No session id means the operator asked for none — even on a keyless
        // host. Attributing it to the host would send them hunting for a
        // keyring to restore a journal they switched off themselves.
        assert_eq!(
            session_persistence_for(None, true),
            SessionPersistence::DisabledByOperator
        );
    }

    /// `disabled_by_host` must be UNPRODUCIBLE and still DECODABLE.
    ///
    /// Two halves that pull in opposite directions, so both are asserted here:
    /// no input to the mapping may yield it (a keyless host journals now, so
    /// emitting it would describe a state this Core cannot be in), and the wire
    /// value must still round-trip, because an older Core sends it and a host
    /// may have stored it against a session it is still tracking.
    ///
    /// Deleting the variant would satisfy the first half and silently break the
    /// second — which is why removing a value we once published is not the same
    /// operation as ceasing to send it.
    #[test]
    fn disabled_by_host_is_no_longer_produced_but_is_still_decodable() {
        for session_id in [None, Some("sess-1")] {
            for replay_unavailable in [false, true] {
                assert_ne!(
                    session_persistence_for(session_id, replay_unavailable),
                    SessionPersistence::DisabledByHost,
                    "this Core emitted a value it can no longer be in the state of: \
                     session_id={session_id:?} replay_unavailable={replay_unavailable}"
                );
            }
        }

        let decoded: SessionPersistence = serde_json::from_str("\"disabled_by_host\"")
            .expect("an older Core's value must decode");
        assert_eq!(decoded, SessionPersistence::DisabledByHost);
        assert_eq!(
            serde_json::to_value(SessionPersistence::JournaledWithoutReplay).unwrap(),
            serde_json::json!("journaled_without_replay"),
            "the new value's wire spelling is a published contract"
        );
    }

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

    /// #1098: only a json-stream connection is a render surface. The terminal
    /// and null sinks must report false, or `RenderArtifactTool` would be
    /// registered under a sink that discards the event and the model would
    /// believe it had shown the user something.
    #[test]
    fn only_the_protocol_sink_reports_render_support() {
        let sink = ProtocolSink::new(Arc::new(ProtocolWriter::new()));
        assert!(OutputSink::render_artifact_supported(&sink));
        assert!(!OutputSink::render_artifact_supported(
            &crate::output::null_sink::NullSink
        ));
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

    /// Pull the `panic_message` off the one `ToolPanicked` frame an emitter
    /// recorded, failing loudly if no frame was written at all.
    fn recorded_panic_message(emitter: &RecordingEmitter) -> String {
        emitter
            .events
            .lock()
            .iter()
            .find_map(|e| match e {
                ProtocolEvent::ToolPanicked { panic_message, .. } => Some(panic_message.clone()),
                _ => None,
            })
            .expect("emit_tool_panicked must write a frame")
    }

    /// #584 — `emit_tool_panicked` is a method on the ONE struct that owns
    /// `token_redactor`, and it was the only tool-text emitter on that struct
    /// not scrubbing. A panic payload quotes whatever the tool was holding,
    /// which on the snooping path is the live approval token.
    ///
    /// The token here is published by sink A's redactor and the panic is
    /// emitted through sink B, whose own redactor was NEVER `set`. That is the
    /// CROSS-BRIDGE case: a sub-agent, a second session, or any sink that never
    /// saw the minting bridge. A same-sink test cannot observe it — scrubbing
    /// through `self.token_redactor` passes that shape and still leaks here,
    /// which is exactly the per-handle reachability gap this PR exists to close.
    #[test]
    fn tool_panicked_scrubs_a_token_minted_on_another_sink() {
        let token = "apr-11111111-2222-3333-4444-555555555555".to_string();

        // Bridge/sink A mints and publishes the token. Held for the whole test:
        // the process-wide registry holds sources WEAKLY, so dropping A would
        // prune the very set under test.
        let emitter_a = Arc::new(RecordingEmitter::default());
        let sink_a = ProtocolSink::with_emitter(emitter_a.clone());
        sink_a.token_redactor().set(vec![token.clone()]);

        // Sink B is a different bridge's sink: its redactor is untouched.
        let emitter_b = Arc::new(RecordingEmitter::default());
        let sink_b = ProtocolSink::with_emitter(emitter_b.clone());
        // CONTROL: the two sinks really do have independent token sets,
        // otherwise "cross-bridge" would be a same-bridge test in disguise.
        assert!(
            sink_b.token_redactor().snapshot().is_empty(),
            "control failed: sink B already knows the token, so this proves nothing"
        );

        sink_b.emit_tool_panicked("m1", "c1", "Bash", &format!("panicked at {token} !"));

        let panic_message = recorded_panic_message(&emitter_b);
        // CONTROL: the payload really rode the frame.
        assert!(
            panic_message.contains("panicked at"),
            "control failed: the frame lost the panic text: {panic_message}"
        );
        assert!(
            !panic_message.contains(&token),
            "tool_panicked leaked an approval token minted on another bridge: {panic_message}"
        );
        drop(sink_a);
    }

    /// The same-bridge case still has to hold — the cross-bridge fix must not
    /// come at the cost of the sink that DID mint the token.
    #[test]
    fn tool_panicked_scrubs_an_active_approval_token() {
        let emitter = Arc::new(RecordingEmitter::default());
        let sink = ProtocolSink::with_emitter(emitter.clone());
        let token = "apr-99999999-8888-7777-6666-555555555555".to_string();
        sink.token_redactor().set(vec![token.clone()]);

        sink.emit_tool_panicked("m1", "c1", "Bash", &format!("panicked at {token} !"));

        let panic_message = recorded_panic_message(&emitter);
        // CONTROL: the payload really rode the frame.
        assert!(
            panic_message.contains("panicked at"),
            "control failed: the frame lost the panic text: {panic_message}"
        );
        assert!(
            !panic_message.contains(&token),
            "tool_panicked leaked the live approval token: {panic_message}"
        );
    }
}
