//! Engine bridge — connects the live `AgentEngine` to the TUI.
//!
//! Wave 2 (T2.1). The TUI is a pure consumer of the same `ProtocolEvent`
//! stream the `--json-stream` host protocol uses, but in-process: there is
//! no subprocess and no stdout. This module supplies the two halves of
//! that wiring:
//!
//!  * [`ChannelEmitter`] — a [`ProtocolEmitter`] that forwards every
//!    event into a `tokio::mpsc` channel instead of writing JSON to
//!    stdout. The engine emits **tool-lifecycle** events
//!    (`ToolRequest` / `ToolRunning` / `ToolResult` / `ToolCancelled` /
//!    `ApprovalRequired`) through the writer set by
//!    `AgentEngine::set_protocol_writer`.
//!  * [`ChannelSink`] — an [`OutputSink`] that translates the engine's
//!    `emit_*` calls into `ProtocolEvent`s and forwards them on the same
//!    channel. The engine emits **streaming** events (`StreamStart` /
//!    `TextDelta` / `Thinking` / `StreamEnd` / `Error` / `Info` …)
//!    through its `OutputSink`.
//!
//! Both halves push onto one `mpsc::Sender<ProtocolEvent>`; the
//! [`spawn_bridge`](super::protocol_bridge::spawn_bridge) task drains the
//! receiver and folds each event into the shared `App`.
//!
//! [`TuiEngine`] is the controller the render loop talks to: it owns the
//! built engine and the approval manager, drives `engine.run` for a
//! submitted prompt on a background task, routes approve/deny decisions,
//! and cancels an in-flight turn.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use wcore_agent::output::OutputSink;
use wcore_protocol::events::{ErrorInfo, FinishReason, ProtocolEvent, ToolStatus, Usage};
use wcore_protocol::writer::ProtocolEmitter;
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};

// ─────────────────────────────────────────────────────────────────────────
// ChannelEmitter — a ProtocolEmitter that forwards into an mpsc channel
// ─────────────────────────────────────────────────────────────────────────

/// A [`ProtocolEmitter`] that forwards every event into an in-process
/// `mpsc` channel instead of serializing JSON to stdout.
///
/// Installed on the engine via `set_protocol_writer`. The engine emits
/// tool-lifecycle and approval events through this path. A send that
/// fails (the receiver — the TUI — has shut down) is dropped silently:
/// there is nothing useful to do once the UI is gone, and surfacing an
/// `io::Error` from a torn-down channel would only obscure the real
/// exit cause.
pub struct ChannelEmitter {
    tx: UnboundedSender<ProtocolEvent>,
    /// Wave 6 #24 — call_ids for which this emitter already SYNTHESIZED an
    /// `ApprovalRequired` after a `ToolRequest`. When `Some`, a later EXPLICIT
    /// `ApprovalRequired` for the same call_id is suppressed, so a gated site
    /// that emits its OWN `ApprovalRequired` (the live-workflow gate in
    /// `engine.rs`, which emits `ToolRequest` + explicit `ApprovalRequired`)
    /// yields exactly ONE gate frame instead of two — the second of which
    /// projects malformed (empty name / null input) on the ACP surface. This
    /// mirrors `GatingProtocolWriter`'s `synthesized` set in the json-stream
    /// path. `None` (the stateless [`ChannelEmitter::new`]) keeps the original
    /// always-synthesize behavior for unit tests and any non-persistent caller.
    synthesized: Option<DedupeSet>,
    /// GHSA-8r7g: sync correlation→secret lookup so a synthesized gate frame
    /// carries the unguessable `resume_token` for a bridge-backed approval
    /// (crucible/egress), and EMPTY for a regular tool (no bridge entry —
    /// resolved via `ToolApprove`). All production transports (TUI and, since
    /// wayland#497, the ACP relay) pass `Some`; `None` (unit tests only)
    /// falls back to the legacy call_id-as-token emit. NOTE: the ACP
    /// PROJECTION still drops `resume_token` and its resolve endpoint stays
    /// call_id-keyed — carrying/accepting the secret end-to-end on ACP is
    /// tracked separately (see the RelayEmitter field doc).
    approval_bridge: Option<Arc<wcore_agent::approval::ApprovalBridge>>,
}

/// Shared "already-synthesized a gate frame" set, held by the persistent
/// emitter owner so the dedupe state survives across `emit` calls (the ACP
/// `RelayEmitter` reconstructs a `ChannelEmitter` per emit, so the set lives in
/// the relay, not the emitter).
pub type DedupeSet = Arc<Mutex<HashSet<String>>>;

impl ChannelEmitter {
    /// Build a stateless emitter that forwards onto `tx` and synthesizes an
    /// `ApprovalRequired` after every `ToolRequest`. Used by callers that never
    /// also emit an explicit `ApprovalRequired` for the same call_id.
    pub fn new(tx: UnboundedSender<ProtocolEvent>) -> Self {
        Self {
            tx,
            synthesized: None,
            approval_bridge: None,
        }
    }

    /// Build an emitter that shares a persistent `synthesized` set so a later
    /// explicit `ApprovalRequired` for an already-synthesized call_id is
    /// suppressed (Wave 6 #24 single-gate dedupe). Use this for the persistent
    /// TUI/ACP emitters where the engine may also emit its own gate frame.
    /// GHSA-8r7g: `approval_bridge` supplies the sync correlation→secret lookup
    /// used to stamp the unguessable `resume_token` onto a bridge-backed gate
    /// frame (`Some` for the TUI and, since wayland#497, the ACP relay;
    /// `None` only in unit tests).
    pub fn with_dedupe(
        tx: UnboundedSender<ProtocolEvent>,
        synthesized: DedupeSet,
        approval_bridge: Option<Arc<wcore_agent::approval::ApprovalBridge>>,
    ) -> Self {
        Self {
            tx,
            synthesized: Some(synthesized),
            approval_bridge,
        }
    }
}

impl ProtocolEmitter for ChannelEmitter {
    fn emit(&self, event: &ProtocolEvent) -> io::Result<()> {
        // Wave 6 #24 — when a persistent `synthesized` set is present, drop a
        // duplicate EXPLICIT `ApprovalRequired` for a call_id we already
        // synthesized a gate frame for (the live-workflow gate in `engine.rs`
        // emits `ToolRequest` + its own `ApprovalRequired`). Without this, the
        // ACP projection consumes the `pending_calls` entry on the first frame
        // and projects the second with an empty tool name / null input — a
        // malformed phantom gate. Mirrors `GatingProtocolWriter`.
        if let ProtocolEvent::ApprovalRequired { call_id, .. } = event
            && let Some(seen) = &self.synthesized
            && seen.lock().map(|s| s.contains(call_id)).unwrap_or(false)
        {
            return Ok(());
        }

        // `ProtocolEvent` derives `Clone` (Wave 2) so the event can be
        // forwarded by value without re-serializing. A closed channel is
        // not an error — the TUI has simply gone away.
        let _ = self.tx.send(event.clone());

        // E3 W4: synthesize `ApprovalRequired` after every `ToolRequest`.
        //
        // The engine's orchestration approval path
        // (`execute_tool_calls_with_approval`) emits `ToolRequest` ONLY
        // when a tool actually needs human approval — allow-listed tools
        // (web/web_fetch/vision/transcribe) and auto-approved categories
        // skip the request entirely (orchestration/mod.rs:843-861). So a
        // `ToolRequest` reaching the TUI's emitter unambiguously means
        // "the engine is blocked on `approval_manager.request_approval`
        // for this call_id." The orchestration path emits `ToolRequest`
        // but never the matching `ApprovalRequired` event (the in-process
        // channel makes the explicit event redundant for the engine
        // itself), which leaves the TUI's protocol bridge with no signal
        // to flip the card status to `AwaitingApproval` and render the
        // inline approval card. We synthesize the event here so the
        // existing bridge handler at
        // `protocol_bridge.rs::ApprovalRequired` fires and
        // `widgets::render_approval_inline` surfaces it in the transcript
        // (v0.9.1 W1-B replaced the modal overlay with an inline card +
        // right-rail pending mirror).
        //
        // Allow-list bypass: read-only tools never produce a
        // `ToolRequest`, so this synthesis is a no-op for them. The
        // bypass logic lives in the engine (orchestration/mod.rs:844-846,
        // `!allow_list.contains(name)`) and is untouched.
        if let ProtocolEvent::ToolRequest {
            msg_id: _,
            call_id,
            tool,
        } = event
        {
            let reason = match tool.category {
                wcore_protocol::events::ToolCategory::Edit => "edit",
                wcore_protocol::events::ToolCategory::Exec => "exec",
                wcore_protocol::events::ToolCategory::Mcp => "mcp",
                wcore_protocol::events::ToolCategory::Info => "info",
            };
            let context = tool.description.clone();
            // GHSA-8r7g: stamp the bridge's SECRET resume_token (looked up by
            // this call_id) onto the host-visible gate frame — EMPTY for a
            // regular tool (no bridge entry; resolved via `ToolApprove`). With
            // no bridge (ACP relay / unit tests) fall back to the legacy
            // behavior of emitting the call_id itself as the resume handle.
            let resume_token = match &self.approval_bridge {
                Some(bridge) => bridge.secret_for_correlation(call_id).unwrap_or_default(),
                None => call_id.clone(),
            };
            // Record that we synthesized a gate for this call_id so a later
            // explicit `ApprovalRequired` (from a self-gating engine site) is
            // suppressed above. No-op for the stateless `new` emitter.
            if let Some(seen) = &self.synthesized
                && let Ok(mut s) = seen.lock()
            {
                s.insert(call_id.clone());
            }
            // Crucible Stage 4: carry the typed proposal card through the
            // synthesized gate frame. The explicit ApprovalRequired{plan} is
            // suppressed above, so the plan must ride the ToolRequest's
            // `tool.args` (engine sets it for the Crucible council). Absent/
            // unparseable for every other tool → None (unchanged behavior).
            let plan = tool.args.get("plan").and_then(|v| {
                serde_json::from_value::<wcore_types::crucible::CruciblePlan>(v.clone()).ok()
            });
            let _ = self.tx.send(ProtocolEvent::ApprovalRequired {
                call_id: call_id.clone(),
                resume_token,
                // GHSA-8r7g: the public correlation handle is the call_id, not
                // the (now possibly secret/empty) resume_token.
                correlation_id: call_id.clone(),
                reason: reason.to_string(),
                context,
                plan,
            });
        }

        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────
// ChannelSink — an OutputSink that forwards into the same mpsc channel
// ─────────────────────────────────────────────────────────────────────────

/// An [`OutputSink`] that translates the engine's streaming `emit_*`
/// calls into `ProtocolEvent`s and forwards them on the same channel as
/// [`ChannelEmitter`].
///
/// This mirrors `ProtocolSink`'s event translation, but with every
/// host-decoder capability gate ON: the TUI is an in-process consumer
/// that wants the full event stream (sub-agent traces, streaming tool
/// chunks, HITL suspend, structured traces). `ProtocolSink` is not
/// reused directly because its constructor binds a concrete
/// `ProtocolWriter` (stdout), not an arbitrary emitter.
pub struct ChannelSink {
    tx: UnboundedSender<ProtocolEvent>,
}

impl ChannelSink {
    /// Build a sink that forwards onto `tx`.
    pub fn new(tx: UnboundedSender<ProtocolEvent>) -> Self {
        Self { tx }
    }

    /// Forward one event, dropping the result — a closed channel means
    /// the TUI shut down and there is nothing to recover.
    fn send(&self, event: ProtocolEvent) {
        let _ = self.tx.send(event);
    }
}

impl OutputSink for ChannelSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.send(ProtocolEvent::TextDelta {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
        });
    }

    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.send(ProtocolEvent::Thinking {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
            subject: None,
        });
    }

    fn emit_thinking_subject(&self, subject: &str, msg_id: &str) {
        self.send(ProtocolEvent::Thinking {
            text: String::new(),
            msg_id: msg_id.to_string(),
            subject: Some(subject.to_string()),
        });
    }

    fn emit_tool_call(&self, name: &str, _input: &str) {
        // v0.9.1.1 F1: this fallback is structurally redundant in TUI mode.
        // The engine ALWAYS emits `ToolRequest`/`ToolRunning` via
        // `ChannelEmitter` (with `call_id`) immediately before calling
        // `emit_tool_call` on every code path in the agent loop. Re-emitting
        // here as `ProtocolEvent::Info` produced a `Tool call: web` system
        // turn for every tool call — duplicating the structured tool card
        // and polluting the transcript.
        //
        // Drop to `tracing::debug!` so the signal stays for log triage but
        // never reaches the transcript.
        tracing::debug!(target: "wcore_cli::tui::channel_sink", "fallback emit_tool_call: {name}");
    }

    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        // v0.9.1.1 F1: same as `emit_tool_call` — `ToolResult` (with
        // `call_id`) fires via `ChannelEmitter` from
        // `orchestration::execute_tools` for every tool. Re-emitting the
        // raw `content` string here dumped the entire provider JSON
        // envelope (e.g. `[web success] {"web":[{"snippet":...}]}`)
        // verbatim into the transcript via `ProtocolEvent::Info`.
        //
        // Drop the transcript emission. The structured `ToolResult` event
        // is the authoritative path — `protocol_bridge::ToolResult`
        // updates the matching `ToolCardModel` and the workspace renders
        // a compact one-liner via `push_tool_card_lines`.
        let status = if is_error { "error" } else { "success" };
        tracing::debug!(
            target: "wcore_cli::tui::channel_sink",
            "fallback emit_tool_result: {name} {status} ({} bytes)",
            content.len()
        );
    }

    fn emit_stream_start(&self, msg_id: &str) {
        self.send(ProtocolEvent::StreamStart {
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
        self.send(ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            finish_reason,
            usage: Some(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: (cache_read_tokens > 0).then_some(cache_read_tokens),
                cache_write_tokens: (cache_creation_tokens > 0).then_some(cache_creation_tokens),
                active_window_percent: None,
            }),
            agent_run_id: None,
        });
    }

    fn emit_error(&self, msg: &str, retryable: bool) {
        self.send(ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "engine_error".to_string(),
                message: msg.to_string(),
                retryable,
            },
        });
    }

    fn emit_info(&self, msg: &str) {
        self.send(ProtocolEvent::Info {
            msg_id: String::new(),
            message: msg.to_string(),
        });
    }

    fn emit_trace(&self, msg_id: &str, trace_json: &serde_json::Value) {
        self.send(ProtocolEvent::TraceEvent {
            msg_id: msg_id.to_string(),
            trace: trace_json.clone(),
        });
    }

    /// The TUI wants streaming tool chunks — advertise the gate so the
    /// engine dispatcher plumbs a streaming sink.
    fn streaming_tools_advertised(&self) -> bool {
        true
    }

    fn emit_tool_chunk(&self, msg_id: &str, call_id: &str, tool_name: &str, chunk: &str) {
        self.send(ProtocolEvent::ToolChunk {
            msg_id: msg_id.to_string(),
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            chunk: chunk.to_string(),
        });
    }

    fn emit_sub_agent_event(
        &self,
        parent_call_id: &str,
        agent_name: &str,
        inner: &serde_json::Value,
    ) {
        self.send(ProtocolEvent::SubAgentEvent {
            parent_call_id: parent_call_id.to_string(),
            agent_name: agent_name.to_string(),
            inner: inner.clone(),
        });
    }

    fn emit_workflow_started(&self, workflow_id: &str, name: &str, node_count: usize) {
        self.send(ProtocolEvent::WorkflowStarted {
            workflow_id: workflow_id.to_string(),
            name: name.to_string(),
            node_count,
        });
    }

    fn emit_workflow_finished(&self, workflow_id: &str, succeeded: bool) {
        self.send(ProtocolEvent::WorkflowFinished {
            workflow_id: workflow_id.to_string(),
            succeeded,
        });
    }

    fn emit_session_cost(&self, session_id: &str, cost_payload: &serde_json::Value) {
        let total_cost_usd = cost_payload
            .get("total_cost_usd")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let per_turn = cost_payload
            .get("per_turn")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        self.send(ProtocolEvent::SessionCost {
            session_id: session_id.to_string(),
            total_cost_usd,
            per_turn,
        });
    }

    fn emit_provider_circuit_event(
        &self,
        primary: &str,
        fallback: Option<&str>,
        state: &str,
        error: Option<&str>,
    ) {
        self.send(ProtocolEvent::ProviderCircuitEvent {
            primary: primary.to_string(),
            fallback: fallback.map(String::from),
            state: state.to_string(),
            error: error.map(String::from),
        });
    }

    fn emit_approval_required(
        &self,
        call_id: &str,
        resume_token: &str,
        reason: &str,
        context: &str,
    ) {
        self.send(ProtocolEvent::ApprovalRequired {
            call_id: call_id.to_string(),
            resume_token: resume_token.to_string(),
            correlation_id: resume_token.to_string(),
            reason: reason.to_string(),
            context: context.to_string(),
            plan: None,
        });
    }

    fn emit_suspend(&self, reason: &str, resume_token: &str) {
        self.send(ProtocolEvent::Suspend {
            reason: reason.to_string(),
            resume_token: resume_token.to_string(),
        });
    }

    fn emit_approval_resume(&self, resume_token: &str, approved: bool) {
        self.send(ProtocolEvent::ApprovalResume {
            resume_token: resume_token.to_string(),
            approved,
        });
    }

    fn emit_budget_exceeded(&self, reason: &str, observed: &str, limit: &str) {
        self.send(ProtocolEvent::BudgetExceeded {
            reason: reason.to_string(),
            observed: observed.to_string(),
            limit: limit.to_string(),
        });
    }

    fn emit_tool_panicked(
        &self,
        msg_id: &str,
        call_id: &str,
        tool_name: &str,
        panic_message: &str,
    ) {
        self.send(ProtocolEvent::ToolPanicked {
            msg_id: msg_id.to_string(),
            call_id: call_id.to_string(),
            tool_name: tool_name.to_string(),
            panic_message: panic_message.to_string(),
        });
    }

    fn emit_plugin_registration_failed(
        &self,
        plugin_name: &str,
        surface: &str,
        error_kind: &str,
        message: &str,
    ) {
        self.send(ProtocolEvent::PluginRegistrationFailed {
            plugin_name: plugin_name.to_string(),
            surface: surface.to_string(),
            error_kind: error_kind.to_string(),
            message: message.to_string(),
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────
// TerminalGuard — a Drop-fired fallback terminal event for a turn task
// ─────────────────────────────────────────────────────────────────────────

/// A guard that guarantees the TUI sees a terminal event for a turn.
///
/// `TuiEngine::submit` synthesizes `StreamEnd` only on the `Ok`/`Err`
/// arms of `engine.run(...).await`. A *panic* inside `run` unwinds past
/// both arms; a `JoinHandle::abort()` drops the task at its current
/// `.await`. Either way, without this guard, no `StreamEnd` and no
/// `Error` ever reaches the bridge — `streaming_active` stays `true` and
/// the TUI shows the "working" spinner forever (AUDIT-D D1, the exact
/// 35-minute-hang symptom).
///
/// The guard is created at the top of the turn task and `disarm()`ed on
/// the normal `Ok`/`Err` paths after the real terminal events are sent.
/// If it is still armed when dropped — panic, abort, or any early
/// return — its `Drop` sends a fallback `Error` + `StreamEnd` so the TUI
/// recovers and tells the user the turn ended abnormally.
struct TerminalGuard {
    /// The event channel the fallback terminal events are sent on.
    tx: UnboundedSender<ProtocolEvent>,
    /// The turn's `msg_id`, carried on the fallback events.
    msg_id: String,
    /// `true` until `disarm()` is called. While armed, `Drop` fires the
    /// fallback events; once disarmed, `Drop` is a no-op.
    armed: bool,
}

impl TerminalGuard {
    /// Build an armed guard for a turn task.
    fn new(tx: UnboundedSender<ProtocolEvent>, msg_id: String) -> Self {
        Self {
            tx,
            msg_id,
            armed: true,
        }
    }

    /// Disarm the guard — the turn ended normally and already sent its
    /// real terminal events, so `Drop` must not send a second pair.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Still armed at drop: the turn task panicked or was aborted
        // before any terminal event went out. Emit a fallback pair so
        // the TUI clears `streaming_active` and shows the user that the
        // turn ended abnormally rather than hanging silently.
        let _ = self.tx.send(ProtocolEvent::Error {
            msg_id: Some(self.msg_id.clone()),
            error: ErrorInfo {
                code: "engine_panic".to_string(),
                message: "The turn ended unexpectedly (engine task panicked or was aborted). \
                          Please try again."
                    .to_string(),
                retryable: true,
            },
        });
        let _ = self.tx.send(ProtocolEvent::StreamEnd {
            msg_id: std::mem::take(&mut self.msg_id),
            finish_reason: FinishReason::Error,
            usage: None,
            agent_run_id: None,
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────
// TuiEngine — the render loop's handle on the live engine
// ─────────────────────────────────────────────────────────────────────────

/// One loaded skill in the [`EngineInventory`] snapshot.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// True when the skill can be run directly as `/name` (vs. only
    /// auto-activating by relevance). Drives the `/skills` "how to act" hint.
    pub user_invocable: bool,
}

/// One *attempted* MCP server in the [`EngineInventory`] snapshot — including
/// servers that failed or timed out at connect (they never become live but the
/// user still needs to see *why* in `/mcp` and `/doctor`).
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    /// Connect outcome captured at snapshot time (Ready / Failed / TimedOut).
    pub health: wcore_mcp::manager::McpServerHealth,
}

impl McpServerInfo {
    /// Whether the server is connected and serving tools.
    pub fn is_connected(&self) -> bool {
        matches!(
            self.health,
            wcore_mcp::manager::McpServerHealth::Ready { .. }
        )
    }
}

/// One registered hook in the [`EngineInventory`] snapshot.
#[derive(Debug, Clone)]
pub struct HookInfo {
    pub name: String,
    /// Which lifecycle point fires it: `pre-tool-use`, `post-tool-use`, `stop`.
    pub trigger: &'static str,
}

/// A synchronous, post-bootstrap snapshot of the engine's loaded extension
/// inventory — skills, MCP servers, and hooks.
///
/// The TUI's command dispatch is synchronous, but the live data lives
/// behind the engine's async `Mutex` (skills) or was consumed into the
/// bootstrap (hooks were in `Config`; MCP managers are separate handles).
/// Rather than lock the engine on the render thread, `/skills`, `/mcp`, and
/// `/hooks` read this immutable snapshot taken once at construction. Empty
/// in tests and headless constructors that never call [`TuiEngine::set_inventory`].
#[derive(Debug, Clone, Default)]
pub struct EngineInventory {
    pub skills: Vec<SkillInfo>,
    pub mcp_servers: Vec<McpServerInfo>,
    pub hooks: Vec<HookInfo>,
}

/// Format a one-paragraph `/repomap` summary from a freshly built [`RepoMap`]:
/// file + symbol totals, a language split, and the densest files. Pure so it
/// unit-tests without a filesystem scan.
fn format_repomap_summary(map: &wcore_repomap::RepoMap) -> String {
    use wcore_repomap::Language;
    let file_count = map.files.len();
    if file_count == 0 {
        return "Indexed the project but found no source files to map.".to_string();
    }
    let total_symbols: usize = map.files.iter().map(|f| f.symbols.len()).sum();
    let (mut rust, mut ts, mut js, mut other) = (0usize, 0usize, 0usize, 0usize);
    for f in &map.files {
        match f.language {
            Language::Rust => rust += 1,
            Language::TypeScript => ts += 1,
            Language::JavaScript => js += 1,
            Language::Other => other += 1,
        }
    }
    // Densest files first — "where the structure lives".
    let mut by_symbols: Vec<&wcore_repomap::FileSummary> =
        map.files.iter().filter(|f| !f.symbols.is_empty()).collect();
    by_symbols.sort_by_key(|f| std::cmp::Reverse(f.symbols.len()));

    let mut out = format!(
        "Indexed {file_count} files — {total_symbols} symbols.\n\
         Languages: {rust} Rust · {ts} TS · {js} JS · {other} other\n"
    );
    let take = by_symbols.len().min(5);
    if take > 0 {
        out.push_str(&format!("Densest files (top {take}):\n"));
        for f in by_symbols.iter().take(take) {
            out.push_str(&format!(
                "  · {}  ({} symbols)\n",
                f.path.display(),
                f.symbols.len()
            ));
        }
    }
    out.push_str("\nFresh scan — the agent indexes the same map on demand.");
    out
}

/// D024: classify a `/mcp add` target string into an [`McpServerConfig`].
///
/// An `http://` or `https://` target is a `streamable-http` server (the URL
/// goes on `url`). Anything else is treated as a stdio command line: the
/// first whitespace-separated token is the program, the rest are argv. An
/// empty target is an error so the caller can show honest guidance rather
/// than spawning an empty command. `deferred` defaults to `false` for a
/// runtime add — the user asked for it explicitly, so its tools are eager.
///
/// Pure so it unit-tests without a network round-trip or an engine.
fn mcp_config_from_target(target: &str) -> Result<wcore_config::config::McpServerConfig, String> {
    use wcore_config::config::{McpServerConfig, TransportType};
    let target = target.trim();
    if target.is_empty() {
        return Err("expected a URL or a command after the server name".to_string());
    }
    if target.starts_with("http://") || target.starts_with("https://") {
        return Ok(McpServerConfig {
            transport: TransportType::StreamableHttp,
            command: None,
            args: None,
            env: None,
            url: Some(target.to_string()),
            headers: None,
            deferred: Some(false),
            allow_local: false,
            only_for_assistant: None,
        });
    }
    let mut parts = target.split_whitespace();
    // `parts` came from a non-empty trimmed string, so the first token exists.
    let command = parts
        .next()
        .ok_or_else(|| "expected a command".to_string())?
        .to_string();
    let args: Vec<String> = parts.map(str::to_string).collect();
    Ok(McpServerConfig {
        transport: TransportType::Stdio,
        command: Some(command),
        args: (!args.is_empty()).then_some(args),
        env: None,
        url: None,
        headers: None,
        deferred: Some(false),
        allow_local: false,
        only_for_assistant: None,
    })
}

/// D008: render the `/model` picker listing from a LIVE [`ModelInfo`] set
/// (the result of `LlmProvider::list_models`), marking the active model `●`.
///
/// Pure so it unit-tests against a mock model set without a Router or a live
/// provider. `provider` is the alias key used only for the header label; an
/// empty `models` slice yields an honest "type an id directly" prompt rather
/// than a blank list. A model matches the active marker when its `id` OR its
/// `display` equals `active_model`, mirroring the static-catalog renderer so a
/// short-form active value (e.g. `opus`) still marks the resolved row.
fn render_model_info_list(
    provider: &str,
    models: &[wcore_providers::ModelInfo],
    active_model: &str,
) -> String {
    let label = if provider.is_empty() {
        "the active provider"
    } else {
        provider
    };
    if models.is_empty() {
        return format!(
            "{label} returned no models — type `/model <id>` to set one directly.\nCurrent model: {active_model}"
        );
    }
    let mut s = format!("Models for {label} (● = current) — type `/model <name>` to switch:\n");
    for m in models {
        let mark = if m.id == active_model || m.display == active_model {
            "●"
        } else {
            "○"
        };
        // Show the human label and, when it differs, the resolved id.
        if m.display == m.id {
            s.push_str(&format!("  {mark} {}\n", m.id));
        } else {
            s.push_str(&format!("  {mark} {}  ({})\n", m.display, m.id));
        }
    }
    s
}

/// The render loop's controller for the live `AgentEngine`.
///
/// `TuiEngine` owns the built engine behind a `tokio::Mutex` so a turn
/// can run on a spawned task while the render loop keeps drawing. It also
/// holds the shared `ToolApprovalManager` (the engine's approval round-
/// trip is resolved through it) and the channel sender both event halves
/// forward on.
pub struct TuiEngine {
    /// The built engine. Behind an async mutex so `run` can be driven on
    /// a background task while the render loop reads `App`.
    engine: Arc<tokio::sync::Mutex<wcore_agent::engine::AgentEngine>>,
    /// The shared approval manager. `approve` / `deny` resolve a pending
    /// tool call through this; it is the same instance the engine's
    /// `ApprovalChannel` awaits against.
    approval: Arc<ToolApprovalManager>,
    /// B2.5 — the engine's egress consent bridge, captured at construction so
    /// `approve`/`deny` can resolve an `egress:`-prefixed consent (which rides
    /// the `ApprovalBridge`, not the `ToolApprovalManager`) without locking the
    /// async engine mutex on the synchronous decision path.
    approval_bridge: Arc<wcore_agent::approval::ApprovalBridge>,
    /// The channel both the `ChannelEmitter` and `ChannelSink` forward
    /// on. Kept so a `StreamEnd` synthesized after `run` returns reaches
    /// the bridge (the engine emits `StreamStart` itself but never
    /// `StreamEnd` — that is the caller's job, as in `run_json_stream`).
    tx: UnboundedSender<ProtocolEvent>,
    /// The join handle of the turn currently running, if any. `Esc`
    /// cancellation aborts this task; dropping the task drops the
    /// `engine.run` future, which is how `run_json_stream` cancels too.
    active_turn: Option<JoinHandle<()>>,
    /// The cooperative-cancellation token for the in-flight turn.
    ///
    /// `cancel()` fires this BEFORE the hard `JoinHandle::abort()` so a
    /// running tool gets a chance to observe cancellation and clean up,
    /// rather than being torn down at an arbitrary `.await` point. Each
    /// `submit` installs a FRESH token (a fired token stays fired — a
    /// turn must not start pre-cancelled). The `abort()` remains the
    /// backstop for code that does not poll the token.
    ///
    /// `submit` installs this token as the engine's session root via
    /// `AgentEngine::set_cancel_token` before each `run()`, so firing it
    /// is observed cooperatively inside the engine loop and in every
    /// in-flight tool — not just by the `abort()` backstop.
    turn_cancel: CancellationToken,
    /// Immutable post-bootstrap snapshot of loaded skills / MCP servers /
    /// hooks, read by the synchronous `/skills` `/mcp` `/hooks` dispatch.
    /// Default-empty until [`Self::set_inventory`] runs in `run_tui_mode`.
    inventory: EngineInventory,
    /// The project root the session was launched in. `/repomap` scans this.
    /// Defaults to `.` until [`Self::set_repo_root`] runs in `run_tui_mode`.
    repo_root: PathBuf,
    /// `(session directory, max_sessions)` for the `/resume` listing, captured
    /// from config in `run_tui_mode`. `None` until [`Self::set_session_store`]
    /// runs; `/resume` then reports no history.
    session_store: Option<(PathBuf, usize)>,
    /// The most recent shell-tool output, staged by `send_message` just
    /// before a submit so the spawned turn task can resolve an `@output`
    /// reference. Consumed (`take`n) per submit; `None` between turns.
    pending_at_ref_output: Option<String>,
    /// F37: monotonic generation counter for deferred rebinds. When the
    /// engine lock is contended, `rebind_with_config` spawns a task that
    /// applies the new binding once the in-flight turn releases the lock.
    /// Two binding-changing commands during one turn each spawn such a task,
    /// and tokio does not order their lock acquisitions — so the
    /// later-issued config could land first, leaving the engine on a stale
    /// binding. Each deferred rebind bumps this counter and captures its own
    /// generation; after acquiring the lock the task re-checks the counter
    /// and skips its swap if a newer rebind has superseded it, so only the
    /// most recently requested config is ever applied.
    rebind_generation: Arc<AtomicU64>,
}

impl TuiEngine {
    /// A clone of the protocol event channel sender, so a deferred async action
    /// (e.g. the /auth OAuth round-trip) can post an Info turn back to the
    /// render loop on completion.
    pub fn events(&self) -> UnboundedSender<ProtocolEvent> {
        self.tx.clone()
    }

    /// Build a `TuiEngine` from an already-constructed engine, the shared
    /// approval manager, and the event channel sender.
    pub fn new(
        engine: wcore_agent::engine::AgentEngine,
        approval: Arc<ToolApprovalManager>,
        tx: UnboundedSender<ProtocolEvent>,
    ) -> Self {
        // Capture the egress consent bridge before the engine is moved behind
        // the async mutex, so the sync `approve`/`deny` path can resolve it.
        let approval_bridge = engine.approval_bridge().clone();
        Self {
            engine: Arc::new(tokio::sync::Mutex::new(engine)),
            approval,
            approval_bridge,
            tx,
            active_turn: None,
            // A starting token; `submit` replaces it with a fresh one
            // per turn so a prior cancel never leaks into a new turn.
            turn_cancel: CancellationToken::new(),
            inventory: EngineInventory::default(),
            repo_root: PathBuf::from("."),
            session_store: None,
            pending_at_ref_output: None,
            rebind_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Install the post-bootstrap extension inventory snapshot. Called once
    /// in `run_tui_mode` after the engine + MCP managers are built, before
    /// the render loop starts. Read by `/skills` `/mcp` `/hooks`.
    pub fn set_inventory(&mut self, inventory: EngineInventory) {
        self.inventory = inventory;
    }

    /// Set the project root scanned by `/repomap`. Called once in
    /// `run_tui_mode` with the session's launch cwd.
    pub fn set_repo_root(&mut self, root: PathBuf) {
        self.repo_root = root;
    }

    /// The project root the session launched in. `/rewind` checks it for a
    /// `.git` dir to decide what restore guidance to give.
    pub fn repo_root(&self) -> &std::path::Path {
        &self.repo_root
    }

    /// Set the on-disk session store (`directory`, `max_sessions`) the
    /// `/resume` listing reads. Called once in `run_tui_mode` from config.
    pub fn set_session_store(&mut self, directory: PathBuf, max_sessions: usize) {
        self.session_store = Some((directory, max_sessions));
    }

    /// Stage the most recent shell-tool output for the next submit, so an
    /// `@output` reference in that prompt resolves to it. `send_message`
    /// computes this from the live transcript (the last Bash tool card) right
    /// before calling [`Self::submit`], which consumes it.
    pub fn set_pending_at_ref_output(&mut self, output: Option<String>) {
        self.pending_at_ref_output = output;
    }

    /// List saved sessions newest-first for `/resume`. A fast synchronous
    /// index read (no engine lock, no async) — the dispatch path renders it
    /// inline. Empty when no store is configured or the index is unreadable.
    pub fn list_sessions(&self) -> Vec<wcore_agent::session::SessionMeta> {
        match &self.session_store {
            Some((dir, max)) => wcore_agent::session::SessionManager::new(dir.clone(), *max)
                .list()
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Run the `/repomap` symbol scan. No persistent index exists (the
    /// `RepoMapTool` is LLM-invoked on demand), so this builds a fresh map of
    /// the project root and emits an `Info` summary — file/symbol counts, a
    /// language split, and the densest files. The scan is filesystem- and
    /// CPU-bound, so it runs on the blocking pool and reports back through the
    /// event channel rather than stalling the render thread. A `push_system`
    /// "scanning…" line at the call site gives instant feedback.
    pub fn index_repomap(&self) {
        let root = self.repo_root.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let message =
                tokio::task::spawn_blocking(move || match wcore_repomap::RepoMap::build(&root) {
                    Ok(map) => format_repomap_summary(&map),
                    Err(e) => format!("Couldn't index the project: {e}"),
                })
                .await
                .unwrap_or_else(|_| "Indexing task failed to complete.".to_string());
            let _ = tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message,
            });
        });
    }

    /// Read-only handle on the loaded extension inventory (skills / MCP
    /// servers / hooks) for the synchronous slash-command listings.
    pub fn inventory(&self) -> &EngineInventory {
        &self.inventory
    }

    /// True while a turn is running (the spawned `run` task has not
    /// finished). Used by the render loop to gate a second submit.
    pub fn is_busy(&self) -> bool {
        self.active_turn
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    /// Submit a prompt: spawn `engine.run(prompt, msg_id)` on a
    /// background task. Streaming events reach the TUI live through the
    /// `ChannelSink`/`ChannelEmitter` already installed on the engine;
    /// when `run` returns this task synthesizes the `StreamEnd` the
    /// engine does not emit itself (matching `run_json_stream`).
    ///
    /// A submit while a turn is already running is ignored — the caller
    /// (`SurfaceAction::SendMessage` routing) gates on [`is_busy`].
    pub fn submit(&mut self, prompt: String, msg_id: String) {
        if self.is_busy() {
            return;
        }
        // Install a FRESH cancellation token for this turn. A token, once
        // fired, stays fired — reusing the prior turn's token would start
        // this turn already-cancelled. The token is held on `self` so
        // `cancel()` can fire it; it is also moved into the task so a
        // future engine `run` that accepts a token can observe it.
        self.turn_cancel = CancellationToken::new();
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        let turn_cancel = self.turn_cancel.clone();
        // Captured for send-time `@session` / `@output` resolution in the task.
        let session_store = self.session_store.clone();
        let last_output = self.pending_at_ref_output.take();
        let handle = tokio::spawn(async move {
            // A terminal-event guard. If the body below `panic!`s — or is
            // dropped by `JoinHandle::abort()` — before a terminal event
            // is sent, the guard's `Drop` synthesizes an `Error` + a
            // `StreamEnd` so the TUI never strands the spinner in the
            // "working" state with nothing shown (AUDIT-D D1). On the
            // normal `Ok`/`Err` paths the guard is `disarm()`ed after the
            // real terminal events go out, so it does nothing on drop.
            let mut term = TerminalGuard::new(tx.clone(), msg_id.clone());

            // Wave 2: resolve `@file`/`@dir`/`@diff`/`@symbol`/`@session`
            // references into inline context before the engine sees the prompt.
            // This runs off the UI thread (we are already on the spawned turn
            // task) so `@diff`'s git subprocess and the `@symbol`/`@session`
            // index reads never block rendering. The user's transcript keeps
            // the literal `@`-tokens (appended in `send_message`); only the
            // engine-facing prompt carries the resolved bodies. A prompt with
            // no resolvable reference is returned unchanged.
            let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let send_ctx = crate::tui::commands::at_refs::SendCtx {
                session_store: session_store.clone(),
                last_output,
            };
            let prompt =
                crate::tui::commands::at_refs::resolve_message_with(&prompt, &root, &send_ctx)
                    .await;

            let mut guard = engine.lock().await;
            // Install this turn's cancellation token as the engine's session
            // root before `run()`. The engine loop checks it between turns
            // and threads a child into every `GraphContext` / `ToolContext`,
            // so `cancel()` firing `turn_cancel` reaches an in-flight tool
            // cooperatively — the `abort()` in `cancel()` is now only a
            // backstop, not the sole stop mechanism.
            guard.set_cancel_token(turn_cancel.clone());
            match guard.run(&prompt, &msg_id).await {
                Ok(result) => {
                    let _ = tx.send(ProtocolEvent::StreamEnd {
                        msg_id: msg_id.clone(),
                        finish_reason: result.finish_reason,
                        usage: Some(Usage {
                            input_tokens: result.usage.input_tokens,
                            output_tokens: result.usage.output_tokens,
                            cache_read_tokens: (result.usage.cache_read_tokens > 0)
                                .then_some(result.usage.cache_read_tokens),
                            cache_write_tokens: (result.usage.cache_creation_tokens > 0)
                                .then_some(result.usage.cache_creation_tokens),
                            active_window_percent: result.active_window_percent,
                        }),
                        agent_run_id: result.agent_run_id.clone(),
                    });
                    term.disarm();
                }
                Err(e) => {
                    let _ = tx.send(ProtocolEvent::Error {
                        msg_id: Some(msg_id.clone()),
                        error: ErrorInfo {
                            code: "engine_error".to_string(),
                            message: e.to_string(),
                            retryable: false,
                        },
                    });
                    let _ = tx.send(ProtocolEvent::StreamEnd {
                        msg_id: msg_id.clone(),
                        finish_reason: FinishReason::Error,
                        usage: None,
                        agent_run_id: None,
                    });
                    term.disarm();
                }
            }
        });
        self.active_turn = Some(handle);
    }

    /// Cancel the in-flight turn (`Esc`).
    ///
    /// Cancellation is two-stage (AUDIT-D + cooperative-cancel wiring):
    ///  1. Fire the per-turn [`CancellationToken`] FIRST, so a running
    ///     tool that polls it gets a *cooperative* cancel and can clean
    ///     up (close a subprocess, flush a file) rather than being torn
    ///     down at an arbitrary `.await`.
    ///  2. Then `JoinHandle::abort()` as the backstop — it stops the task
    ///     even if nothing on the turn path observes the token. Aborting
    ///     drops the `engine.run` future, the same mechanism the
    ///     json-stream loop uses.
    ///
    /// A synthetic `Info` + `StreamEnd` keeps the TUI's streaming state
    /// from getting stuck. (The turn task's own [`TerminalGuard`] would
    /// also fire a fallback terminal event on the abort-drop; the
    /// explicit pair here gives the user the friendlier "cancelled"
    /// wording immediately rather than the guard's generic message.)
    pub fn cancel(&mut self) {
        if let Some(handle) = self.active_turn.take() {
            // Stage 1: cooperative cancel — fire the token before the
            // hard abort so a tool that polls it can unwind cleanly.
            self.turn_cancel.cancel();
            // Stage 2: hard abort — the backstop for code that does not
            // observe the token.
            handle.abort();
            let _ = self.tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message: "Turn cancelled.".to_string(),
            });
            let _ = self.tx.send(ProtocolEvent::StreamEnd {
                msg_id: String::new(),
                finish_reason: FinishReason::Error,
                usage: None,
                agent_run_id: None,
            });
        }
    }

    /// Approve a pending tool call (`SurfaceAction::Approve`). Resolves
    /// the engine's `ApprovalChannel` oneshot through the shared manager,
    /// exactly as `run_json_stream`'s `ToolApprove` arm does.
    ///
    /// v0.9.3 W8 B1: `answer` is forwarded into the resolved
    /// `ToolApprovalResult::Approved { answer }` so AskUserQuestion's
    /// interactive Enter handler can ferry the user's choice to the
    /// orchestration synthesis arm. Non-AskUser callers pass `None`,
    /// preserving the v0.9.2 behaviour.
    pub fn approve(
        &self,
        call_id: &str,
        scope: wcore_protocol::commands::ApprovalScope,
        answer: Option<String>,
    ) {
        // B2.5 — an egress consent (`egress:` call_id) rides the ApprovalBridge,
        // not the ToolApprovalManager. `a` (Always) persists the domain; `y`
        // (Once) allows just this reach. The scope crosses to the policy via
        // the resolved outcome's `modifications`.
        if call_id.starts_with("egress:") {
            let always = matches!(scope, wcore_protocol::commands::ApprovalScope::Always);
            self.resolve_egress(call_id, true, always);
            return;
        }
        // Stage 4b — a Crucible council (`crucible:` call_id) also rides the
        // ApprovalBridge, not the ToolApprovalManager. The in-process TUI offers
        // a binary decision; approve → run the council (the bridge default maps
        // approved + no modifications → `CrucibleDecision::Approve`). Without
        // this the council task would hang until the 24h `CRUCIBLE_APPROVAL_TTL`.
        if call_id.starts_with("crucible:") {
            self.resolve_crucible(call_id, true);
            return;
        }
        // `ToolApprovalManager::approve` honours `ApprovalScope::Always`
        // by registering the tool's category for auto-approval; the
        // `answer` payload threads through to the resolved oneshot so
        // orchestration's synth arm sees it.
        self.approval.approve(call_id, scope, answer);
    }

    /// Deny a pending tool call (`SurfaceAction::Deny`).
    pub fn deny(&self, call_id: &str, reason: String) {
        if call_id.starts_with("egress:") {
            self.resolve_egress(call_id, false, false);
            return;
        }
        // Stage 4b — deny a Crucible council on the bridge (no spend). A
        // not-approved outcome maps to `CrucibleDecision::Cancel` in
        // `BridgeApprover::approve`.
        if call_id.starts_with("crucible:") {
            self.resolve_crucible(call_id, false);
            return;
        }
        self.approval
            .resolve(call_id, ToolApprovalResult::Denied { reason });
    }

    /// B2.5 — resolve an egress consent on the `ApprovalBridge`. `call_id` is
    /// the bridge correlation id (the doorbell registers it via
    /// `request_with_id`). Spawned because `bridge.resolve` is async while the
    /// render-loop decision path is sync; we run under the tokio runtime so the
    /// spawn lands on it.
    fn resolve_egress(&self, call_id: &str, approved: bool, always: bool) {
        let bridge = self.approval_bridge.clone();
        let call_id = call_id.to_string();
        let modifications =
            (approved && always).then(|| serde_json::json!({ "egress_scope": "always" }));
        tokio::spawn(async move {
            // GHSA-8r7g: a TUI keypress holds the PUBLIC call_id, so resolve by
            // correlation (the secret resume_token is only on the wire path).
            bridge
                .resolve_by_correlation(
                    &call_id,
                    wcore_agent::approval::ApprovalOutcome {
                        approved,
                        modifications,
                    },
                )
                .await;
        });
    }

    /// Stage 4b — resolve a Crucible council approval on the `ApprovalBridge`.
    /// `call_id` (the `crucible:`-prefixed id) is the bridge correlation key:
    /// `BridgeApprover` registers it via `request_with_id_and_ttl`, and the
    /// approval dedupe makes that call_id the only token any host can resolve.
    /// The in-process TUI decision is binary: `approved` true → the council runs
    /// (`BridgeApprover` maps approved + no modifications → `CrucibleDecision::
    /// Approve`); false → no spend (`Cancel`). Spawned because `bridge.resolve`
    /// is async while the render-loop decision path is sync.
    fn resolve_crucible(&self, call_id: &str, approved: bool) {
        let bridge = self.approval_bridge.clone();
        let call_id = call_id.to_string();
        tokio::spawn(async move {
            // GHSA-8r7g: resolve by the PUBLIC correlation id (TUI keypress).
            bridge
                .resolve_by_correlation(
                    &call_id,
                    wcore_agent::approval::ApprovalOutcome {
                        approved,
                        modifications: None,
                    },
                )
                .await;
        });
    }

    /// Apply a session-mode change (`SurfaceAction::SetMode`). The
    /// approval manager auto-approves tool categories per the mode, so
    /// the engine's approval gate honours the change immediately.
    pub fn set_mode(&self, mode: wcore_protocol::commands::SessionMode) {
        self.approval.set_mode(mode);
    }

    /// Switch the engine's active model (the `/model` command). The engine
    /// lives behind an async mutex, so — like `toggle_voice` — the swap is
    /// applied in a spawned task; the TUI updates its own status-bar view
    /// synchronously at the call site. Takes effect on the next turn (an
    /// in-flight turn finishes on the old model). Best-effort: the model is
    /// validated/resolved by the caller before this is invoked.
    pub fn set_model(&self, model: String) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.lock().await.set_model(model);
        });
    }

    /// D008: fetch the LIVE model library for the active provider and render
    /// it back into the transcript as an `Info` event.
    ///
    /// The slash dispatch is synchronous, but `LlmProvider::list_models` is
    /// async and may hit a network endpoint (Anthropic / OpenAI-compatible),
    /// so — like [`index_repomap`](Self::index_repomap) and
    /// [`compact`](Self::compact) — the work runs on a spawned task and the
    /// result arrives later as an `Info` turn. The caller pushes a synchronous
    /// "Fetching…" line for instant feedback.
    ///
    /// The provider `Arc` is cloned out from under the engine lock and the
    /// lock is dropped BEFORE the (potentially slow) `list_models().await`, so
    /// a network round-trip never stalls an in-flight turn waiting on the
    /// engine mutex. On any provider error the trait's own alias fallback has
    /// already kicked in (providers must not error from `list_models`), but we
    /// add a belt-and-braces fallback to the static alias catalog here too so
    /// the picker never renders empty.
    pub fn list_models(&self) {
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            // Snapshot the provider handle + active model under the lock, then
            // release it before the async fetch.
            let (provider, active_model) = {
                let guard = engine.lock().await;
                (guard.provider().clone(), guard.model().to_string())
            };
            let alias_key = provider.alias_key().to_string();
            let models = match provider.list_models().await {
                Ok(models) if !models.is_empty() => models,
                // Empty or errored — fall back to the static alias catalog so
                // the picker always shows something actionable.
                _ => wcore_providers::alias_models(&alias_key),
            };
            let message = render_model_info_list(&alias_key, &models, &active_model);
            let _ = tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message,
            });
        });
    }

    /// D014: the engine's LIVE active model, read without blocking.
    ///
    /// Used by the router to detect when a skill/hook `switch_model` has moved
    /// the live model off the user's explicit `/model` pick. `try_lock` keeps
    /// this off the render thread's critical path — a `None` (lock held by an
    /// in-flight turn) simply defers the divergence check to the next frame.
    pub fn live_model(&self) -> Option<String> {
        self.engine
            .try_lock()
            .ok()
            .map(|guard| guard.model().to_string())
    }

    /// D005: enter plan mode on the LIVE engine. Flips the per-turn tool gate
    /// (`AgentEngine::enter_plan_mode`) so every mutating tool is filtered out
    /// of the turn while the user is in the read-only plan-review posture - not
    /// just a surface label. Mirrors `set_model`'s spawn-then-lock shape (the
    /// engine mutex is async; this call site is sync). Idempotent.
    pub fn enter_plan_mode(&self) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.lock().await.enter_plan_mode();
        });
    }

    /// D005/D006: exit plan mode on the LIVE engine. Clears the plan gate and
    /// restores the pre-plan allow-list (`AgentEngine::exit_plan_mode`). Called
    /// when the user approves a plan ("Approve & run" -> `/exit-plan-mode`) or
    /// discards it, so mutating tools run again. Idempotent.
    pub fn exit_plan_mode(&self) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.lock().await.exit_plan_mode();
        });
    }

    /// D001 / D007 / D016 keystone: re-resolve the on-disk config and
    /// rebind the LIVE engine to it without a restart.
    ///
    /// The engine is built ONCE at boot from the resolved `Config`. After
    /// onboarding writes a provider + API key + model to disk, or a
    /// `/config` Tier-1 save changes the approval posture, the running
    /// engine still carries the boot defaults — so the user's next prompt
    /// runs against the wrong provider (or a keyless boot `Config::default()`
    /// that can never authenticate). This is the seam that closes that gap:
    ///
    ///  1. Re-resolve `Config` from disk (global ← project ← CLI defaults),
    ///     the same `Config::resolve` path `main.rs` boot uses.
    ///  2. Rebuild the provider via
    ///     `wcore_agent::bootstrap::create_provider_with_oauth` (the OAuth-aware
    ///     analogue of `wcore_providers::create_provider`) — the API key /
    ///     OAuth bearer source is baked into the provider `Arc` at
    ///     construction, so a fresh build is the only way the newly entered
    ///     credential reaches the wire.
    ///  3. Lock the engine and swap provider + compat + model atomically
    ///     (`rebind_provider`), replace the system prompt with the resolved
    ///     prompt + the `[default] user` display name (`set_system_prompt`,
    ///     D016), and push the resolved approval posture to the shared
    ///     manager (`set_mode`, D007).
    ///
    /// M1/M2 honesty: steps 1 + 2 (`Config::resolve` +
    /// `create_provider_with_oauth`)
    /// run SYNCHRONOUSLY here, BEFORE any task is spawned — they take no
    /// `.await`. On a resolve failure (a malformed config the user just
    /// wrote) the engine is left untouched (the key is never dropped) and
    /// `None` is returned, so the caller can show "live apply skipped"
    /// instead of a false "now live". Only step 3 — the engine
    /// `lock().await` + the provider/model/prompt swap + `approval.set_mode`
    /// — applies SYNCHRONOUSLY when the engine lock is uncontended, and is
    /// deferred to a spawned task only when a turn currently holds the lock
    /// (so an in-flight turn finishes on the old binding and the next turn
    /// picks up the new one). A session launched with runtime `--force`
    /// (`force_pinned`) keeps its Force posture across rebinds — `--force` is
    /// not persisted to disk, so re-resolving disk must not downgrade it.
    ///
    /// Returns `Some(RebindApplied)` when the resolve + provider build
    /// succeeded and the live apply was scheduled — carrying the resolved
    /// approval [`SessionMode`](wcore_protocol::commands::SessionMode) (so
    /// the router can sync the status-bar badge) and a fresh [`ConfigView`]
    /// (so the router can mirror the saved Tier-1 fields onto `App::config`
    /// for the next `/config` re-entry). Returns `None` on resolve failure.
    pub fn rebind(&self, force_pinned: bool) -> Option<RebindApplied> {
        // M1/M2: resolve + build the provider SYNCHRONOUSLY so a failure is
        // known before the caller renders any "now live" microcopy. No
        // `.await` is taken on this path.
        let config = match wcore_config::config::Config::resolve(
            &wcore_config::config::CliArgs::default(),
        ) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_cli::tui::engine_bridge",
                    "rebind skipped: config did not resolve ({e:#})"
                );
                return None;
            }
        };
        self.rebind_with_config(config, force_pinned)
    }

    /// D022: live-swap the active provider (the `/provider <name>` command).
    ///
    /// Re-resolves the on-disk config exactly as [`rebind`](Self::rebind)
    /// does, then OVERRIDES `[default] provider` with `name` before the
    /// provider is rebuilt — so the running engine swaps to the named
    /// provider without a restart. The provider's compat row and default
    /// model are re-derived from the override (a provider switch reloads
    /// compat by definition), so a turn never observes a provider that
    /// disagrees with its compat. Returns `None` (live apply skipped) when
    /// the base config does not resolve. `name` is assumed validated by the
    /// caller against the known-providers catalog.
    ///
    /// [`rebind`]: Self::rebind
    pub fn rebind_with_provider(&self, name: &str, force_pinned: bool) -> Option<RebindApplied> {
        // Override the provider through the SAME `Config::resolve` path the
        // boot + `--provider <id>` flag use, so compat, base_url, the env-var
        // API key, and the default model are all re-derived consistently for
        // the new provider — never reach into the resolved config to swap the
        // provider slug alone (that would leave compat/model from the OLD
        // provider, an AGENTS.md "no hardcoded provider quirks" violation).
        // When the new provider has a sensible default model, stamp it via the
        // model override too, so a switch doesn't strand the engine on the
        // prior provider's model id.
        let default_model = wcore_config::config::default_model_for_slug(name);
        let cli = wcore_config::config::CliArgs {
            provider: Some(name.to_string()),
            model: (!default_model.is_empty()).then(|| default_model.to_string()),
            ..Default::default()
        };
        let config = match wcore_config::config::Config::resolve(&cli) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_cli::tui::engine_bridge",
                    "/provider rebind skipped: config did not resolve ({e:#})"
                );
                return None;
            }
        };
        self.rebind_with_config(config, force_pinned)
    }

    /// D021: live-load a saved profile (the `/profile <name>` command).
    ///
    /// Re-resolves the on-disk config WITH the named profile overlaid —
    /// the same `--profile` overlay `Config::resolve` applies at boot — then
    /// rebinds the running engine to it. So `/profile fast` switches
    /// provider + model + overrides in-session, no restart. Returns `None`
    /// (live apply skipped) when the config or the named profile does not
    /// resolve (e.g. an unknown profile name), leaving the engine untouched.
    pub fn rebind_with_profile(&self, name: &str, force_pinned: bool) -> Option<RebindApplied> {
        let cli = wcore_config::config::CliArgs {
            profile: Some(name.to_string()),
            ..Default::default()
        };
        let config = match wcore_config::config::Config::resolve(&cli) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_cli::tui::engine_bridge",
                    "/profile rebind skipped: config/profile did not resolve ({e:#})"
                );
                return None;
            }
        };
        self.rebind_with_config(config, force_pinned)
    }

    /// Shared core of the rebind family: given an already-resolved `Config`,
    /// rebuild the provider and swap provider + compat + model + system
    /// prompt + approval posture onto the live engine. Factored out so
    /// [`rebind`](Self::rebind) (disk re-resolve), [`rebind_with_provider`]
    /// (provider override), and [`rebind_with_profile`] (profile overlay)
    /// all share one engine-swap path.
    ///
    /// [`rebind_with_provider`]: Self::rebind_with_provider
    /// [`rebind_with_profile`]: Self::rebind_with_profile
    fn rebind_with_config(
        &self,
        config: wcore_config::config::Config,
        force_pinned: bool,
    ) -> Option<RebindApplied> {
        // Rebuild the provider so the freshly entered API key reaches the
        // wire (the key is baked into the provider Arc). Synchronous.
        //
        // Routed through the OAuth-aware builder rather than
        // `wcore_providers::create_provider` so a runtime swap to an
        // OAuth-backed provider (`openai-chatgpt`, and any future
        // `*-oauth` provider) constructs its bearer-source-backed provider
        // instead of hitting the `create_native_provider` panic. For every
        // non-OAuth provider this returns exactly what `create_provider`
        // did (a `ResilientProvider` with an empty fallback chain + NoOp
        // reporter), so existing behaviour is unchanged. A build failure
        // (e.g. an unreadable OAuth token store) is treated like a resolve
        // failure: leave the engine on its current binding and report
        // "live apply skipped" rather than dropping the running provider.
        let provider = match wcore_agent::bootstrap::create_provider_with_oauth(&config) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_cli::tui::engine_bridge",
                    "rebind skipped: provider build failed ({e:#})"
                );
                return None;
            }
        };
        let compat = config.compat.clone();
        let model = config.model.clone();

        // D016 / Wave-6 #5: the rebind OVERLAY — the onboarded display-name
        // block ALONE. `AgentEngine::set_system_prompt` re-prepends this overlay
        // onto the retained boot base (Constitution / persona / skills index +
        // the resolved config prompt that `build_system_prompt` already folded
        // in at bootstrap). Passing `None` for the base here is deliberate: the
        // resolved `config.system_prompt` is the RAW `[default] system_prompt`,
        // which is already embedded in the retained base — re-supplying it would
        // duplicate it AND would still omit the framework fragments. The name
        // block is the only thing these verbs (/provider, /profile, /config)
        // change about the prompt, so the overlay carries just that.
        let system_prompt = build_rebind_system_prompt(
            None,
            wcore_config::config::global_user_display_name().as_deref(),
        );

        // D007: the resolved approval posture — applied to the shared
        // manager in the task below AND returned so the router can sync the
        // status-bar badge (`app.mode`) to match the live gate.
        let session_mode = approval_mode_to_session(config.approval_mode);
        // M4: a fresh ConfigView the router mirrors onto `App::config` so the
        // next `/config` on_enter re-seeds from the just-saved truth.
        let config_view = super::config_view_from(&config);

        let engine = self.engine.clone();
        // N3: a second Arc handle for the deferred path — the match below holds
        // a borrow of `engine` (via the `try_lock` result temporary) across both
        // arms, so the spawned task must move a distinct clone, not `engine`.
        let engine_for_task = engine.clone();
        let approval = self.approval.clone();
        let task_mode = approval_mode_to_session(config.approval_mode);
        // N1: a session launched with runtime --force is pinned to Force, which
        // is NOT persisted to disk; blindly re-resolving the disk approval would
        // silently downgrade the live gate (and flip the badge) off Force. When
        // the session is force-pinned, preserve the live posture by skipping the
        // approval set_mode below.
        let apply_mode = !force_pinned;
        // F37: claim a generation for THIS rebind request. A later rebind
        // issued while a turn still holds the engine lock will claim a higher
        // generation; the deferred task below checks this before applying so a
        // superseded (older) config never lands after a newer one.
        let my_generation = self.rebind_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let rebind_generation = self.rebind_generation.clone();
        // N3: swap synchronously when the engine lock is uncontended (the common
        // case right after onboarding completion or a /config save, when no turn
        // is in flight) so an immediate first submit cannot race onto the old
        // keyless binding. Only when a turn currently holds the lock do we defer
        // the swap to a task; the in-flight turn then finishes on the old
        // binding and the next turn picks up the new one.
        match engine.try_lock() {
            Ok(mut guard) => {
                if apply_mode {
                    approval.set_mode(task_mode);
                }
                guard.rebind_provider(provider, compat, model);
                guard.set_system_prompt(system_prompt);
            }
            Err(_) => {
                tokio::spawn(async move {
                    // Wave-6 #16: acquire the engine lock FIRST, then apply the
                    // approval posture together with the provider/model/prompt
                    // swap — mirroring the synchronous arm above. The approval
                    // manager is a separate Arc<ToolApprovalManager> NOT behind
                    // the engine lock, read live by the running turn for tool
                    // gating. Flipping it before the lock resolves would change
                    // the in-flight turn's consent posture while it still runs
                    // on the OLD provider/model — a split binding (e.g. a save
                    // that loosens to auto-approve could let the running turn
                    // auto-execute tools the user consented to only for the NEXT
                    // turn). Deferring set_mode until the lock is held keeps the
                    // in-flight turn wholly on the old binding; the new posture
                    // lands atomically with the swap, for the next turn.
                    let mut guard = engine_for_task.lock().await;
                    // F37: a newer rebind may have been issued (and may have
                    // already applied) while this task waited for the lock.
                    // Applying this now-stale config would clobber the newer
                    // binding. Skip if superseded — only the latest desired
                    // config wins.
                    if rebind_generation.load(Ordering::SeqCst) != my_generation {
                        return;
                    }
                    if apply_mode {
                        approval.set_mode(task_mode);
                    }
                    guard.rebind_provider(provider, compat, model);
                    guard.set_system_prompt(system_prompt);
                });
            }
        }

        Some(RebindApplied {
            session_mode,
            config_view,
        })
    }

    /// Force a context compaction now (the `/compact` command). Runs in a
    /// spawned task (the engine is behind an async mutex) and emits a single
    /// `Info` event with the freed-tokens summary so the user gets real
    /// feedback instead of a silent no-op.
    pub fn compact(&self) {
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            // Deterministic micro-compaction: clear old tool-result bodies in
            // place. Preserves the conversation; no canned-summary truncation.
            let result = engine.lock().await.compact_now();
            let message = if result.cleared_count == 0 {
                "Nothing to compact yet — no old tool results to clear.".to_string()
            } else {
                format!(
                    "Context compacted: cleared {} old tool result(s) (~{} tokens freed).",
                    result.cleared_count, result.estimated_tokens_freed
                )
            };
            let _ = tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message,
            });
        });
    }

    /// Fire the config `[[hooks]] stop` + plugin `Stop` hooks on TUI exit.
    /// The REPL and json-stream surfaces already call `engine.run_stop_hooks()`
    /// on their way out; the TUI moved the engine into this controller, so it
    /// could not — leaving Stop hooks silently un-fired for the whole TUI
    /// surface. `fire_on_session_end` (dream/curator/auto-memorize) already
    /// runs via `engine.run()` exit and is NOT duplicated here.
    pub async fn run_stop_hooks(&self) {
        self.engine.lock().await.run_stop_hooks().await;
    }

    /// Drop the engine's conversation history (the `/new` command). The UI
    /// turns are cleared synchronously by the caller; this clears the engine's
    /// message buffer so the next turn genuinely starts fresh instead of
    /// silently carrying the prior context. Runs in a spawned task because the
    /// engine is behind an async mutex; no user-facing event — the caller
    /// already pushed the "Started a new conversation." confirmation.
    pub fn clear_conversation(&self) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.lock().await.clear_conversation();
        });
    }

    /// D024: add an MCP server LIVE (the `/mcp add <name> <url-or-cmd>`
    /// command). The engine already grows its tool set at runtime — the
    /// json-stream host path does exactly this via `AddMcpServer`
    /// (`connect_all` for the one server, then `register_single_server_tools`
    /// against `engine.registry_mut()`). This is the in-TUI mirror of that
    /// path, so `/mcp add` is a real action rather than a "edit wcore.toml;
    /// restart" printout.
    ///
    /// `target` is classified by [`mcp_config_from_target`]: an `http(s)://`
    /// target becomes a `streamable-http` server; anything else is a stdio
    /// command line (the first token is the program, the rest are argv). The
    /// connect + registration runs on a spawned task (the engine mutex is
    /// async, and the MCP handshake hits the network / spawns a process) and
    /// reports back through the bridge channel as an `Info`/`Error` turn.
    ///
    /// HONEST failure: registering the discovered tools needs
    /// `Arc::get_mut` on the shared tool registry, which only succeeds when
    /// no clone is outstanding. While a turn is in flight the registry is
    /// Arc-shared, so the add is reported as "busy — try again when idle"
    /// rather than silently dropped. This mirrors the json-stream path's own
    /// `registry busy` guard.
    pub fn add_mcp_server(&self, name: String, target: String) {
        let config = match mcp_config_from_target(&target) {
            Ok(c) => c,
            Err(e) => {
                let _ = self.tx.send(ProtocolEvent::Error {
                    msg_id: None,
                    error: ErrorInfo {
                        code: "mcp_add".to_string(),
                        message: format!("Can't add MCP server '{name}': {e}"),
                        retryable: false,
                    },
                });
                return;
            }
        };
        tokio::spawn(Self::connect_and_register_mcp(
            self.engine.clone(),
            self.tx.clone(),
            name,
            config,
        ));
    }

    /// Slice 3, Piece 2 — connect a discovered Forge MCP server after a
    /// successful loopback grant. The bearer `token` is stored in the
    /// credentials store (NEVER `config.toml`); the persisted
    /// `[mcp.servers.<name>]` entry carries only a `${cred:KEY}` reference in
    /// its `Authorization` header, which is resolved back to the real token at
    /// connect time by [`Self::connect_and_register_mcp`]. The server is then
    /// connected + registered live (no restart). The TUI (Piece 3) calls this
    /// once [`wcore_mcp::forge_grant::request_grant`] returns `Granted`.
    pub fn connect_forge_server(&self, name: String, url: String, token: String) {
        match Self::store_and_persist_forge(&name, &url, &token) {
            Ok(server) => {
                // Connect + register live (the `${cred:}` header is resolved inside).
                tokio::spawn(Self::connect_and_register_mcp(
                    self.engine.clone(),
                    self.tx.clone(),
                    name,
                    server,
                ));
            }
            Err(e) => {
                let _ = self.tx.send(ProtocolEvent::Error {
                    msg_id: None,
                    error: ErrorInfo {
                        code: "mcp_connect".to_string(),
                        message: e,
                        retryable: false,
                    },
                });
            }
        }
    }

    /// Slice 3, Piece 3 — the full one-command Forge connect: discover → live
    /// probe → grant → connect. `name` selects among discovered servers (or
    /// picks the sole one). The grant `POST` triggers the human **Approve**
    /// prompt in the producer's GUI, so this runs entirely off-thread and
    /// reports progress/outcome as turns on the bridge channel. Recoverable
    /// grant outcomes (denied / rate-limited / bad-scopes) are surfaced as
    /// guidance, not hard errors.
    pub fn connect_forge_discovered(&self, name: Option<String>) {
        let tx = self.tx.clone();
        let info = |msg: String| {
            let _ = tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message: msg,
            });
        };

        // Pick the target server synchronously (a cheap local file read).
        let servers = wcore_config::forge_discovery::read_discovered_servers();
        let server = match Self::pick_forge_server(servers, name.as_deref()) {
            Ok(s) => s,
            Err(msg) => {
                info(msg);
                return;
            }
        };
        let Some(grant_url) = server.auth.grant_url.clone() else {
            info(format!(
                "'{}' doesn't use the loopback-grant flow — nothing to authorize.",
                server.label()
            ));
            return;
        };

        let engine = self.engine.clone();
        let tx2 = self.tx.clone();
        let label = server.label().to_string();
        tokio::spawn(async move {
            let report_info = |msg: String| {
                let _ = tx2.send(ProtocolEvent::Info {
                    msg_id: String::new(),
                    message: msg,
                });
            };
            let report_err = |msg: String| {
                let _ = tx2.send(ProtocolEvent::Error {
                    msg_id: None,
                    error: ErrorInfo {
                        code: "mcp_connect".to_string(),
                        message: msg,
                        retryable: false,
                    },
                });
            };

            let client = wcore_mcp::forge_grant::loopback_http_client();

            // 1. Liveness: a stale discovery entry must not prompt a grant.
            if wcore_mcp::forge_grant::probe_metadata(&client, &server.metadata_url, &server.name)
                .await
                .is_none()
            {
                report_err(format!(
                    "'{label}' is listed but not responding — it may not be running. \
                     Start it and try /mcp connect again."
                ));
                return;
            }

            // 2. Grant — this triggers the Approve prompt in the producer's GUI.
            report_info(format!(
                "Requesting access to '{label}' — approve the request in its window…"
            ));
            match wcore_mcp::forge_grant::request_grant(
                &client,
                &grant_url,
                "Wayland Core",
                &["read", "write"],
            )
            .await
            {
                wcore_mcp::forge_grant::GrantOutcome::Granted { token, .. } => {
                    // F12: the granted bearer token must never be sent off-box.
                    // Refuse to store/persist/connect unless the server URL is
                    // strictly loopback (SSRF guard) — a tampered discovery file
                    // could otherwise point url at an attacker host.
                    if !wcore_tools::url_safety::is_loopback_url(&server.url) {
                        report_err(format!(
                            "refusing to connect Forge server '{}': its url '{}' is not loopback \
                             — the granted token must not be sent off-box (SSRF guard)",
                            server.name, server.url
                        ));
                        return;
                    }
                    match Self::store_and_persist_forge(&server.name, &server.url, &token) {
                        Ok(cfg) => {
                            Self::connect_and_register_mcp(engine, tx2, server.name, cfg).await
                        }
                        Err(e) => report_err(e),
                    }
                }
                wcore_mcp::forge_grant::GrantOutcome::Denied => report_err(format!(
                    "Access to '{label}' was not approved. Run /mcp connect to try again."
                )),
                wcore_mcp::forge_grant::GrantOutcome::RateLimited => report_err(format!(
                    "'{label}' is rate-limiting grant requests — wait a moment and retry."
                )),
                wcore_mcp::forge_grant::GrantOutcome::BadScopes(m) => {
                    report_err(format!("'{label}' rejected the requested scopes: {m}"))
                }
                wcore_mcp::forge_grant::GrantOutcome::Error(m) => {
                    report_err(format!("Couldn't get access to '{label}': {m}"))
                }
            }
        });
    }

    /// Pick a discovered Forge server by `name`, or the sole one when no name is
    /// given. Returns a user-facing message (Err) when the choice is ambiguous,
    /// absent, or unnamed-but-plural.
    fn pick_forge_server(
        servers: Vec<wcore_config::forge_discovery::DiscoveredMcpServer>,
        name: Option<&str>,
    ) -> Result<wcore_config::forge_discovery::DiscoveredMcpServer, String> {
        if servers.is_empty() {
            return Err(
                "No Forge MCP servers found. (Looked in the Forge discovery file under your OS \
                 config dir — start Agent Vault, then try /mcp connect.)"
                    .to_string(),
            );
        }
        match name {
            Some(n) => servers
                .into_iter()
                .find(|s| s.name == n)
                .ok_or_else(|| format!("No discovered Forge server named '{n}'.")),
            None => {
                if servers.len() == 1 {
                    Ok(servers.into_iter().next().expect("len == 1"))
                } else {
                    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
                    Err(format!(
                        "Multiple Forge servers discovered ({}). Pick one: /mcp connect <name>.",
                        names.join(", ")
                    ))
                }
            }
        }
    }

    /// Store a granted bearer `token` in the credentials store and persist a
    /// `[mcp.servers.<name>]` entry whose `Authorization` header is a `${cred:}`
    /// reference (so the token never lands in `config.toml`). Returns the built
    /// server config (ready to connect), or a user-facing error string. Shared
    /// by [`Self::connect_forge_server`] and [`Self::connect_forge_discovered`].
    fn store_and_persist_forge(
        name: &str,
        url: &str,
        token: &str,
    ) -> Result<wcore_config::config::McpServerConfig, String> {
        let cred_key = wcore_config::mcp_cred_refs::mcp_token_cred_key(name);
        let cfg = wcore_config::config::Config::resolve(&wcore_config::config::CliArgs::default())
            .map_err(|e| format!("Couldn't load config to store the grant token: {e}"))?;
        let store = cfg
            .open_credentials_store()
            .map_err(|e| format!("Credentials store unavailable: {e}"))?;
        store
            .put(&cred_key, token)
            .map_err(|e| format!("Couldn't store the grant token securely: {e}"))?;

        let server = wcore_config::mcp_cred_refs::build_forge_mcp_server_config(url, &cred_key);
        wcore_config::config::patch_global_config({
            let name = name.to_string();
            let server = server.clone();
            move |f| {
                f.mcp.servers.insert(name, server);
            }
        })
        .map_err(|e| format!("Couldn't persist the MCP server config: {e}"))?;

        Ok(server)
    }

    /// Connect a single MCP server and register its tools onto the live engine
    /// registry, reporting progress/failure on the bridge channel. Shared by the
    /// `/mcp add` path and the Forge connect flow. `${cred:KEY}` references in
    /// the server's headers are resolved against the credentials store here, at
    /// the connect boundary — `config.toml` keeps the literal reference.
    async fn connect_and_register_mcp(
        engine: Arc<tokio::sync::Mutex<wcore_agent::engine::AgentEngine>>,
        tx: UnboundedSender<ProtocolEvent>,
        name: String,
        mut config: wcore_config::config::McpServerConfig,
    ) {
        // #135: idempotency. If a server with this name is already connected,
        // re-adding it must NOT spawn a duplicate connection (a second stdio
        // child process for stdio transports). Check the live registry BEFORE
        // any connect, report the no-op, and return. Covers the plain `/mcp add`
        // path and the Forge grant path (both funnel through here).
        let already_connected = engine.lock().await.mcp_server_connected(&name);
        if already_connected {
            let _ = tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message: format!(
                    "MCP server '{name}' is already connected — keeping the existing \
                     connection (no duplicate started). To change its settings, remove \
                     it first, then add it again."
                ),
            });
            return;
        }

        // Resolve `${cred:KEY}` header references just before connecting. This
        // is the single-server live-add path: per the `mcp_cred_refs` contract,
        // a resolution failure (missing key / store error / malformed ref) is a
        // HARD error the user must see — the user just asked to connect THIS
        // server, so silently falling through to a literal `${cred:...}` bearer
        // would be a confusing mis-connect (F22). A no-reference header (the
        // plain `/mcp add` path) never touches the store and never errors.
        if let Ok(cfg) =
            wcore_config::config::Config::resolve(&wcore_config::config::CliArgs::default())
            && let Ok(store) = cfg.open_credentials_store()
            && let Err(e) =
                wcore_config::mcp_cred_refs::resolve_server_headers(&mut config, &*store)
        {
            let reason = format!("{e}");
            let _ = tx.send(ProtocolEvent::McpFailed {
                name: name.clone(),
                reason: reason.clone(),
            });
            let _ = tx.send(ProtocolEvent::Error {
                msg_id: None,
                error: ErrorInfo {
                    code: "mcp_add".to_string(),
                    message: format!("Couldn't connect MCP server '{name}': {reason}"),
                    retryable: false,
                },
            });
            return;
        }

        let mut single = std::collections::HashMap::new();
        single.insert(name.clone(), config.clone());
        let manager = match wcore_mcp::manager::McpManager::connect_all(&single).await {
            Ok(mgr) => std::sync::Arc::new(mgr),
            Err(e) => {
                let reason = format!("{e}");
                let _ = tx.send(ProtocolEvent::McpFailed {
                    name: name.clone(),
                    reason: reason.clone(),
                });
                let _ = tx.send(ProtocolEvent::Error {
                    msg_id: None,
                    error: ErrorInfo {
                        code: "mcp_add".to_string(),
                        message: format!("Couldn't connect MCP server '{name}': {reason}"),
                        retryable: false,
                    },
                });
                return;
            }
        };
        // `connect_all` is non-fatal per-server: a server that failed or
        // timed out still returns `Ok` with the cause recorded in
        // `health()`, not `Err`. Surface that honestly instead of falling
        // through and reporting "connected, 0 tools".
        use wcore_mcp::manager::McpServerHealth;
        match manager.health().get(&name) {
            Some(McpServerHealth::Ready { .. }) => {}
            other => {
                let reason = match other {
                    Some(McpServerHealth::Failed { reason }) => reason.clone(),
                    Some(McpServerHealth::TimedOut { after }) => {
                        format!("connect timed out after {after:?}")
                    }
                    _ => "server did not connect".to_string(),
                };
                let _ = tx.send(ProtocolEvent::McpFailed {
                    name: name.clone(),
                    reason: reason.clone(),
                });
                let _ = tx.send(ProtocolEvent::Error {
                    msg_id: None,
                    error: ErrorInfo {
                        code: "mcp_add".to_string(),
                        message: format!("MCP server '{name}' failed to connect: {reason}"),
                        retryable: false,
                    },
                });
                return;
            }
        }
        // Register the freshly discovered tools onto the LIVE registry.
        // `registry_mut` only hands out a `&mut` when the registry Arc is
        // uncontended — a turn in flight holds a clone, so we report busy
        // instead of dropping the add. The MCP proxies clone the manager
        // Arc internally, so it stays alive after this scope.
        let mut guard = engine.lock().await;
        let builtin_names = guard.tool_names();
        let message = match guard.registry_mut() {
            Some(reg) => {
                wcore_mcp::tool_proxy::register_single_server_tools(
                    reg,
                    &manager,
                    &name,
                    &builtin_names,
                    config.deferred.unwrap_or(true),
                );
                let tool_names: Vec<String> = manager
                    .all_tools()
                    .iter()
                    .filter(|(sn, _)| *sn == name.as_str())
                    .map(|(_, t)| t.name.clone())
                    .collect();
                let tool_count = tool_names.len();
                // Update the TUI's live mcp_status (so /doctor reflects the
                // add) — the bridge's McpReady arm records Ready{tool_count}.
                // Previously this path emitted only Info, leaving mcp_status
                // stale after a live `/mcp add`.
                let _ = tx.send(ProtocolEvent::McpReady {
                    name: name.clone(),
                    tools: tool_names,
                });
                format!(
                    "MCP server '{name}' connected. {tool_count} tool(s) available now. \
                     Type /mcp to see all servers."
                )
            }
            None => {
                let _ = tx.send(ProtocolEvent::Error {
                    msg_id: None,
                    error: ErrorInfo {
                        code: "mcp_add".to_string(),
                        message: format!(
                            "Can't add '{name}' while a turn is running. Try /mcp add again \
                             once it's idle."
                        ),
                        retryable: true,
                    },
                });
                return;
            }
        };
        let _ = tx.send(ProtocolEvent::Info {
            msg_id: String::new(),
            message,
        });
    }

    /// v0.9.1 W1 E (debt sweep): direct `/voice` toggle entry — the
    /// follow-up the v0.9.0 W4 E1 report flagged ("the LLM tool-call
    /// path is fully wired, but the Ctrl+Space → SurfaceAction → tool
    /// path needs that follow-up"). Spawns a background task that:
    ///
    ///  1. Looks up the `voice_mode` Tool in the engine's registry.
    ///     Hidden when no real recorder is wired (R-H6 graceful
    ///     degradation); returns `None` in that case.
    ///  2. Invokes `Tool::execute` with `{"action": "toggle_record"}`
    ///     — the same code path the LLM tool dispatcher uses, just
    ///     without the LLM round-trip Sean asked us to avoid.
    ///  3. Emits a `ProtocolEvent::Info` on the bridge channel so the
    ///     TUI surfaces "Recording started…" or "Recording stopped,
    ///     transcribing…" as a system turn (NOT a tool card — this is
    ///     a UI affordance, not a model-visible result).
    ///
    /// Errors are swallowed and logged via `tracing::warn!` so the
    /// Ctrl+Space chord stays best-effort: a failed device probe shows
    /// up later in `/doctor` rather than crashing the composer.
    pub fn toggle_voice(&self) {
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let registry = {
                let guard = engine.lock().await;
                guard.tools()
            };
            let Some(tool) = registry.get("voice_mode") else {
                // No `voice_mode` tool registered — either cpal
                // couldn't bind a device (CI, container, headless SSH)
                // or the build excluded the backend. Surface a single
                // info line so Ctrl+Space doesn't appear silently
                // broken; `/doctor` will explain why.
                let _ = tx.send(ProtocolEvent::Info {
                    msg_id: String::new(),
                    message: "Voice capture unavailable — run /doctor for details.".to_string(),
                });
                return;
            };
            let result = tool
                .execute(serde_json::json!({ "action": "toggle_record" }))
                .await;
            if result.is_error {
                tracing::warn!("voice_mode toggle_record failed: {}", result.content);
                let _ = tx.send(ProtocolEvent::Info {
                    msg_id: String::new(),
                    message: format!("Voice toggle failed: {}", result.content),
                });
                return;
            }
            // `VoiceModeTool` returns a JSON-encoded
            // `{"is_recording": bool, …}` payload in `content`.
            // Translate the boolean into the human-readable affordance
            // the v0.9.1 spec calls for ("Recording started…" /
            // "Recording stopped, transcribing…").
            let is_recording = serde_json::from_str::<serde_json::Value>(&result.content)
                .ok()
                .and_then(|v| v.get("is_recording").and_then(serde_json::Value::as_bool))
                .unwrap_or(false);
            let message = if is_recording {
                "Recording started…".to_string()
            } else {
                "Recording stopped, transcribing…".to_string()
            };
            let _ = tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message,
            });
        });
    }

    /// D023: run an installed skill directly (the `/skillname` dispatch).
    ///
    /// Skills auto-activate by relevance during a turn, but a user typing
    /// `/lint` for an installed skill wants to RUN it now. The engine already
    /// carries a `Skill` tool (`skill_tool::SkillTool`) that looks a skill up
    /// by name and prepares its content; this invokes that same tool — the
    /// exact path the LLM uses to invoke a skill — without an LLM round-trip,
    /// mirroring [`toggle_voice`](Self::toggle_voice).
    ///
    /// `name` is the skill name WITHOUT the leading slash (the dispatcher
    /// strips it); `args` is the optional remainder of the command line. The
    /// prepared skill body comes back as a single `Info` turn. A missing
    /// `Skill` tool (no skills loaded) or an execution error surfaces a single
    /// honest line rather than failing silently.
    pub fn run_skill(&self, name: String, args: Option<String>) {
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let registry = {
                let guard = engine.lock().await;
                guard.tools()
            };
            let Some(tool) = registry.get("Skill") else {
                let _ = tx.send(ProtocolEvent::Info {
                    msg_id: String::new(),
                    message: "No skill runner is loaded in this session.".to_string(),
                });
                return;
            };
            let mut input = serde_json::json!({ "skill": name });
            if let Some(args) = args.filter(|a| !a.trim().is_empty()) {
                input["args"] = serde_json::Value::String(args);
            }
            let result = tool.execute(input).await;
            let _ = tx.send(ProtocolEvent::Info {
                msg_id: String::new(),
                message: result.content,
            });
        });
    }

    /// D018: list saved sessions for `/resume`, loading the full `Session`
    /// (messages included) for a matched id. `id_or_prefix` matches a full
    /// session id or a `--resume`-style short prefix against the on-disk
    /// index. Returns the deserialized [`Session`](wcore_agent::session::Session)
    /// so the caller can repaint its transcript and rehydrate the engine
    /// conversation buffer. `None` when no store is configured or no session
    /// matches. A fast synchronous index+file read (no engine lock).
    pub fn load_session(&self, id_or_prefix: &str) -> Option<wcore_agent::session::Session> {
        let (dir, max) = self.session_store.clone()?;
        let manager = wcore_agent::session::SessionManager::new(dir, max);
        // Resolve a short prefix to a full id via the index, then load.
        let meta = manager
            .list()
            .ok()?
            .into_iter()
            .find(|m| m.id == id_or_prefix || m.id.starts_with(id_or_prefix))?;
        manager.load(&meta.id).ok()
    }

    /// D018: swap the live engine's conversation buffer to a resumed session's
    /// messages, so the next turn continues that session's context rather than
    /// the in-memory one. Mirrors [`clear_conversation`](Self::clear_conversation)'s
    /// spawn-then-lock shape (the engine mutex is async; this call site is sync).
    ///
    /// Runs in a spawned task; no user-facing event — the caller already
    /// repainted the resumed transcript and pushed the confirmation line.
    pub fn load_conversation(&self, messages: Vec<wcore_types::message::Message>) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            engine.lock().await.load_conversation(messages);
        });
    }
}

/// The outcome of a SUCCESSFUL [`TuiEngine::rebind`] — the synchronously
/// resolved pieces the router needs to keep the view state honest after a
/// live rebind. Returned only when `Config::resolve` + `create_provider`
/// succeeded (a resolve failure returns `None`, signalling "live apply
/// skipped" so the caller never shows a false "now live").
pub struct RebindApplied {
    /// H2: the resolved approval posture as a live `SessionMode`. The router
    /// writes this onto `App::mode` so the status-bar approval badge matches
    /// the posture just pushed to the live `ToolApprovalManager` — otherwise
    /// the badge stays stale (displayed != behavior).
    pub session_mode: wcore_protocol::commands::SessionMode,
    /// M4: a fresh [`ConfigView`](crate::tui::app::ConfigView) from the
    /// re-resolved disk config. The router mirrors the saved Tier-1 fields onto
    /// `App::config` so the next `/config` `on_enter` re-seeds from the
    /// just-saved truth rather than snapping back to the pre-save values.
    pub config_view: crate::tui::app::ConfigView,
}

/// Convert an engine `ToolStatus` into the protocol enum. Tiny shim kept
/// here so the dependency direction stays one-way (TUI → protocol).
#[allow(dead_code)]
fn tool_status(is_error: bool) -> ToolStatus {
    if is_error {
        ToolStatus::Error
    } else {
        ToolStatus::Success
    }
}

/// Map a resolved `[default] approval_mode` to the live `SessionMode` the
/// approval manager honours. Mirrors `main.rs::approval_mode_to_session`
/// (kept local so the engine-rebind seam does not reach into the binary
/// crate). Pure so it unit- and integration-tests without an engine; `pub`
/// so the engine-rebind integration test can pin the D007 mapping.
pub fn approval_mode_to_session(
    mode: wcore_config::config::ApprovalMode,
) -> wcore_protocol::commands::SessionMode {
    use wcore_protocol::commands::SessionMode;
    match mode {
        wcore_config::config::ApprovalMode::Default => SessionMode::Default,
        wcore_config::config::ApprovalMode::AutoEdit => SessionMode::AutoEdit,
        wcore_config::config::ApprovalMode::Force => SessionMode::Force,
    }
}

/// D016: build the system prompt installed on engine rebind — the resolved
/// `[default] system_prompt` (if any) with the onboarded `[default] user`
/// display name folded in. Pure so the prompt-shape contract unit-tests
/// without a live engine.
///
/// The name block is PREPENDED to the resolved prompt so the model reads
/// "who am I talking to" before any persona instructions, mirroring how the
/// boot path layers context. When no name is set the resolved prompt passes
/// through unchanged; when neither is set the result is empty (the engine's
/// default-empty prompt, exactly as a fresh boot would produce). `pub` so the
/// engine-rebind integration test can pin the D016 display-name fold.
pub fn build_rebind_system_prompt(system_prompt: Option<&str>, user_name: Option<&str>) -> String {
    let base = system_prompt.unwrap_or("").trim();
    match user_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => {
            let name_block = format!("You are talking to {name}.");
            if base.is_empty() {
                name_block
            } else {
                format!("{name_block}\n\n{base}")
            }
        }
        None => base.to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Onboarding config write
// ─────────────────────────────────────────────────────────────────────────

/// One provider + its API key, as gathered by the onboarding flow.
#[derive(Debug, Clone)]
pub struct OnboardingProvider {
    /// The provider slug — `anthropic`, `openai`, … — used as the
    /// `[providers.<slug>]` table name and the `[default] provider`.
    pub slug: String,
    /// The validated API key for this provider.
    pub api_key: String,
}

/// Render the onboarding-gathered providers + display name into a TOML
/// document body. Pure — no filesystem side effects — so it is unit
/// testable without touching the global config path.
///
/// The first provider in `providers` becomes `[default] provider`. The
/// display `name`, when present, is written as `[default] user`. `toml`
/// escaping is handled by `toml::Value` so a key/name with a quote or
/// backslash cannot break the file.
fn render_onboarding_config(providers: &[OnboardingProvider], name: Option<&str>) -> String {
    let mut doc = toml::value::Table::new();

    let mut default = toml::value::Table::new();
    if let Some(first) = providers.first() {
        default.insert(
            "provider".to_string(),
            toml::Value::String(first.slug.clone()),
        );
        // D002: stamp a default model up front when the provider has one, so a
        // built-in provider never lands in the no-model dead-end on the first
        // prompt. Catalog / Tier-2 providers (Groq, OpenRouter, DeepSeek, …)
        // have no sensible default and resolve to `""` — those are recovered
        // in-app via the Workspace `/model` affordance, so we write no model
        // line for them rather than a wrong guess.
        let model = wcore_config::config::default_model_for_slug(&first.slug);
        if !model.is_empty() {
            default.insert("model".to_string(), toml::Value::String(model.to_string()));
        }
    }
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        default.insert(
            "user".to_string(),
            toml::Value::String(name.trim().to_string()),
        );
    }
    doc.insert("default".to_string(), toml::Value::Table(default));

    if !providers.is_empty() {
        let mut providers_tbl = toml::value::Table::new();
        for p in providers {
            let mut provider_tbl = toml::value::Table::new();
            provider_tbl.insert(
                "api_key".to_string(),
                toml::Value::String(p.api_key.clone()),
            );
            // Last write wins if the user added the same provider twice —
            // the onboarding flow already de-duplicates, this is defensive.
            providers_tbl.insert(p.slug.clone(), toml::Value::Table(provider_tbl));
        }
        doc.insert("providers".to_string(), toml::Value::Table(providers_tbl));
    }

    toml::to_string_pretty(&toml::Value::Table(doc))
        .expect("onboarding config table always serializes")
}

/// The global `config.toml` path the onboarding writer targets. Exposed
/// so the onboarding surface can show the path (and detect an existing
/// config) without reaching into `wcore_config` itself.
pub fn onboarding_config_path() -> std::path::PathBuf {
    wcore_config::config::global_config_path()
}

/// Write a minimal provider-choice record (`[default] provider = "<slug>"`)
/// to `config.toml` without storing any API key on disk.
///
/// Used by the env-key connect path: the provider selection is persisted the
/// moment the user presses the number key so a relaunch sees the choice and
/// lands on the Workspace surface instead of re-running onboarding.  The key
/// itself remains in the environment variable and is read by the engine on the
/// next boot — nothing secret is written to disk.
///
/// Silently skips the write if `config.toml` already exists (the user's
/// existing config must never be clobbered by this lightweight write).
/// Returns `true` when a file was written, `false` when it was skipped.
pub fn persist_env_provider_selection(slug: &str) -> bool {
    use std::io::Write;

    let path = wcore_config::config::global_config_path();
    if path.exists() {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    // Write the minimal TOML: just [default] with the provider choice.
    // Include the default model when one is known so the first prompt does
    // not land in the no-model dead-end (mirrors render_onboarding_config).
    let model = wcore_config::config::default_model_for_slug(slug);
    let mut content = format!("[default]\nprovider = \"{slug}\"\n");
    if !model.is_empty() {
        content.push_str(&format!("model = \"{model}\"\n"));
    }
    let Ok(mut f) = std::fs::File::create(&path) else {
        return false;
    };
    if f.write_all(content.as_bytes()).is_err() {
        return false;
    }
    // Best-effort 0o600 hardening — same as write_onboarding_config.
    let _ = wcore_config::credentials::secure_credential_file(&path);
    true
}

/// Write the onboarding-chosen providers + display name into the global
/// `config.toml`.
///
/// `wcore_config::init_config()` only stamps a fixed default template
/// (no provider/key arguments, fixed stdout side effects), so it is the
/// wrong primitive for the onboarding completion. This writer renders a
/// minimal, well-formed config: the first provider as the default, every
/// gathered provider's key under its own `[providers.<slug>]` table, and
/// the display name under `[default] user`. It then tightens file
/// permissions to `0o600` via the credentials helper so keys are never
/// world-readable.
///
/// When `overwrite` is `false` an existing config is **not** clobbered —
/// the writer returns an error. When `true` (the user explicitly chose
/// "Overwrite" on the Ready step) the existing file is replaced. Returns
/// the path written, or an error describing why the write failed (a
/// missing config dir, a permission error, …).
pub fn write_onboarding_config(
    providers: &[OnboardingProvider],
    name: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context;

    let path = wcore_config::config::global_config_path();
    if path.exists() && !overwrite {
        anyhow::bail!(
            "config already exists at {} — edit it directly or via /config",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }

    let rendered = render_onboarding_config(providers, name);
    std::fs::write(&path, rendered)
        .with_context(|| format!("writing config to {}", path.display()))?;
    // SECURITY: enforce 0o600 so keys are never world-readable.
    wcore_config::credentials::secure_credential_file(&path)
        .with_context(|| format!("securing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod onboarding_config_tests {
    use super::{OnboardingProvider, render_onboarding_config, write_onboarding_config};

    fn prov(slug: &str, key: &str) -> OnboardingProvider {
        OnboardingProvider {
            slug: slug.to_string(),
            api_key: key.to_string(),
        }
    }

    #[test]
    fn write_refuses_to_clobber_an_existing_config() {
        // `global_config_path()` is process-global; in the test
        // environment a config may or may not exist. If it does,
        // `write_onboarding_config` with `overwrite = false` must refuse
        // rather than clobber.
        let path = wcore_config::config::global_config_path();
        if path.exists() {
            let err =
                write_onboarding_config(&[prov("anthropic", "sk-ant-test")], Some("Sean"), false)
                    .expect_err("must refuse to overwrite an existing config");
            assert!(err.to_string().contains("already exists"));
        }
        // If no config exists the writer is exercised by integration —
        // a unit test must not create a real global config side effect.
    }

    #[test]
    fn render_uses_first_provider_as_default_and_writes_name() {
        let body = render_onboarding_config(
            &[prov("anthropic", "sk-ant-1"), prov("openai", "sk-2")],
            Some("Sean"),
        );
        let parsed: toml::Value = toml::from_str(&body).expect("valid toml");
        assert_eq!(parsed["default"]["provider"].as_str(), Some("anthropic"));
        assert_eq!(parsed["default"]["user"].as_str(), Some("Sean"));
        // Every gathered provider gets its own table + key.
        assert_eq!(
            parsed["providers"]["anthropic"]["api_key"].as_str(),
            Some("sk-ant-1")
        );
        assert_eq!(
            parsed["providers"]["openai"]["api_key"].as_str(),
            Some("sk-2")
        );
    }

    #[test]
    fn render_stamps_default_model_for_builtin_but_not_catalog_provider() {
        // D002: a built-in provider with a known default gets a `[default]
        // model` line stamped up front, so the first prompt never dead-ends.
        let builtin = render_onboarding_config(&[prov("anthropic", "sk-ant-1")], None);
        let parsed: toml::Value = toml::from_str(&builtin).expect("valid toml");
        assert!(
            parsed["default"]["model"]
                .as_str()
                .is_some_and(|m| !m.is_empty()),
            "a built-in provider must get a stamped default model:\n{builtin}"
        );

        // A catalog / Tier-2 provider (Groq) has no sensible default — no model
        // line is written; the in-app `/model` recovery banner covers it.
        let catalog = render_onboarding_config(&[prov("groq", "gsk-1")], None);
        let parsed_c: toml::Value = toml::from_str(&catalog).expect("valid toml");
        assert!(
            parsed_c["default"].get("model").is_none(),
            "a catalog provider with no default must NOT get a guessed model line:\n{catalog}"
        );
    }

    #[test]
    fn render_omits_name_when_absent_or_blank() {
        let body = render_onboarding_config(&[prov("anthropic", "k")], None);
        let parsed: toml::Value = toml::from_str(&body).expect("valid toml");
        assert!(parsed["default"].get("user").is_none());
        let body_blank = render_onboarding_config(&[prov("anthropic", "k")], Some("   "));
        let parsed_blank: toml::Value = toml::from_str(&body_blank).expect("valid toml");
        assert!(parsed_blank["default"].get("user").is_none());
    }

    #[test]
    fn render_escapes_quotes_in_keys() {
        // A key with a quote must not break the TOML — `toml::Value`
        // owns the escaping.
        let body = render_onboarding_config(&[prov("openai", "weird\"key")], None);
        let parsed: toml::Value = toml::from_str(&body).expect("valid toml despite quote");
        assert_eq!(
            parsed["providers"]["openai"]["api_key"].as_str(),
            Some("weird\"key")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;
    use wcore_protocol::events::FinishReason;

    #[test]
    fn channel_emitter_forwards_events() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        emitter
            .emit(&ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            })
            .expect("emit succeeds");
        let got = rx.try_recv().expect("event forwarded");
        match got {
            ProtocolEvent::StreamStart { msg_id } => assert_eq!(msg_id, "m1"),
            other => panic!("expected StreamStart, got {other:?}"),
        }
    }

    fn disc(name: &str) -> wcore_config::forge_discovery::DiscoveredMcpServer {
        wcore_config::forge_discovery::DiscoveredMcpServer {
            name: name.to_string(),
            display_name: None,
            transport: "streamable-http".to_string(),
            url: format!("http://127.0.0.1:3456/{name}"),
            auth: wcore_config::forge_discovery::DiscoveredAuth {
                scheme: "loopback-grant".to_string(),
                grant_url: Some("http://127.0.0.1:3456/grant".to_string()),
            },
            metadata_url: "http://127.0.0.1:3456/.well-known/mcp".to_string(),
        }
    }

    #[test]
    fn pick_forge_server_errors_when_none_discovered() {
        let got = TuiEngine::pick_forge_server(vec![], None);
        assert!(got.unwrap_err().contains("No Forge MCP servers"));
    }

    #[test]
    fn pick_forge_server_returns_sole_server_without_a_name() {
        let got = TuiEngine::pick_forge_server(vec![disc("agent-vault")], None).unwrap();
        assert_eq!(got.name, "agent-vault");
    }

    #[test]
    fn pick_forge_server_requires_a_name_when_plural() {
        let err = TuiEngine::pick_forge_server(vec![disc("a"), disc("b")], None).unwrap_err();
        assert!(err.contains("Multiple Forge servers"));
        assert!(err.contains("a") && err.contains("b"));
    }

    #[test]
    fn pick_forge_server_selects_by_name() {
        let got = TuiEngine::pick_forge_server(vec![disc("a"), disc("b")], Some("b")).unwrap();
        assert_eq!(got.name, "b");
    }

    #[test]
    fn pick_forge_server_reports_unknown_name() {
        let err = TuiEngine::pick_forge_server(vec![disc("a")], Some("nope")).unwrap_err();
        assert!(err.contains("nope"));
    }

    #[test]
    fn render_model_info_list_marks_active_and_shows_resolved_id() {
        use wcore_providers::ModelInfo;
        // A LIVE library the static 5-alias catalog would never contain — the
        // point of D008 is that the picker reflects whatever the provider
        // actually returns, not a hardcoded list.
        let models = vec![
            ModelInfo {
                id: "live-flagship-2026".into(),
                display: "Flagship".into(),
            },
            ModelInfo {
                id: "live-fast-2026".into(),
                display: "Fast".into(),
            },
        ];
        let out = render_model_info_list("anthropic", &models, "live-fast-2026");
        // Both live models render, with their resolved ids alongside labels.
        assert!(out.contains("Flagship") && out.contains("live-flagship-2026"));
        assert!(out.contains("Fast") && out.contains("live-fast-2026"));
        // The active one is marked ●; the other ○.
        let active_line = out
            .lines()
            .find(|l| l.contains("live-fast-2026"))
            .expect("active row present");
        assert!(
            active_line.contains('●'),
            "active model marked: {active_line}"
        );
        let other_line = out
            .lines()
            .find(|l| l.contains("live-flagship-2026"))
            .expect("other row present");
        assert!(
            other_line.contains('○'),
            "inactive model unmarked: {other_line}"
        );
    }

    #[test]
    fn render_model_info_list_empty_is_honest_not_blank() {
        // A provider that returns nothing (and whose alias fallback is also
        // empty) must still give the user an actionable next step.
        let out = render_model_info_list("custom", &[], "my-model");
        assert!(
            out.contains("/model <id>"),
            "offers a direct-set path: {out}"
        );
        assert!(out.contains("my-model"), "shows the current model: {out}");
    }

    #[test]
    fn render_model_info_list_marks_active_by_display_short_form() {
        use wcore_providers::ModelInfo;
        // When the live active value is the short-form display (e.g. the alias
        // fallback path), the row still marks ● by matching `display`.
        let models = vec![ModelInfo {
            id: "anthropic.opus.resolved".into(),
            display: "opus".into(),
        }];
        let out = render_model_info_list("anthropic", &models, "opus");
        assert!(out.contains('●'), "short-form active still marked: {out}");
    }

    #[test]
    fn mcp_config_from_target_classifies_url_and_command_d024() {
        use wcore_config::config::TransportType;

        // An https URL → streamable-http with the URL set, no command.
        let https = mcp_config_from_target("https://mcp.example.com/sse").expect("url ok");
        assert!(matches!(https.transport, TransportType::StreamableHttp));
        assert_eq!(https.url.as_deref(), Some("https://mcp.example.com/sse"));
        assert!(https.command.is_none());
        // A runtime add is eager (the user asked for it explicitly).
        assert_eq!(https.deferred, Some(false));

        // A bare command → stdio with program + argv split out.
        let stdio = mcp_config_from_target("npx -y @scope/server --flag").expect("cmd ok");
        assert!(matches!(stdio.transport, TransportType::Stdio));
        assert_eq!(stdio.command.as_deref(), Some("npx"));
        assert_eq!(
            stdio.args.as_deref(),
            Some(
                ["-y", "@scope/server", "--flag"]
                    .map(String::from)
                    .as_slice()
            )
        );
        assert!(stdio.url.is_none());

        // A single-token command has no args (None, not an empty Vec).
        let one = mcp_config_from_target("my-server").expect("single ok");
        assert_eq!(one.command.as_deref(), Some("my-server"));
        assert!(one.args.is_none());

        // An empty target is an honest error, never a spawned empty command.
        assert!(mcp_config_from_target("   ").is_err());
    }

    #[test]
    fn channel_emitter_drops_silently_when_receiver_gone() {
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx); // receiver gone — the TUI shut down
        let emitter = ChannelEmitter::new(tx);
        // A closed channel must not surface as an error.
        emitter
            .emit(&ProtocolEvent::Pong)
            .expect("closed channel is not an error");
    }

    // ── E3: ApprovalRequired synthesis after ToolRequest ────────────────
    //
    // The engine's orchestration approval path emits `ToolRequest` only
    // when a tool actually needs approval (allow-listed tools skip the
    // event entirely). The TUI emitter synthesizes the matching
    // `ApprovalRequired` event so the protocol bridge can flip the card
    // status and surface the modal. These tests pin that contract.

    fn tool_request(
        call_id: &str,
        name: &str,
        category: wcore_protocol::events::ToolCategory,
    ) -> ProtocolEvent {
        ProtocolEvent::ToolRequest {
            msg_id: "m1".into(),
            call_id: call_id.into(),
            tool: wcore_protocol::events::ToolInfo {
                name: name.into(),
                category,
                args: serde_json::json!({"command": "echo hi"}),
                description: "execute a shell command".into(),
            },
        }
    }

    #[test]
    fn channel_emitter_synthesizes_approval_required_after_tool_request() {
        // A ToolRequest reaching the TUI's emitter unambiguously means
        // the engine is parked on `request_approval` for this call —
        // synthesize the missing `ApprovalRequired` so the modal opens.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        emitter
            .emit(&tool_request(
                "c-1",
                "Bash",
                wcore_protocol::events::ToolCategory::Exec,
            ))
            .expect("emit succeeds");
        // First the original ToolRequest.
        match rx.try_recv().expect("first event") {
            ProtocolEvent::ToolRequest { call_id, .. } => assert_eq!(call_id, "c-1"),
            other => panic!("expected ToolRequest first, got {other:?}"),
        }
        // Then the synthesized ApprovalRequired with the matching call_id.
        match rx.try_recv().expect("synthesized ApprovalRequired") {
            ProtocolEvent::ApprovalRequired {
                call_id,
                reason,
                resume_token,
                correlation_id,
                ..
            } => {
                assert_eq!(call_id, "c-1");
                assert_eq!(reason, "exec");
                // Resume token + correlation id are the call_id by
                // convention so the host can resolve without extra state.
                assert_eq!(resume_token, "c-1");
                assert_eq!(correlation_id, "c-1");
            }
            other => panic!("expected ApprovalRequired second, got {other:?}"),
        }
    }

    #[test]
    fn dedupe_emitter_suppresses_explicit_approval_after_synthesizing_one() {
        // Wave 6 #24 — the live-workflow gate emits `ToolRequest` + its OWN
        // explicit `ApprovalRequired` for one call_id. A `with_dedupe` emitter
        // synthesizes a gate after the `ToolRequest` AND records the call_id, so
        // the later explicit `ApprovalRequired` for the same id is suppressed:
        // exactly ONE gate frame reaches the channel (no double bell / no
        // malformed second frame downstream).
        let (tx, mut rx) = mpsc::unbounded_channel();
        let seen: DedupeSet = Arc::new(Mutex::new(HashSet::new()));
        let emitter = ChannelEmitter::with_dedupe(tx, seen, None);

        // (1) ToolRequest → forwarded + one synthesized ApprovalRequired.
        emitter
            .emit(&tool_request(
                "wf1",
                "Workflow",
                wcore_protocol::events::ToolCategory::Exec,
            ))
            .expect("emit ToolRequest");
        // (2) The engine's OWN explicit ApprovalRequired for the same call_id —
        // must be dropped by the dedupe guard.
        emitter
            .emit(&ProtocolEvent::ApprovalRequired {
                call_id: "wf1".into(),
                resume_token: "wf1".into(),
                correlation_id: "wf1".into(),
                reason: "Run ForgeFlow `demo`?".into(),
                context: "~2 agents".into(),
                plan: None,
            })
            .expect("emit explicit ApprovalRequired");

        // Drain: ToolRequest, then the single synthesized ApprovalRequired.
        assert!(matches!(
            rx.try_recv().expect("ToolRequest"),
            ProtocolEvent::ToolRequest { .. }
        ));
        match rx.try_recv().expect("synthesized ApprovalRequired") {
            ProtocolEvent::ApprovalRequired {
                call_id, reason, ..
            } => {
                assert_eq!(call_id, "wf1");
                // The synthesized reason ("exec") wins; the explicit one is
                // suppressed (matching the json-stream GatingProtocolWriter).
                assert_eq!(reason, "exec");
            }
            other => panic!("expected ApprovalRequired, got {other:?}"),
        }
        // No third frame: the explicit ApprovalRequired was suppressed.
        assert!(
            rx.try_recv().is_err(),
            "explicit ApprovalRequired for an already-synthesized call_id must be suppressed"
        );
    }

    #[test]
    fn channel_emitter_carries_crucible_plan_from_tool_args() {
        // Crucible Stage 4: the engine puts the typed CruciblePlan in the
        // ToolRequest args; the synthesized gate frame must carry it (the
        // explicit ApprovalRequired{plan} is suppressed by the dedupe), else the
        // TUI renders "plan unavailable" instead of the cost card.
        use wcore_types::crucible::{CouncilMemberCard, CouncilRole, CruciblePlan};
        let plan = CruciblePlan {
            convene: true,
            members: vec![CouncilMemberCard {
                spec: "openai:gpt-5".into(),
                vendor: "openai".into(),
                role: CouncilRole::Proposer,
            }],
            stakes: "med".into(),
            focus: None,
            ceiling_microcents: Some(210_000_000),
            single_model_baseline_microcents: None,
            day_spent_microcents: None,
            day_cap_microcents: None,
            judge_independent: true,
            reason: "x".into(),
            trims: vec![],
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        emitter
            .emit(&ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "cru1".into(),
                tool: wcore_protocol::events::ToolInfo {
                    name: "Crucible".into(),
                    category: wcore_protocol::events::ToolCategory::Exec,
                    args: serde_json::json!({
                        "task": "t",
                        "plan": serde_json::to_value(&plan).unwrap(),
                    }),
                    description: "Convene a cross-vendor council".into(),
                },
            })
            .expect("emit ToolRequest");
        assert!(matches!(
            rx.try_recv().unwrap(),
            ProtocolEvent::ToolRequest { .. }
        ));
        match rx.try_recv().expect("synthesized ApprovalRequired") {
            ProtocolEvent::ApprovalRequired { plan: Some(p), .. } => {
                assert_eq!(p.ceiling_microcents, Some(210_000_000));
                assert_eq!(p.members.len(), 1);
            }
            other => panic!("expected ApprovalRequired carrying the plan, got {other:?}"),
        }
    }

    #[test]
    fn channel_emitter_maps_tool_category_to_approval_reason() {
        // The `reason` string surfaces in the modal subtitle; map every
        // category to a human-readable label.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        for (cat, want) in [
            (wcore_protocol::events::ToolCategory::Edit, "edit"),
            (wcore_protocol::events::ToolCategory::Exec, "exec"),
            (wcore_protocol::events::ToolCategory::Mcp, "mcp"),
            (wcore_protocol::events::ToolCategory::Info, "info"),
        ] {
            emitter
                .emit(&tool_request("c", "X", cat))
                .expect("emit succeeds");
            let _ = rx.try_recv().expect("ToolRequest"); // drain
            match rx.try_recv().expect("ApprovalRequired") {
                ProtocolEvent::ApprovalRequired { reason, .. } => assert_eq!(reason, want),
                other => panic!("expected ApprovalRequired, got {other:?}"),
            }
        }
    }

    #[test]
    fn channel_emitter_does_not_synthesize_for_non_tool_request_events() {
        // ToolResult, StreamStart, etc. must not trigger an
        // ApprovalRequired synthesis — only ToolRequest does.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        emitter
            .emit(&ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            })
            .expect("emit succeeds");
        let _ = rx.try_recv().expect("StreamStart"); // drain
        assert!(
            rx.try_recv().is_err(),
            "no synthesized event must follow StreamStart"
        );
    }

    #[test]
    fn allow_listed_tool_skips_modal_by_never_emitting_tool_request() {
        // E3 contract pin: the allow_list bypass lives upstream in the
        // engine (orchestration/mod.rs:844-846,
        // `!allow_list.contains(&name)`). An allow-listed tool (`web`,
        // `web_fetch`, `vision`, `transcribe`) skips the entire
        // `needs_approval` block — so `ToolRequest` never reaches this
        // emitter. We model that here by sending a `ToolRunning` event
        // directly (the bypass branch's first event) and asserting that
        // no synthesized `ApprovalRequired` follows.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        emitter
            .emit(&ProtocolEvent::ToolRunning {
                msg_id: "m1".into(),
                call_id: "web-1".into(),
                tool_name: "web".into(),
            })
            .expect("emit succeeds");
        let _ = rx.try_recv().expect("ToolRunning"); // drain
        // No synthesized ApprovalRequired because no ToolRequest came in.
        assert!(
            rx.try_recv().is_err(),
            "allow-listed tools must not trigger an approval synthesis"
        );
    }

    #[test]
    fn channel_emitter_carries_tool_description_as_approval_context() {
        // The `context` field on ApprovalRequired carries the tool's
        // description so the modal can show the user what's being asked.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        emitter
            .emit(&tool_request(
                "c-2",
                "Bash",
                wcore_protocol::events::ToolCategory::Exec,
            ))
            .expect("emit succeeds");
        let _ = rx.try_recv().expect("ToolRequest");
        match rx.try_recv().expect("ApprovalRequired") {
            ProtocolEvent::ApprovalRequired { context, .. } => {
                assert_eq!(context, "execute a shell command");
            }
            other => panic!("expected ApprovalRequired, got {other:?}"),
        }
    }

    #[test]
    fn channel_sink_translates_text_delta() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        sink.emit_text_delta("hello", "m1");
        match rx.try_recv().expect("event forwarded") {
            ProtocolEvent::TextDelta { text, msg_id } => {
                assert_eq!(text, "hello");
                assert_eq!(msg_id, "m1");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn channel_sink_translates_stream_end_with_usage() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        sink.emit_stream_end("m1", 2, 100, 50, 0, 30, FinishReason::Stop);
        match rx.try_recv().expect("event forwarded") {
            ProtocolEvent::StreamEnd {
                msg_id,
                finish_reason,
                usage,
                ..
            } => {
                assert_eq!(msg_id, "m1");
                assert_eq!(finish_reason, FinishReason::Stop);
                let usage = usage.expect("usage present");
                assert_eq!(usage.input_tokens, 100);
                assert_eq!(usage.cache_read_tokens, Some(30));
                // No cache writes — the field stays `None`.
                assert_eq!(usage.cache_write_tokens, None);
            }
            other => panic!("expected StreamEnd, got {other:?}"),
        }
    }

    #[test]
    fn channel_sink_advertises_streaming_tools() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        // The TUI wants streaming tool chunks.
        assert!(sink.streaming_tools_advertised());
    }

    #[test]
    fn channel_sink_translates_error() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        // retryable=true asserts the flag is threaded through, not hardcoded
        // false (audit finding: the TUI bridge ChannelSink used to discard it).
        sink.emit_error("boom", true);
        match rx.try_recv().expect("event forwarded") {
            ProtocolEvent::Error { error, .. } => {
                assert_eq!(error.message, "boom");
                assert_eq!(error.code, "engine_error");
                assert!(
                    error.retryable,
                    "ChannelSink must honor the caller's retryable flag"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn tool_status_maps_error_flag() {
        assert!(matches!(tool_status(true), ToolStatus::Error));
        assert!(matches!(tool_status(false), ToolStatus::Success));
    }

    // ── AUDIT-D D1 — panic-safe terminal event ───────────────────────
    //
    // `TuiEngine::submit` needs a live `AgentEngine` to construct, so the
    // panic-safety contract is tested at the `TerminalGuard` seam — the
    // unit that actually guarantees a terminal event on every exit path —
    // plus a `tokio::spawn` test that proves a panicking task still
    // delivers `Error` + `StreamEnd` to the bridge channel.

    #[test]
    fn terminal_guard_armed_drop_emits_error_then_stream_end() {
        // A guard dropped while still armed (the panic / abort path) must
        // synthesize an `Error` followed by a `StreamEnd` so the TUI
        // clears `streaming_active` and shows the user something failed.
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let _guard = TerminalGuard::new(tx, "m1".to_string());
            // dropped here without `disarm()` — simulates a panic/abort.
        }
        match rx.try_recv().expect("armed drop must emit an Error") {
            ProtocolEvent::Error { msg_id, error } => {
                assert_eq!(msg_id.as_deref(), Some("m1"));
                assert_eq!(error.code, "engine_panic");
            }
            other => panic!("expected Error first, got {other:?}"),
        }
        match rx.try_recv().expect("armed drop must emit a StreamEnd") {
            ProtocolEvent::StreamEnd {
                msg_id,
                finish_reason,
                ..
            } => {
                assert_eq!(msg_id, "m1");
                assert_eq!(finish_reason, FinishReason::Error);
            }
            other => panic!("expected StreamEnd second, got {other:?}"),
        }
    }

    #[test]
    fn terminal_guard_disarmed_drop_emits_nothing() {
        // The normal `Ok`/`Err` paths `disarm()` the guard after sending
        // their own terminal events — a disarmed guard's `Drop` must be
        // silent so the bridge does not see a duplicate terminal pair.
        let (tx, mut rx) = mpsc::unbounded_channel();
        {
            let mut guard = TerminalGuard::new(tx, "m1".to_string());
            guard.disarm();
        }
        assert!(
            rx.try_recv().is_err(),
            "a disarmed guard must not emit any event on drop"
        );
    }

    #[tokio::test]
    async fn a_panicking_turn_task_still_delivers_a_terminal_event() {
        // The D1 contract end-to-end: a `tokio::spawn`ed turn body that
        // PANICS (the engine-panic case) must still deliver a terminal
        // event to the bridge channel via the `TerminalGuard`'s `Drop`.
        // Without the guard the panic unwinds past every `tx.send` and
        // `streaming_active` stays stuck `true` forever.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            let _term = TerminalGuard::new(tx, "m-panic".to_string());
            // The engine run future panics mid-turn (a poisoned
            // `std::sync::Mutex`, an `.expect`, …).
            panic!("simulated engine panic mid-turn");
        });
        // The task ends as a panic — the join result is an `Err`.
        assert!(handle.await.is_err(), "the task must report a panic");

        // …but the bridge still received a terminal event pair, so the
        // TUI recovers instead of hanging on the spinner.
        let first = rx.try_recv().expect("a panicked turn must emit Error");
        assert!(
            matches!(first, ProtocolEvent::Error { .. }),
            "expected an Error event, got {first:?}"
        );
        let second = rx.try_recv().expect("a panicked turn must emit StreamEnd");
        assert!(
            matches!(second, ProtocolEvent::StreamEnd { .. }),
            "expected a StreamEnd event, got {second:?}"
        );
    }

    #[tokio::test]
    async fn an_aborted_turn_task_still_delivers_a_terminal_event() {
        // The other D1 exit path: `JoinHandle::abort()` drops the task at
        // its current `.await`. The `TerminalGuard` is dropped with it
        // and must still deliver a terminal event so a hard cancel never
        // strands the spinner either.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            let _term = TerminalGuard::new(tx, "m-abort".to_string());
            // Park forever — only the abort below stops this task.
            std::future::pending::<()>().await;
        });
        // Give the task a tick to install the guard, then abort it.
        tokio::task::yield_now().await;
        handle.abort();
        assert!(handle.await.is_err(), "the aborted task must report Err");

        let first = rx.try_recv().expect("an aborted turn must emit Error");
        assert!(matches!(first, ProtocolEvent::Error { .. }));
        let second = rx.try_recv().expect("an aborted turn must emit StreamEnd");
        assert!(matches!(second, ProtocolEvent::StreamEnd { .. }));
    }

    // ── Approval round-trip ──────────────────────────────────────────
    //
    // `TuiEngine::approve` / `deny` forward to the shared
    // `ToolApprovalManager`. The manager is the same instance the
    // engine's `ApprovalChannel` awaits against, so resolving a pending
    // request through it is exactly what unblocks an in-flight tool.
    // These tests exercise that contract at the manager seam — the layer
    // `TuiEngine` is a thin wrapper over — without needing a live
    // `AgentEngine` to construct.

    #[tokio::test]
    async fn approval_manager_approve_resolves_a_pending_request() {
        use wcore_protocol::commands::ApprovalScope;
        use wcore_protocol::events::ToolCategory;

        let manager = Arc::new(ToolApprovalManager::new());
        // The engine's orchestration path registers a pending request
        // when a tool needs approval; the TUI then approves it.
        let rx = manager.request_approval("call-1", &ToolCategory::Exec, "Bash");
        manager.approve("call-1", ApprovalScope::Once, None);
        let result = rx.await.expect("the oneshot resolves on approve");
        assert!(matches!(result, ToolApprovalResult::Approved { .. }));
    }

    #[tokio::test]
    async fn approval_manager_deny_resolves_with_a_reason() {
        use wcore_protocol::events::ToolCategory;

        let manager = Arc::new(ToolApprovalManager::new());
        let rx = manager.request_approval("call-2", &ToolCategory::Edit, "Write");
        manager.resolve(
            "call-2",
            ToolApprovalResult::Denied {
                reason: "not now".to_string(),
            },
        );
        match rx.await.expect("the oneshot resolves on deny") {
            ToolApprovalResult::Denied { reason } => assert_eq!(reason, "not now"),
            ToolApprovalResult::Approved { .. } => panic!("expected a denial"),
        }
    }

    #[test]
    fn approval_manager_set_mode_auto_approves_per_mode() {
        use wcore_protocol::commands::SessionMode;

        let manager = ToolApprovalManager::new();
        // `Force` auto-approves every category.
        manager.set_mode(SessionMode::Force);
        assert!(manager.is_auto_approved("exec"));
        // `AutoEdit` auto-approves `edit` + `info`, not `exec`.
        manager.set_mode(SessionMode::AutoEdit);
        assert!(manager.is_auto_approved("edit"));
        assert!(!manager.is_auto_approved("exec"));
        // `Default` auto-approves nothing.
        manager.set_mode(SessionMode::Default);
        assert!(!manager.is_auto_approved("edit"));
    }

    // ── v0.9.1.1 F1 regression: ChannelSink fallback emit_tool_*
    //    must NOT leak Info events into the transcript ────────────────────
    //
    // Live visual e2e found the transcript dumping the raw provider JSON
    // envelope after every tool call (`[web success] {"web":[{...}]}`).
    // Root cause: `ChannelSink::emit_tool_result` sent a
    // `ProtocolEvent::Info` carrying `content` verbatim. The proper
    // structured `ToolResult` event (with `call_id`) is already emitted
    // by `orchestration::execute_tools` and consumed by the bridge into a
    // compact `ToolCardModel`, so this fallback is pure noise in TUI
    // mode. The fix makes both `emit_tool_call` and `emit_tool_result`
    // no-ops on the channel sink (routed to `tracing::debug!` only).

    #[test]
    fn channel_sink_emit_tool_call_does_not_send_info_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        // Fallback path the engine fires unconditionally from
        // `agent::engine::stream` for every tool call.
        OutputSink::emit_tool_call(&sink, "web", r#"{"query":"rust ownership"}"#);
        // No event must reach the TUI channel — the proper `ToolRequest`
        // (with call_id) is the authoritative path.
        assert!(
            rx.try_recv().is_err(),
            "emit_tool_call must not enqueue any ProtocolEvent on the TUI channel"
        );
    }

    #[test]
    fn repomap_summary_reports_real_counts_and_honest_empty_state() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        // A tiny Rust file with two extractable symbols.
        let mut f = std::fs::File::create(dir.path().join("lib.rs")).unwrap();
        writeln!(f, "pub fn alpha() {{}}\npub struct Beta;").unwrap();
        let map = wcore_repomap::RepoMap::build(dir.path()).unwrap();
        let summary = format_repomap_summary(&map);
        assert!(summary.contains("Indexed 1 files"), "got: {summary}");
        assert!(summary.contains("1 Rust"), "got: {summary}");
        assert!(
            summary.contains("lib.rs"),
            "densest-files list missing: {summary}"
        );
        assert!(summary.contains("Fresh scan"), "got: {summary}");

        // An empty map → the honest "no source files" line, never a fake 0-row.
        let empty = wcore_repomap::RepoMap::empty(dir.path().to_path_buf());
        assert!(format_repomap_summary(&empty).contains("no source files"));
    }

    #[test]
    fn channel_sink_emit_tool_result_does_not_leak_json_envelope() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = ChannelSink::new(tx);
        // Simulate a real web tool result payload — the live e2e saw
        // exactly this shape dumped into the transcript.
        let payload = r#"{"web":[{"snippet":"Learn Rust online","title":"Rust Programming Language","url":"https://example.com/rust","metadata":{}}]}"#;
        OutputSink::emit_tool_result(&sink, "web", false, payload);
        // Authoritative path is the structured `ToolResult` event with
        // `call_id` from orchestration. The fallback emit must produce
        // ZERO transcript events — in particular no `Info` carrying
        // raw JSON keys like `"snippet"` or `"url"`.
        if let Ok(event) = rx.try_recv() {
            if let ProtocolEvent::Info { message, .. } = &event {
                assert!(
                    !message.contains("\"snippet\":"),
                    "emit_tool_result leaked raw JSON payload into Info: {message}"
                );
                assert!(
                    !message.contains("\"url\":"),
                    "emit_tool_result leaked raw JSON payload into Info: {message}"
                );
            }
            panic!("emit_tool_result must not enqueue any ProtocolEvent: {event:?}");
        }
    }
}
