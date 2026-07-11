//! THE shared engine-session bridge for the ACP/REST transports.
//!
//! `wcore-acp` is a mid-layer crate and must not depend on `wcore-agent`
//! (the engine). The ACP server therefore reaches the engine through the
//! [`wcore_acp::turn::TurnEngine`] trait, implemented here in `wcore-cli`
//! (which already owns `wcore-agent`) exactly like `A2aHandler`.
//!
//! Three pieces live here:
//!  * [`EngineSession`] — owns ONE `AgentEngine` session and drives one
//!    prompt turn into a stream of ACP [`MessageEvent`]s. This is the single
//!    place engine output is projected onto the ACP surface; both the ACP
//!    transport (via [`EngineTurnEngine`]) and the A2A handler
//!    ([`EngineA2aHandler`]) consume it, so the engine-driving logic exists
//!    once.
//!  * [`EngineTurnEngine`] — the `TurnEngine` impl. Holds a per-`session_id`
//!    pool of `EngineSession`s and the inputs to build a fresh one on first
//!    use.
//!  * [`EngineA2aHandler`] — an engine-backed `A2aHandler` whose `on_message`
//!    routes a task through the same bridge and whose `capabilities()`
//!    reports the engine's real tool catalog.
//!
//! The engine is **sink-driven**: it takes an `OutputSink` at build time and
//! a `ProtocolEmitter` via `set_protocol_writer`, then emits streaming/tool
//! events through them while `run()` drives the turn. `run()` never emits a
//! terminal `StreamEnd` — the caller synthesizes it. Because each turn needs
//! its own `ProtocolEvent` channel (so concurrent/serial turns don't
//! cross-talk) but the engine is built once per session, the sink/emitter are
//! **relays** ([`RelaySink`]/[`RelayEmitter`]) that forward to whichever
//! channel `run_turn` installs for the current turn. This reuses the proven
//! [`ChannelSink`]/[`ChannelEmitter`] per-event mapping verbatim — no
//! duplicate `ProtocolEvent` construction.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::stream::Stream;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use wcore_acp::AcpError;
use wcore_acp::a2a::{A2aCapabilities, A2aError, A2aHandler, A2aHandshake, A2aMessage};
use wcore_acp::protocol::{ErrorCode, JsonRpcError, MessageEvent, ToolCall, ToolResult};
use wcore_acp::turn::{TurnEngine, TurnRequest};

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::engine::{AgentEngine, AgentError, AgentResult};
use wcore_agent::output::OutputSink;
use wcore_config::config::Config;
use wcore_protocol::ToolApprovalManager;
use wcore_protocol::events::{FinishReason, ProtocolEvent, ToolStatus};
use wcore_protocol::writer::ProtocolEmitter;

use crate::tui::{ChannelEmitter, ChannelSink};

// ─────────────────────────────────────────────────────────────────────────
// Relay sink/emitter — forward to the current turn's channel
// ─────────────────────────────────────────────────────────────────────────

/// The swappable sender both relays forward on. `EngineSession::run_turn`
/// installs a fresh sender for each turn (under the engine lock, so a queued
/// turn cannot redirect a running one), so the single engine — built once per
/// session — drives a fresh channel per turn without cross-talk.
type RelayHandle = Arc<Mutex<Option<UnboundedSender<ProtocolEvent>>>>;

/// Approval TTL for DEFAULT-posture ACP/REST sessions.
///
/// Wave 6 #31 set this to an interim 30s fail-fast because the transports had
/// no approval-resolve endpoint, so a gated `ApprovalRequired` could not be
/// answered and a long TTL meant a 5-minute silent stall. Blocker #2 added
/// `POST /v1/sessions/{id}/approvals/{call_id}/resolve` (wired through
/// `EngineTurnEngine::resolve_approval` → the session's `ToolApprovalManager`),
/// so a host CAN now answer the gate. The TTL is therefore restored to the
/// host-facing 300s window: enough time for an operator/UI to respond, with the
/// reaper still auto-denying a genuinely-abandoned gate. Force sessions never
/// gate and keep the default TTL regardless.
const DEFAULT_POSTURE_API_APPROVAL_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// An [`OutputSink`] that forwards every call onto the relay's current
/// sender by delegating to a [`ChannelSink`] bound to it. Reuses the TUI
/// bridge's exact `ProtocolEvent` mapping — no re-implementation.
struct RelaySink {
    handle: RelayHandle,
}

impl RelaySink {
    fn new(handle: RelayHandle) -> Self {
        Self { handle }
    }

    /// Run `f` against a `ChannelSink` bound to the current sender, if any.
    /// When no turn is active the call is dropped (the engine should not emit
    /// outside a turn, but a dropped event is never an error).
    fn with_sink<F: FnOnce(&ChannelSink)>(&self, f: F) {
        let tx = self.handle.lock().unwrap().clone();
        if let Some(tx) = tx {
            f(&ChannelSink::new(tx));
        }
    }
}

impl OutputSink for RelaySink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.with_sink(|s| s.emit_text_delta(text, msg_id));
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.with_sink(|s| s.emit_thinking(text, msg_id));
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        self.with_sink(|s| s.emit_tool_call(name, input));
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        self.with_sink(|s| s.emit_tool_result(name, is_error, content));
    }
    fn emit_stream_start(&self, msg_id: &str) {
        self.with_sink(|s| s.emit_stream_start(msg_id));
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
    ) {
        self.with_sink(|s| {
            s.emit_stream_end(
                msg_id,
                turns,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                finish_reason,
            )
        });
    }
    fn emit_error(&self, msg: &str, retryable: bool) {
        self.with_sink(|s| s.emit_error(msg, retryable));
    }
    fn emit_info(&self, msg: &str) {
        self.with_sink(|s| s.emit_info(msg));
    }
    fn emit_trace(&self, msg_id: &str, trace_json: &serde_json::Value) {
        self.with_sink(|s| s.emit_trace(msg_id, trace_json));
    }
    fn streaming_tools_advertised(&self) -> bool {
        true
    }
    fn emit_tool_chunk(&self, msg_id: &str, call_id: &str, tool_name: &str, chunk: &str) {
        self.with_sink(|s| s.emit_tool_chunk(msg_id, call_id, tool_name, chunk));
    }
    fn emit_sub_agent_event(
        &self,
        parent_call_id: &str,
        agent_name: &str,
        inner: &serde_json::Value,
    ) {
        self.with_sink(|s| s.emit_sub_agent_event(parent_call_id, agent_name, inner));
    }
    fn emit_session_cost(&self, session_id: &str, cost_payload: &serde_json::Value) {
        self.with_sink(|s| s.emit_session_cost(session_id, cost_payload));
    }
    fn emit_provider_circuit_event(
        &self,
        primary: &str,
        fallback: Option<&str>,
        state: &str,
        error: Option<&str>,
    ) {
        self.with_sink(|s| s.emit_provider_circuit_event(primary, fallback, state, error));
    }
    fn emit_approval_required(
        &self,
        call_id: &str,
        resume_token: &str,
        reason: &str,
        context: &str,
    ) {
        self.with_sink(|s| s.emit_approval_required(call_id, resume_token, reason, context));
    }
    fn emit_suspend(&self, reason: &str, resume_token: &str) {
        self.with_sink(|s| s.emit_suspend(reason, resume_token));
    }
    fn emit_approval_resume(&self, resume_token: &str, approved: bool) {
        self.with_sink(|s| s.emit_approval_resume(resume_token, approved));
    }
    fn emit_budget_exceeded(&self, reason: &str, observed: &str, limit: &str) {
        self.with_sink(|s| s.emit_budget_exceeded(reason, observed, limit));
    }
    fn emit_tool_panicked(
        &self,
        msg_id: &str,
        call_id: &str,
        tool_name: &str,
        panic_message: &str,
    ) {
        self.with_sink(|s| s.emit_tool_panicked(msg_id, call_id, tool_name, panic_message));
    }
    fn emit_plugin_registration_failed(
        &self,
        plugin_name: &str,
        surface: &str,
        error_kind: &str,
        message: &str,
    ) {
        self.with_sink(|s| {
            s.emit_plugin_registration_failed(plugin_name, surface, error_kind, message)
        });
    }
}

/// A [`ProtocolEmitter`] that forwards onto the relay's current sender by
/// delegating to a [`ChannelEmitter`] bound to it. Inherits the
/// `ApprovalRequired`-after-`ToolRequest` synthesis from `ChannelEmitter`.
struct RelayEmitter {
    handle: RelayHandle,
    /// Wave 6 #24 — persistent per-session "already-synthesized a gate frame"
    /// set. `ChannelEmitter` is rebuilt per emit here (the active `tx` swaps per
    /// turn), so the dedupe state must live on the relay, not the throwaway
    /// emitter. Threaded into `ChannelEmitter::with_dedupe` so a later explicit
    /// `ApprovalRequired` (the live-workflow gate emits its own) is suppressed,
    /// preventing a malformed second gate frame on the ACP projection.
    synthesized: crate::tui::DedupeSet,
    /// GHSA-8r7g M1 (wayland#497) — the session engine's `ApprovalBridge`.
    /// Threaded into `ChannelEmitter::with_dedupe` so a bridge-backed
    /// approval (Crucible council / egress consent) synthesized on the ACP
    /// relay stamps the server-generated SECRET `apr-{uuid}` resume_token
    /// instead of the legacy model-known `call_id`. `None` only in unit
    /// tests.
    ///
    /// SCOPE (per the #497 cross-audit): this is frame-level parity with the
    /// stdin/TUI transports only. The ACP PROJECTION drops `resume_token`
    /// before it reaches the host (`MessageEvent::ApprovalRequired` carries
    /// no token field) and the resolve endpoint stays call_id-keyed behind
    /// X-API-Key — so a bridge-backed gate raised during an ACP turn is not
    /// yet resolvable by the ACP host at all, and manager-gated resolution
    /// still keys on the model-known call_id. Carrying + accepting the
    /// secret end-to-end on ACP is tracked as FerroxLabs/wayland#568.
    approval_bridge: Option<Arc<wcore_agent::approval::ApprovalBridge>>,
}

impl RelayEmitter {
    fn new(
        handle: RelayHandle,
        approval_bridge: Option<Arc<wcore_agent::approval::ApprovalBridge>>,
    ) -> Self {
        Self {
            handle,
            synthesized: Arc::new(Mutex::new(std::collections::HashSet::new())),
            approval_bridge,
        }
    }
}

impl ProtocolEmitter for RelayEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        let tx = self.handle.lock().unwrap().clone();
        if let Some(tx) = tx {
            ChannelEmitter::with_dedupe(tx, self.synthesized.clone(), self.approval_bridge.clone())
                .emit(event)?;
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────
// TerminalGuard — guarantees a terminal ProtocolEvent even on panic/abort
// ─────────────────────────────────────────────────────────────────────────

/// A turn-task guard that guarantees a terminal `ProtocolEvent` reaches the
/// projection even if the spawned `run()` task panics or is aborted before a
/// normal terminal frame is sent. Without it an aborted turn strands the ACP
/// client waiting for a terminal frame forever (the 35-minute-hang class).
/// Mirrors `tui::engine_bridge::TerminalGuard`; cloned locally rather than
/// widening the TUI type's visibility.
struct TerminalGuard {
    tx: UnboundedSender<ProtocolEvent>,
    msg_id: String,
    armed: bool,
}

impl TerminalGuard {
    fn new(tx: UnboundedSender<ProtocolEvent>, msg_id: String) -> Self {
        Self {
            tx,
            msg_id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = self.tx.send(ProtocolEvent::Error {
            msg_id: Some(self.msg_id.clone()),
            error: wcore_protocol::events::ErrorInfo {
                code: "engine_panic".to_string(),
                message: "The turn ended unexpectedly (engine task panicked or was aborted)."
                    .to_string(),
                retryable: true,
            },
        });
        let _ = self.tx.send(ProtocolEvent::StreamEnd {
            msg_id: std::mem::take(&mut self.msg_id),
            finish_reason: FinishReason::Error,
            usage: None,
            usage_delta: None,
            agent_run_id: None,
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────
// ProtocolEvent -> MessageEvent projection
// ─────────────────────────────────────────────────────────────────────────

/// Map a protocol [`FinishReason`] onto an ACP StopReason string. `cancelled`
/// is produced by other paths, not by `FinishReason`.
fn stop_reason_str(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "end_turn",
        FinishReason::Length => "max_tokens",
        FinishReason::Error => "refusal",
        // #457: the engine's per-turn cap maps to the ACP `max_turn_requests`
        // stop reason so an ACP client can offer Continue, not a refusal.
        FinishReason::MaxTurns => "max_turn_requests",
    }
}

/// Build the terminal `StreamEnd` for a successful turn from its result.
fn stream_end_event(msg_id: &str, result: &AgentResult) -> ProtocolEvent {
    ProtocolEvent::StreamEnd {
        msg_id: msg_id.to_string(),
        finish_reason: result.finish_reason,
        usage: Some(wcore_protocol::events::Usage {
            input_tokens: result.usage.input_tokens,
            output_tokens: result.usage.output_tokens,
            cache_read_tokens: (result.usage.cache_read_tokens > 0)
                .then_some(result.usage.cache_read_tokens),
            cache_write_tokens: (result.usage.cache_creation_tokens > 0)
                .then_some(result.usage.cache_creation_tokens),
            active_window_percent: result.active_window_percent,
        }),
        // CORE-2: run-scoped delta rides beside the cumulative usage.
        usage_delta: Some(wcore_protocol::events::Usage {
            input_tokens: result.usage_delta.input_tokens,
            output_tokens: result.usage_delta.output_tokens,
            cache_read_tokens: (result.usage_delta.cache_read_tokens > 0)
                .then_some(result.usage_delta.cache_read_tokens),
            cache_write_tokens: (result.usage_delta.cache_creation_tokens > 0)
                .then_some(result.usage_delta.cache_creation_tokens),
            active_window_percent: None,
        }),
        agent_run_id: result.agent_run_id.clone(),
    }
}

/// Map an [`AgentError`] from the OUTER `run()` `Err` arm onto an
/// `ErrorInfo`. The honest retryable flag for retryable provider failures is
/// emitted IN-BAND by the engine via `emit_error` before `run()` returns, so
/// this outer mapping must NOT overwrite it; it is the catch-all for
/// `AgentError` variants that bypassed the classifier. All map to
/// `retryable: false` per the engine-seam contract.
fn error_info_for(e: &AgentError) -> wcore_protocol::events::ErrorInfo {
    wcore_protocol::events::ErrorInfo {
        code: "engine_error".to_string(),
        message: e.to_string(),
        // UserAborted / ContextTooLong / ApiError(honest already fired) /
        // Provider => not retryable at this outer arm.
        retryable: false,
    }
}

/// A [`Stream`] that drains a `ProtocolEvent` receiver and projects each
/// event onto zero-or-one ACP [`MessageEvent`]. Terminates (returns
/// `Poll::Ready(None)`) immediately after yielding a terminal `Done`/`Error`
/// so nothing follows the terminal frame.
struct ProtocolToMessageStream {
    rx: UnboundedReceiver<ProtocolEvent>,
    done: bool,
    /// D012: correlate a `ProtocolEvent::ApprovalRequired` back to the
    /// `ToolRequest` that triggered it. The protocol `ApprovalRequired` carries
    /// only `call_id` (not the tool name/input), but a host needs the gated
    /// call's identity to render the approval. We remember each `ToolRequest`'s
    /// `(name, input)` by `call_id` so the gate frame projects a faithful
    /// `ToolCall`. The map is bounded by the in-flight tool calls of one turn.
    pending_calls: HashMap<String, (String, serde_json::Value)>,
}

impl ProtocolToMessageStream {
    fn new(rx: UnboundedReceiver<ProtocolEvent>) -> Self {
        Self {
            rx,
            done: false,
            pending_calls: HashMap::new(),
        }
    }

    /// Project one `ProtocolEvent` onto zero-or-one [`MessageEvent`]. Returns
    /// `None` to swallow the event; sets `done` when the frame is terminal.
    fn project(&mut self, ev: ProtocolEvent) -> Option<MessageEvent> {
        match ev {
            ProtocolEvent::TextDelta { text, .. } => Some(MessageEvent::TextDelta { text }),
            ProtocolEvent::Thinking { text, .. } => Some(MessageEvent::Thinking { text }),
            ProtocolEvent::ToolRequest { call_id, tool, .. } => {
                // D012: remember the call so the matching `ApprovalRequired`
                // (synthesized by the relay's `ChannelEmitter`) can project a
                // faithful `ToolCall` identity to the host.
                self.pending_calls
                    .insert(call_id.clone(), (tool.name.clone(), tool.args.clone()));
                Some(MessageEvent::ToolCall {
                    call: ToolCall {
                        id: call_id,
                        name: tool.name,
                        input: tool.args,
                    },
                })
            }
            ProtocolEvent::ToolResult {
                call_id,
                status,
                output,
                ..
            } => Some(MessageEvent::ToolResult {
                result: ToolResult {
                    call_id,
                    output: serde_json::Value::String(output),
                    is_error: matches!(status, ToolStatus::Error),
                },
            }),
            ProtocolEvent::ToolCancelled {
                call_id, reason, ..
            } => Some(MessageEvent::ToolResult {
                result: ToolResult {
                    call_id,
                    output: serde_json::Value::String(reason),
                    is_error: true,
                },
            }),
            ProtocolEvent::ToolPanicked {
                call_id,
                panic_message,
                ..
            } => Some(MessageEvent::ToolResult {
                result: ToolResult {
                    call_id,
                    output: serde_json::Value::String(panic_message),
                    is_error: true,
                },
            }),
            ProtocolEvent::StreamEnd { finish_reason, .. } => {
                self.done = true;
                Some(MessageEvent::Done {
                    stop_reason: stop_reason_str(finish_reason).to_string(),
                })
            }
            ProtocolEvent::Error { error, .. } => {
                self.done = true;
                Some(MessageEvent::Error {
                    error: JsonRpcError {
                        code: ErrorCode::ToolFailed.code(),
                        message: error.message,
                        // Preserve the honest retryable flag for hosts that
                        // inspect it.
                        data: Some(serde_json::json!({ "retryable": error.retryable })),
                    },
                })
            }
            // D012 (P0 security): surface the approval gate to REST/SSE/ACP
            // hosts instead of swallowing it. The engine emits this only when a
            // mutating tool is parked awaiting approval (default posture); a
            // host must see the gate frame, not just a bare `ToolCall` that is
            // indistinguishable from an already-approved call. Project a
            // faithful `ToolCall` identity using the `ToolRequest` we recorded
            // for this `call_id` (falling back to the id alone if the request
            // was not seen, e.g. an engine site that emits `ApprovalRequired`
            // without a preceding `ToolRequest`).
            ProtocolEvent::ApprovalRequired {
                call_id,
                reason,
                resume_token,
                ..
            } => {
                let (name, input) = self
                    .pending_calls
                    .remove(&call_id)
                    .unwrap_or_else(|| (String::new(), serde_json::Value::Null));
                // #568 (B): carry the SECRET resume_token to the host instead of
                // dropping it. Bridge-backed gates stamp a real `apr-` token
                // (via the RelayEmitter's ApprovalBridge); manager-gated tools
                // carry an empty token and are resolved by `call_id`.
                Some(MessageEvent::ApprovalRequired {
                    call: ToolCall {
                        id: call_id,
                        name,
                        input,
                    },
                    reason,
                    resume_token,
                })
            }
            // StreamStart / ToolRunning / ToolChunk / Suspend / ApprovalResume /
            // Info / traces / costs etc. have no faithful ACP `MessageEvent`
            // analogue — swallow them.
            _ => None,
        }
    }
}

impl Stream for ProtocolToMessageStream {
    type Item = MessageEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        loop {
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(ev)) => {
                    if let Some(out) = self.project(ev) {
                        return Poll::Ready(Some(out));
                    }
                    // Swallowed event — keep draining without yielding.
                    continue;
                }
                // Channel closed without a terminal frame. The TerminalGuard
                // normally prevents this, but be defensive and just end.
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// EngineSession — THE shared bridge
// ─────────────────────────────────────────────────────────────────────────

/// One live engine session. Built once per ACP `session_id`, then
/// [`run_turn`](Self::run_turn) is called per prompt. The engine is behind an
/// async mutex because `run()` takes `&mut self` and a session may receive
/// serialized turns.
pub struct EngineSession {
    engine: Arc<AsyncMutex<AgentEngine>>,
    approval_manager: Arc<ToolApprovalManager>,
    /// GHSA-8r7g M2 (#568) — the engine's `ApprovalBridge`, captured at build
    /// time so `resolve_approval` can answer a bridge-backed gate by SECRET
    /// resume_token WITHOUT locking the engine mutex (a live turn holds it).
    approval_bridge: Arc<wcore_agent::approval::ApprovalBridge>,
    /// The relay handle the engine's `RelaySink`/`RelayEmitter` forward on.
    /// `run_turn` installs this turn's channel into it.
    relay: RelayHandle,
    /// Tool names the engine registered, captured at build time for the A2A
    /// capability catalog.
    tool_names: Vec<String>,
}

impl EngineSession {
    /// Wire an already-bootstrapped engine (built with a [`RelaySink`] as its
    /// `OutputSink` and a [`RelayEmitter`] installed via
    /// `set_protocol_writer`) into a session. `relay` MUST be the same handle
    /// both relays were constructed with.
    fn new(
        engine: AgentEngine,
        approval_manager: Arc<ToolApprovalManager>,
        relay: RelayHandle,
    ) -> Self {
        let tool_names = engine.tools().tool_names();
        let approval_bridge = engine.approval_bridge().clone();
        Self {
            engine: Arc::new(AsyncMutex::new(engine)),
            approval_manager,
            approval_bridge,
            relay,
            tool_names,
        }
    }

    /// The shared approval manager (so a future host permission channel can
    /// resolve approvals).
    pub fn approval_manager(&self) -> Arc<ToolApprovalManager> {
        self.approval_manager.clone()
    }

    /// The engine's registered tool names (for A2A capabilities).
    pub fn tool_names(&self) -> Vec<String> {
        self.tool_names.clone()
    }

    /// Drive ONE prompt turn. Returns a stream that ends with exactly one
    /// terminal `MessageEvent` (`Done` | `Error`). Mirrors
    /// `TuiEngine::submit` + the json-stream `run()` synthesis, but projects
    /// to `MessageEvent` and installs a per-turn relay channel.
    pub async fn run_turn(
        &self,
        text: String,
        msg_id: String,
    ) -> Pin<Box<dyn Stream<Item = MessageEvent> + Send>> {
        let (tx, rx) = unbounded_channel::<ProtocolEvent>();

        let engine = self.engine.clone();
        let relay = self.relay.clone();
        let turn_cancel = CancellationToken::new();

        tokio::spawn(async move {
            // Guarantee a terminal event even on panic/abort.
            let mut term = TerminalGuard::new(tx.clone(), msg_id.clone());
            let mut guard = engine.lock().await;
            // Point the engine's relay sink/emitter at THIS turn's channel
            // only after acquiring the engine lock, so a second `run_turn`
            // queued behind us cannot redirect our turn's events: the swap
            // and the `run()` it scopes are both under the same lock.
            *relay.lock().unwrap() = Some(tx.clone());
            guard.set_cancel_token(turn_cancel.clone());
            match guard.run(&text, &msg_id).await {
                Ok(result) => {
                    if result.finish_reason == FinishReason::Error {
                        // FerroxLabs/wayland#200: a turn can end Ok with finish_reason=Error when
                        // the provider returned a Done event carrying an unrecognized/empty
                        // finish_reason (mapped to FinishReason::Error) — e.g. an OpenAI model
                        // whose finish_reason string the engine doesn't map yet. The engine
                        // classifies that as success, so without this the host would emit a
                        // contentless stream_end and the turn would fail SILENTLY. Surface it.
                        let _ = tx.send(ProtocolEvent::Error {
                            msg_id: Some(msg_id.clone()),
                            error: wcore_protocol::events::ErrorInfo {
                                code: "finish_reason_error".to_string(),
                                message: "The model ended the turn with an error and no output \
                                    (finish_reason=error). The provider likely returned an empty \
                                    response or an unrecognized completion status. Check the engine \
                                    log for an 'unrecognized finish_reason' warning, and verify the \
                                    model name and provider."
                                    .to_string(),
                                retryable: false,
                            },
                        });
                    }
                    let _ = tx.send(stream_end_event(&msg_id, &result));
                    term.disarm();
                }
                Err(e) => {
                    // Outer Err arm: do NOT clobber any honest in-band
                    // `emit_error` the engine already fired (it rides the
                    // relay channel ahead of this). Emit the catch-all
                    // mapping then the terminal StreamEnd.
                    let _ = tx.send(ProtocolEvent::Error {
                        msg_id: Some(msg_id.clone()),
                        error: error_info_for(&e),
                    });
                    let _ = tx.send(ProtocolEvent::StreamEnd {
                        msg_id: msg_id.clone(),
                        finish_reason: FinishReason::Error,
                        usage: None,
                        usage_delta: None,
                        agent_run_id: None,
                    });
                    term.disarm();
                }
            }
        });

        Box::pin(ProtocolToMessageStream::new(rx))
    }

    /// Drive one turn to completion and collect the streamed `TextDelta`
    /// frames into a single reply string (used by the synchronous A2A path).
    /// Returns `Err` if the turn ends in a terminal `Error` frame.
    pub async fn run_turn_collect(&self, text: String, msg_id: String) -> Result<String, String> {
        use futures::stream::StreamExt;
        let mut stream = self.run_turn(text, msg_id).await;
        let mut reply = String::new();
        while let Some(ev) = stream.next().await {
            match ev {
                MessageEvent::TextDelta { text } => reply.push_str(&text),
                MessageEvent::Error { error } => return Err(error.message),
                MessageEvent::Done { .. } => break,
                // Thinking / tool frames are not part of the A2A reply text.
                _ => {}
            }
        }
        Ok(reply)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// EngineTurnEngine — the TurnEngine impl (ACP transport)
// ─────────────────────────────────────────────────────────────────────────

/// The `TurnEngine` the ACP server is wired with. Holds the engine-session
/// pool keyed by ACP `session_id` plus the inputs to build a fresh
/// `EngineSession` on first use of a session id.
pub struct EngineTurnEngine {
    config: Config,
    cwd: String,
    sessions: Arc<AsyncMutex<HashMap<String, Arc<EngineSession>>>>,
    /// Optional pre-built provider injected for hermetic tests. When `Some`,
    /// `session_for` hands it to `AgentBootstrap::provider` instead of
    /// resolving one from config (which would require network). `None` in
    /// production — the resolved config drives the real provider.
    provider: Option<Arc<dyn wcore_providers::LlmProvider>>,
    /// Auto-approve ALL tool calls (Force mode) for API-originated sessions.
    /// **Off by default**: a network-exposed engine must not silently grant
    /// full tool execution (Bash/Write/Spawn) to anyone holding the API key.
    /// When off, tools that require approval simply do not auto-execute (the
    /// client-bound approval channel is a tracked follow-up). When on, the
    /// API key is root-equivalent and the operator opted into that.
    force_tools: bool,
}

impl EngineTurnEngine {
    /// Build a turn engine over a resolved [`Config`] and the working
    /// directory new sessions run in. Tool auto-approval is OFF by default;
    /// call [`Self::force_tools`] to opt in.
    pub fn new(config: Config, cwd: String) -> Self {
        Self {
            config,
            cwd,
            sessions: Arc::new(AsyncMutex::new(HashMap::new())),
            provider: None,
            force_tools: false,
        }
    }

    /// Opt into Force mode (auto-approve every tool call) for API sessions.
    /// Default is off. Turning this on makes the API key root-equivalent:
    /// any caller can run shell/file-mutating tools without approval. Only
    /// the operator launching the server should set this.
    pub fn force_tools(mut self, on: bool) -> Self {
        self.force_tools = on;
        self
    }

    /// Test/embedding seam: build a turn engine with a pre-created provider,
    /// bypassing config-driven provider resolution (which would hit the
    /// network). The hermetic ACP-turn integration test uses this to back the
    /// engine with a `MockLlm`-bound provider.
    pub fn with_provider(
        config: Config,
        cwd: String,
        provider: Arc<dyn wcore_providers::LlmProvider>,
    ) -> Self {
        Self {
            config,
            cwd,
            sessions: Arc::new(AsyncMutex::new(HashMap::new())),
            provider: Some(provider),
            // Embedding/test seam: the caller is in-process and trusted, so
            // tools auto-approve (hermetic turn tests drive real tool flow).
            force_tools: true,
        }
    }

    /// Fetch (or build + cache) the `EngineSession` for `session_id`. One
    /// engine per session preserves conversation history across turns.
    async fn session_for(&self, session_id: &str) -> Result<Arc<EngineSession>, AcpError> {
        {
            let pool = self.sessions.lock().await;
            if let Some(existing) = pool.get(session_id) {
                return Ok(existing.clone());
            }
        }

        // Build a fresh engine for this session. The relay handle is shared
        // by the sink, the emitter, and the session so `run_turn` can swap
        // the active channel per turn.
        let relay: RelayHandle = Arc::new(Mutex::new(None));

        // Tool-approval posture. Force (auto-approve every tool, including
        // Bash/Write/Spawn) is OPT-IN only: a network-exposed engine must not
        // silently grant shell/file execution to anyone holding the API key.
        // Default leaves the manager in its safe mode, where approval-required
        // tools do not auto-run.
        //
        // Under the default posture a gated mutating tool projects an
        // approvable `ApprovalRequired` gate frame to the host (D012). Blocker
        // #2 wired the matching answer path — `resolve_approval` below, backed
        // by `POST /v1/sessions/{id}/approvals/{call_id}/resolve` — so the host
        // can now resolve the gate. DEFAULT-posture sessions therefore use the
        // host-facing `DEFAULT_POSTURE_API_APPROVAL_TTL` (300s): a real window
        // for an operator/UI to respond, with the reaper still auto-denying a
        // genuinely-abandoned gate. Force sessions never gate, so they keep the
        // crate default TTL (no pending approval is ever created).
        let approval_manager = Arc::new(if self.force_tools {
            ToolApprovalManager::new()
        } else {
            ToolApprovalManager::with_ttl(DEFAULT_POSTURE_API_APPROVAL_TTL)
        });
        if self.force_tools {
            approval_manager.set_mode(wcore_protocol::commands::SessionMode::Force);
        }

        let output: Arc<dyn OutputSink> = Arc::new(RelaySink::new(relay.clone()));
        let mut bootstrap = AgentBootstrap::new(self.config.clone(), self.cwd.clone(), output);
        if let Some(provider) = &self.provider {
            bootstrap = bootstrap.provider(provider.clone());
        }
        let result = bootstrap
            .build()
            .await
            .map_err(|e| AcpError::Protocol(format!("engine bootstrap failed: {e}")))?;
        let mut engine = result.engine;

        let provider_name = self.config.provider_label.clone();
        engine
            .init_session(&provider_name, &self.cwd, Some(session_id))
            .map_err(|e| AcpError::Protocol(format!("engine init_session failed: {e}")))?;
        engine.rebind_memory_session().await;
        engine.run_session_start_hooks().await;
        engine.set_approval_manager(approval_manager.clone());
        // GHSA-8r7g M1 (wayland#497): bind the engine's ApprovalBridge so
        // bridge-backed gate frames on the ACP relay carry the secret
        // resume_token (parity with the stdin/TUI transports).
        let bridge = engine.approval_bridge().clone();
        engine.set_protocol_writer(Arc::new(RelayEmitter::new(relay.clone(), Some(bridge))));

        let session = Arc::new(EngineSession::new(engine, approval_manager, relay));

        let mut pool = self.sessions.lock().await;
        // Another turn may have built the session concurrently; keep the
        // first to preserve a single history.
        let entry = pool
            .entry(session_id.to_string())
            .or_insert_with(|| session.clone());
        Ok(entry.clone())
    }
}

#[async_trait]
impl TurnEngine for EngineTurnEngine {
    async fn run_turn(
        &self,
        req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        // Per-session `tools` override is stored on the ACP server but not
        // applied to the engine build in this MVP (documented follow-up); the
        // engine's full registry is the faithful default.
        let _ = &req.tools;
        let session = self.session_for(&req.session_id).await?;
        let msg_id = uuid::Uuid::new_v4().to_string();
        Ok(session.run_turn(req.text, msg_id).await)
    }

    async fn resolve_approval(
        &self,
        session_id: &str,
        call_id: &str,
        decision: wcore_acp::turn::ApprovalDecision,
    ) -> Result<(), AcpError> {
        // Look up the live session WITHOUT building one: a resolve for a
        // session that never ran a turn (so has no pending gate) must 404,
        // not silently spin up an engine. `session_for` would build-on-miss,
        // so consult the pool directly.
        let session = {
            let pool = self.sessions.lock().await;
            pool.get(session_id).cloned()
        };
        let Some(session) = session else {
            return Err(AcpError::Session(format!(
                "session not found: {session_id}"
            )));
        };

        // Map the transport-neutral wire scope onto the real protocol scope.
        // This is the only place the two vocabularies meet (wcore-acp has no
        // wcore-protocol dep by design).
        let scope = match decision.scope {
            wcore_acp::turn::ApprovalScopeWire::Once => {
                wcore_protocol::commands::ApprovalScope::Once
            }
            wcore_acp::turn::ApprovalScopeWire::Always => {
                wcore_protocol::commands::ApprovalScope::Always
            }
            wcore_acp::turn::ApprovalScopeWire::AlwaysPrefix { prefix } => {
                wcore_protocol::commands::ApprovalScope::AlwaysPrefix { prefix }
            }
        };

        // #568 (C): secret-preferred resolution. A BRIDGE-backed gate (Crucible
        // council / egress consent) minted a SECRET `apr-` resume_token and is
        // resolvable ONLY through the bridge; a manager-gated tool carries no
        // secret and resolves by `call_id`. When the host presents a non-empty
        // token, try the bridge first; fall through to the manager path if it
        // is not a live bridge token (stale, or actually a manager gate). The
        // ACP resolve carries no `modifications` payload (its `answer` threads
        // to the manager path), so bridge consent is a plain approve/deny.
        if let Some(token) = decision.resume_token.as_deref().filter(|t| !t.is_empty()) {
            let outcome = wcore_agent::approval::ApprovalOutcome {
                approved: decision.approved,
                modifications: None,
            };
            if session.approval_bridge.resolve(token, outcome).await {
                return Ok(());
            }
        }

        let resolved = session.approval_manager().resolve_host(
            call_id,
            decision.approved,
            scope,
            decision.answer,
        );
        if resolved {
            Ok(())
        } else {
            // Unknown / already-resolved / expired call_id. Reported as a
            // not-found session error so the REST layer maps it to 404 and the
            // endpoint stays idempotent (a second resolve is a clean 404).
            Err(AcpError::Session(format!(
                "approval not found (unknown, already resolved, or expired): {call_id}"
            )))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// EngineA2aHandler — engine-backed A2A handler
// ─────────────────────────────────────────────────────────────────────────

/// An engine-backed [`A2aHandler`]. Replaces `DefaultA2aHandler`'s echo:
/// `on_message` routes the task through the SAME `EngineSession` bridge and
/// returns the engine's reply text; `capabilities()` reports the engine's
/// real tool catalog. Reuses [`EngineTurnEngine`] so the engine-driving logic
/// is not duplicated.
pub struct EngineA2aHandler {
    agent_id: String,
    inner: EngineTurnEngine,
}

impl EngineA2aHandler {
    /// Build the handler over a resolved config + working directory.
    pub fn new(agent_id: impl Into<String>, config: Config, cwd: String) -> Self {
        Self {
            agent_id: agent_id.into(),
            inner: EngineTurnEngine::new(config, cwd),
        }
    }

    /// Test/embedding seam: build the handler over a pre-constructed
    /// [`EngineTurnEngine`] (e.g. one backed by an injected provider). The
    /// agent id defaults to `"wayland-core"`.
    pub fn with_engine(inner: EngineTurnEngine) -> Self {
        Self {
            agent_id: "wayland-core".to_string(),
            inner,
        }
    }

    /// Engine tool names for the A2A capability catalog. Lazily builds a
    /// session keyed on the agent id (the first build wins) and reads its
    /// registered tools.
    async fn tool_catalog(&self) -> Vec<String> {
        match self.inner.session_for(&self.agent_id).await {
            Ok(session) => session.tool_names(),
            Err(_) => Vec::new(),
        }
    }
}

#[async_trait]
impl A2aHandler for EngineA2aHandler {
    async fn on_handshake(&self, h: A2aHandshake) -> Result<A2aHandshake, A2aError> {
        // F-070 (defense-in-depth): an anonymous caller (empty agent_id)
        // receives ONLY agent_kind — no version, agent_id, or capabilities.
        if h.agent_id.is_empty() {
            return Ok(A2aHandshake {
                agent_id: String::new(),
                agent_kind: "wayland-core".to_string(),
                version: String::new(),
                capabilities: A2aCapabilities::default(),
            });
        }
        // Identified peer: return a real AgentCard-lite with the engine's
        // real tool catalog.
        let caps = A2aCapabilities {
            tools: self.tool_catalog().await,
            ..A2aCapabilities::default()
        };
        Ok(A2aHandshake {
            agent_id: self.agent_id.clone(),
            agent_kind: "wayland-core".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: caps,
        })
    }

    async fn on_message(&self, m: A2aMessage) -> Result<A2aMessage, A2aError> {
        // Route the task through the shared bridge. Derive a deterministic,
        // engine-valid session id from the peer's correlation id (so a
        // multi-message A2A exchange shares one engine history) WITHOUT
        // trusting the peer's arbitrary string to be a valid session id — the
        // engine requires 6-40 hex chars, and a peer can send anything. A
        // namespaced v5 UUID is stable per correlation id and always valid.
        let session_id = match &m.correlation_id {
            Some(corr) => {
                uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, corr.as_bytes()).to_string()
            }
            None => uuid::Uuid::new_v4().to_string(),
        };
        let session = self
            .inner
            .session_for(&session_id)
            .await
            .map_err(|e| A2aError::HandlerError(e.to_string()))?;
        let msg_id = uuid::Uuid::new_v4().to_string();
        let reply_text = session
            .run_turn_collect(m.text, msg_id)
            .await
            .map_err(A2aError::HandlerError)?;
        Ok(A2aMessage {
            from: self.agent_id.clone(),
            to: m.from,
            text: reply_text,
            attachments: vec![],
            correlation_id: m.correlation_id,
        })
    }

    async fn capabilities(&self) -> Result<A2aCapabilities, A2aError> {
        Ok(A2aCapabilities {
            tools: self.tool_catalog().await,
            ..A2aCapabilities::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::StreamExt;

    // ── Projection (T-A5) ──────────────────────────────────────────────

    /// Drive a `ProtocolToMessageStream` over a synthetic event sequence and
    /// collect the projected frames.
    async fn project_all(events: Vec<ProtocolEvent>) -> Vec<MessageEvent> {
        let (tx, rx) = unbounded_channel::<ProtocolEvent>();
        for ev in events {
            tx.send(ev).unwrap();
        }
        drop(tx);
        ProtocolToMessageStream::new(rx).collect().await
    }

    fn tool_info(name: &str) -> wcore_protocol::events::ToolInfo {
        wcore_protocol::events::ToolInfo {
            name: name.to_string(),
            category: wcore_protocol::events::ToolCategory::Info,
            args: serde_json::json!({"path": "x"}),
            description: "desc".to_string(),
        }
    }

    #[tokio::test]
    async fn projection_swallows_noise_and_terminates_after_done() {
        let events = vec![
            ProtocolEvent::StreamStart { msg_id: "m".into() },
            ProtocolEvent::TextDelta {
                text: "hi".into(),
                msg_id: "m".into(),
            },
            ProtocolEvent::ToolRequest {
                msg_id: "m".into(),
                call_id: "c1".into(),
                tool: tool_info("Read"),
            },
            ProtocolEvent::ToolRunning {
                msg_id: "m".into(),
                call_id: "c1".into(),
                tool_name: "Read".into(),
            },
            ProtocolEvent::ToolResult {
                msg_id: "m".into(),
                call_id: "c1".into(),
                tool_name: "Read".into(),
                status: ToolStatus::Error,
                output: "boom".into(),
                output_type: wcore_protocol::events::OutputType::Text,
                metadata: None,
            },
            ProtocolEvent::StreamEnd {
                msg_id: "m".into(),
                finish_reason: FinishReason::Length,
                usage: None,
                usage_delta: None,
                agent_run_id: None,
            },
            // Anything after the terminal frame must NOT appear.
            ProtocolEvent::TextDelta {
                text: "after".into(),
                msg_id: "m".into(),
            },
        ];
        let out = project_all(events).await;
        assert_eq!(out.len(), 4, "StreamStart + ToolRunning swallowed");
        assert!(matches!(out[0], MessageEvent::TextDelta { .. }));
        match &out[1] {
            MessageEvent::ToolCall { call } => {
                assert_eq!(call.id, "c1");
                assert_eq!(call.name, "Read");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        match &out[2] {
            MessageEvent::ToolResult { result } => {
                assert_eq!(result.call_id, "c1");
                assert!(result.is_error);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        match &out[3] {
            MessageEvent::Done { stop_reason } => assert_eq!(stop_reason, "max_tokens"),
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// D012 (P0 security, PART B) — a `ProtocolEvent::ApprovalRequired` for a
    /// gated mutating tool must project to `MessageEvent::ApprovalRequired`
    /// (not be swallowed), carrying the gated call's identity correlated from
    /// the preceding `ToolRequest`, so REST/SSE/ACP hosts receive the gate
    /// frame rather than only a bare `ToolCall`.
    #[tokio::test]
    async fn projection_surfaces_approval_required_with_correlated_call() {
        let events = vec![
            ProtocolEvent::ToolRequest {
                msg_id: "m".into(),
                call_id: "c9".into(),
                tool: tool_info("Write"),
            },
            ProtocolEvent::ApprovalRequired {
                call_id: "c9".into(),
                resume_token: "c9".into(),
                correlation_id: "c9".into(),
                reason: "edit".into(),
                context: "write a file".into(),
                plan: None,
            },
            ProtocolEvent::StreamEnd {
                msg_id: "m".into(),
                finish_reason: FinishReason::Stop,
                usage: None,
                usage_delta: None,
                agent_run_id: None,
            },
        ];
        let out = project_all(events).await;
        assert_eq!(out.len(), 3, "ToolCall + ApprovalRequired + Done");
        match &out[0] {
            MessageEvent::ToolCall { call } => assert_eq!(call.id, "c9"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
        match &out[1] {
            MessageEvent::ApprovalRequired {
                call,
                reason,
                resume_token,
            } => {
                assert_eq!(call.id, "c9", "gate frame correlates to the call");
                assert_eq!(
                    call.name, "Write",
                    "gate frame carries the gated tool name from the ToolRequest"
                );
                assert_eq!(reason, "edit");
                // #568 (B): the SECRET resume_token on the protocol event is
                // carried through to the host, not dropped by the projection.
                assert_eq!(
                    resume_token, "c9",
                    "the projection must carry resume_token end-to-end (#568)"
                );
            }
            other => panic!("expected ApprovalRequired, got {other:?}"),
        }
        match &out[2] {
            MessageEvent::Done { .. } => {}
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// Wave 6 #24 — a self-gating engine site (the live-workflow gate emits a
    /// `ToolRequest` AND its own explicit `ApprovalRequired` for one call_id)
    /// must yield EXACTLY ONE approvable `ApprovalRequired` MessageEvent through
    /// the `RelayEmitter` → projection path, not the malformed second gate frame
    /// (empty name / null input) the un-deduped synthesis produced. The single
    /// surviving gate frame carries the gated tool's identity so the host can
    /// render and answer it.
    #[tokio::test]
    async fn relay_emitter_dedupes_workflow_double_gate_to_single_approval() {
        let (tx, rx) = unbounded_channel::<ProtocolEvent>();
        // The relay's emitter forwards onto the per-turn channel and owns the
        // persistent dedupe set (mirrors `EngineSession`'s wiring).
        let relay: RelayHandle = Arc::new(Mutex::new(Some(tx)));
        let emitter = RelayEmitter::new(relay, None);

        // Reproduce the live-workflow gate's emit sequence (engine.rs):
        // (1) ToolRequest for the gated "Workflow" call, then
        // (2) the engine's OWN explicit ApprovalRequired for the same call_id.
        // The RelayEmitter synthesizes its ApprovalRequired after (1); (2) must
        // be suppressed so only one gate frame survives.
        emitter
            .emit(&ProtocolEvent::ToolRequest {
                msg_id: "m".into(),
                call_id: "wf1".into(),
                tool: wcore_protocol::events::ToolInfo {
                    name: "Workflow".into(),
                    category: wcore_protocol::events::ToolCategory::Exec,
                    args: serde_json::json!({"name": "demo"}),
                    description: "run a forgeflow".into(),
                },
            })
            .unwrap();
        emitter
            .emit(&ProtocolEvent::ApprovalRequired {
                call_id: "wf1".into(),
                resume_token: "wf1".into(),
                correlation_id: "wf1".into(),
                reason: "Run ForgeFlow `demo`?".into(),
                context: "~2 agents / ~$0.10".into(),
                plan: None,
            })
            .unwrap();
        emitter
            .emit(&ProtocolEvent::StreamEnd {
                msg_id: "m".into(),
                finish_reason: FinishReason::Stop,
                usage: None,
                usage_delta: None,
                agent_run_id: None,
            })
            .unwrap();

        // Close the relay's sender so the projection stream terminates.
        if let Some(tx) = emitter.handle.lock().unwrap().take() {
            drop(tx);
        }
        let out: Vec<MessageEvent> = ProtocolToMessageStream::new(rx).collect().await;

        // Exactly one ApprovalRequired, and it is well-formed (non-empty name).
        let gates: Vec<&MessageEvent> = out
            .iter()
            .filter(|e| matches!(e, MessageEvent::ApprovalRequired { .. }))
            .collect();
        assert_eq!(
            gates.len(),
            1,
            "the workflow double-gate must collapse to one ApprovalRequired, got {out:?}"
        );
        match gates[0] {
            MessageEvent::ApprovalRequired { call, .. } => {
                assert_eq!(call.id, "wf1");
                assert_eq!(
                    call.name, "Workflow",
                    "the surviving gate frame must carry the gated tool name, not an empty phantom"
                );
            }
            other => panic!("expected ApprovalRequired, got {other:?}"),
        }
    }

    /// GHSA-8r7g M1 (wayland#497): a bridge-backed approval synthesized on
    /// the ACP relay must stamp the bridge's SECRET `apr-{uuid}`
    /// resume_token — not the model-known `call_id` (the pre-fix legacy
    /// behavior this transport had) and not an empty token. Asserted at the
    /// raw `ProtocolEvent` boundary, where the wire value is decided.
    #[tokio::test]
    async fn relay_emitter_stamps_bridge_secret_resume_token() {
        let bridge = Arc::new(wcore_agent::approval::ApprovalBridge::new());
        let (secret, _outcome_rx) = bridge
            .request_with_id(
                "crucible-call-9".to_string(),
                wcore_agent::approval::ApprovalRequest {
                    call_id: "crucible-call-9".into(),
                    reason: "council".into(),
                    context: "ctx".into(),
                },
            )
            .await;

        let (tx, mut rx) = unbounded_channel::<ProtocolEvent>();
        let relay: RelayHandle = Arc::new(Mutex::new(Some(tx)));
        let emitter = RelayEmitter::new(relay, Some(bridge));

        emitter
            .emit(&ProtocolEvent::ToolRequest {
                msg_id: "m".into(),
                call_id: "crucible-call-9".into(),
                tool: wcore_protocol::events::ToolInfo {
                    name: "CrucibleCouncil".into(),
                    category: wcore_protocol::events::ToolCategory::Exec,
                    args: serde_json::json!({}),
                    description: "council approval".into(),
                },
            })
            .unwrap();

        // First frame is the forwarded ToolRequest; second is the
        // synthesized gate whose resume_token is under test.
        let first = rx.recv().await.expect("forwarded ToolRequest");
        assert!(matches!(first, ProtocolEvent::ToolRequest { .. }));
        match rx.recv().await.expect("synthesized ApprovalRequired") {
            ProtocolEvent::ApprovalRequired {
                call_id,
                resume_token,
                ..
            } => {
                assert_eq!(call_id, "crucible-call-9");
                assert_eq!(
                    resume_token, secret,
                    "ACP relay must stamp the bridge's secret resume_token"
                );
                assert!(resume_token.starts_with("apr-"), "got {resume_token:?}");
                assert_ne!(
                    resume_token, call_id,
                    "the model-known call_id must never be the resume handle"
                );
            }
            other => panic!("expected ApprovalRequired, got {other:?}"),
        }
    }

    /// Companion negative: with the bridge bound but NO bridge entry for the
    /// call (a regular tool gated by the ToolApprovalManager), the
    /// synthesized token must be EMPTY — never a fallback to the call_id.
    #[tokio::test]
    async fn relay_emitter_emits_empty_token_for_non_bridge_call() {
        let bridge = Arc::new(wcore_agent::approval::ApprovalBridge::new());
        let (tx, mut rx) = unbounded_channel::<ProtocolEvent>();
        let relay: RelayHandle = Arc::new(Mutex::new(Some(tx)));
        let emitter = RelayEmitter::new(relay, Some(bridge));

        emitter
            .emit(&ProtocolEvent::ToolRequest {
                msg_id: "m".into(),
                call_id: "bash-1".into(),
                tool: wcore_protocol::events::ToolInfo {
                    name: "Bash".into(),
                    category: wcore_protocol::events::ToolCategory::Exec,
                    args: serde_json::json!({"command": "ls"}),
                    description: "run".into(),
                },
            })
            .unwrap();

        let _tool_req = rx.recv().await.expect("forwarded ToolRequest");
        match rx.recv().await.expect("synthesized ApprovalRequired") {
            ProtocolEvent::ApprovalRequired { resume_token, .. } => {
                assert!(
                    resume_token.is_empty(),
                    "non-bridge call must get an EMPTY token (resolved via ToolApprove), got {resume_token:?}"
                );
            }
            other => panic!("expected ApprovalRequired, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn projection_maps_error_frame_with_retryable_data() {
        let events = vec![ProtocolEvent::Error {
            msg_id: Some("m".into()),
            error: wcore_protocol::events::ErrorInfo {
                code: "engine_error".into(),
                message: "kaboom".into(),
                retryable: true,
            },
        }];
        let out = project_all(events).await;
        assert_eq!(out.len(), 1);
        match &out[0] {
            MessageEvent::Error { error } => {
                assert_eq!(error.message, "kaboom");
                assert_eq!(error.code, ErrorCode::ToolFailed.code());
                assert_eq!(
                    error.data.as_ref().unwrap().get("retryable").unwrap(),
                    &serde_json::Value::Bool(true)
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // ── TerminalGuard (T-A6) ───────────────────────────────────────────

    #[tokio::test]
    async fn terminal_guard_fires_when_sender_dropped_without_terminal() {
        let (tx, rx) = unbounded_channel::<ProtocolEvent>();
        // Simulate a turn task that drops its sender (panic/abort) without a
        // terminal frame: the guard's Drop must emit Error + StreamEnd because
        // it was never disarmed.
        drop(TerminalGuard::new(tx.clone(), "m".to_string()));
        drop(tx);
        let out: Vec<MessageEvent> = ProtocolToMessageStream::new(rx).collect().await;
        // The guard emitted Error (terminal) — projection stops there.
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], MessageEvent::Error { .. }));
    }

    // ── error_info_for (T-A7) ──────────────────────────────────────────

    #[test]
    fn error_info_for_maps_user_aborted_not_retryable() {
        let info = error_info_for(&AgentError::UserAborted);
        assert!(!info.retryable);
        assert_eq!(info.code, "engine_error");
        let info = error_info_for(&AgentError::ContextTooLong {
            input_tokens: 100,
            limit: 50,
        });
        assert!(!info.retryable);
        let info = error_info_for(&AgentError::ApiError("boom".to_string()));
        assert!(!info.retryable);
    }

    #[test]
    fn stop_reason_mapping_is_exhaustive() {
        assert_eq!(stop_reason_str(FinishReason::Stop), "end_turn");
        assert_eq!(stop_reason_str(FinishReason::Length), "max_tokens");
        assert_eq!(stop_reason_str(FinishReason::Error), "refusal");
    }

    // ── A2A handshake redaction (T-B1, no engine call) ─────────────────

    #[tokio::test]
    async fn engine_a2a_handshake_redacts_anonymous_probe() {
        let handler = EngineA2aHandler::new(
            "server-agent",
            // Config is only touched lazily on a turn; an anonymous handshake
            // never builds a session, so a placeholder default is fine here.
            placeholder_config(),
            ".".to_string(),
        );
        let incoming = A2aHandshake {
            agent_id: String::new(),
            agent_kind: "other".to_string(),
            version: "0.0.1".to_string(),
            capabilities: A2aCapabilities::default(),
        };
        let reply = handler.on_handshake(incoming).await.unwrap();
        assert_eq!(reply.agent_kind, "wayland-core");
        assert!(reply.agent_id.is_empty(), "anonymous: no agent_id");
        assert!(reply.version.is_empty(), "anonymous: no version");
        assert!(reply.capabilities.tools.is_empty());
    }

    /// A minimal `Config` for tests that never reach an engine build. Mirrors
    /// the fields populated by `Config::resolve` defaults closely enough for
    /// the handshake-redaction path (which short-circuits before any build).
    fn placeholder_config() -> Config {
        Config::resolve(&wcore_config::config::CliArgs {
            provider: Some("anthropic".to_string()),
            api_key: Some("sk-ant-test-not-real".to_string()),
            base_url: None,
            model: None,
            max_tokens: None,
            max_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: None,
        })
        .expect("resolve a default config")
    }

    // ── F4: resolve_approval engine-level isolation ────────────────────────
    //
    // `EngineTurnEngine::resolve_approval` is the security-load-bearing half of
    // the ACP/REST approval endpoint: it MUST find the gate ONLY under the
    // session that actually owns it, never create a session on a resolve, and
    // be idempotent. The REST tests exercise it through a MockHandler that
    // ignores `session_id`, so the real per-session isolation is untested
    // there. These drive the REAL `EngineTurnEngine`.
    //
    // Scaffolding: build a live `EngineSession` over a real (offline) engine
    // and insert it straight into the private `sessions` pool — the same map
    // `session_for` populates — so we get a resolvable gate WITHOUT running a
    // full turn (which would need a live provider). A gate is staged by
    // registering a pending approval on the session's shared manager; the held
    // `oneshot::Receiver` is the ground truth for "still pending" (Empty) vs
    // "resolved" (a delivered `ToolApprovalResult`).

    use tokio::sync::oneshot::error::TryRecvError;
    use wcore_acp::turn::{ApprovalDecision, ApprovalScopeWire};
    use wcore_protocol::ToolApprovalResult;
    use wcore_protocol::events::ToolCategory;
    use wcore_tools::registry::ToolRegistry;

    /// Build a live `EngineSession` backed by a real (offline) `AgentEngine`.
    /// No turn is run, so no provider call is made.
    fn live_session() -> Arc<EngineSession> {
        let relay: RelayHandle = Arc::new(Mutex::new(None));
        let approval_manager = Arc::new(ToolApprovalManager::new());
        let output: Arc<dyn OutputSink> = Arc::new(RelaySink::new(relay.clone()));
        let engine = AgentEngine::new(placeholder_config(), ToolRegistry::new(), output);
        Arc::new(EngineSession::new(engine, approval_manager, relay))
    }

    /// A plain "approve once" decision (manager path — no bridge secret).
    fn approve_once() -> ApprovalDecision {
        ApprovalDecision {
            approved: true,
            scope: ApprovalScopeWire::Once,
            answer: None,
            resume_token: None,
        }
    }

    /// (#568 C) A BRIDGE-backed gate (Crucible council / egress consent) is
    /// resolvable through the ACP resolve path via its SECRET resume_token — the
    /// ingress that was previously missing, so such gates hung to TTL on ACP.
    /// Proves the endpoint routes a non-empty token to `ApprovalBridge::resolve`.
    #[tokio::test]
    async fn resolve_bridge_gate_by_secret_resume_token() {
        let turn = EngineTurnEngine::new(placeholder_config(), ".".to_string());
        let session = live_session();
        turn.sessions
            .lock()
            .await
            .insert("s1".to_string(), session.clone());

        // Stage a bridge-backed gate the way Crucible/egress would, capturing
        // the server-minted SECRET token and the pending waiter.
        let (secret, mut rx) = session
            .approval_bridge
            .request_with_id(
                "corr-1".to_string(),
                wcore_agent::approval::ApprovalRequest {
                    call_id: "corr-1".to_string(),
                    reason: "council".to_string(),
                    context: String::new(),
                },
            )
            .await;
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "gate is pending before resolve"
        );

        // Resolve through the ACP endpoint path WITH the secret token.
        let decision = ApprovalDecision {
            approved: true,
            scope: ApprovalScopeWire::Once,
            answer: None,
            resume_token: Some(secret),
        };
        turn.resolve_approval("s1", "corr-1", decision)
            .await
            .expect("bridge gate resolves via secret resume_token");

        let outcome = rx
            .try_recv()
            .expect("approved outcome delivered to the bridge waiter");
        assert!(outcome.approved, "the bridge waiter sees the approval");
    }

    /// (#568 C) A stale/unknown resume_token does NOT resolve via the bridge and
    /// falls through to the manager path; with no manager gate staged either the
    /// endpoint 404s (`Session` error) — idempotent, never a phantom 200.
    #[tokio::test]
    async fn resolve_with_stale_token_falls_through_then_404s() {
        let turn = EngineTurnEngine::new(placeholder_config(), ".".to_string());
        let session = live_session();
        turn.sessions
            .lock()
            .await
            .insert("s2".to_string(), session.clone());

        let decision = ApprovalDecision {
            approved: true,
            scope: ApprovalScopeWire::Once,
            answer: None,
            resume_token: Some("apr-does-not-exist".to_string()),
        };
        let err = turn
            .resolve_approval("s2", "no-such-call", decision)
            .await
            .expect_err("stale token + no manager gate must not resolve");
        assert!(
            matches!(err, AcpError::Session(_)),
            "stale token falls through to the manager path and 404s, got {err:?}"
        );
    }

    /// (a) Resolving for an UNKNOWN session id returns a `Session` error and
    /// does NOT create a session in the pool (resolve must never build-on-miss).
    #[tokio::test]
    async fn resolve_unknown_session_errors_and_creates_no_session() {
        let turn = EngineTurnEngine::new(placeholder_config(), ".".to_string());
        assert_eq!(turn.sessions.lock().await.len(), 0, "pool starts empty");

        let err = turn
            .resolve_approval("ghost-session", "call-1", approve_once())
            .await
            .expect_err("unknown session must not resolve");
        assert!(
            matches!(err, AcpError::Session(_)),
            "unknown session id maps to a Session error, got {err:?}"
        );
        assert_eq!(
            turn.sessions.lock().await.len(),
            0,
            "a resolve for an unknown session must NOT spin up a session",
        );
    }

    /// (b) Two live sessions A and B each hold a pending gate. Resolving B's
    /// `call_id` UNDER session A returns not-found and leaves B's gate pending
    /// (cross-session isolation: A cannot answer B's approval).
    #[tokio::test]
    async fn resolve_is_isolated_per_session() {
        let turn = EngineTurnEngine::new(placeholder_config(), ".".to_string());

        let session_a = live_session();
        let session_b = live_session();
        {
            let mut pool = turn.sessions.lock().await;
            pool.insert("A".to_string(), session_a.clone());
            pool.insert("B".to_string(), session_b.clone());
        }

        // Stage a pending gate on B only; hold B's receiver as ground truth.
        let mut rx_b =
            session_b
                .approval_manager()
                .request_approval("call-b", &ToolCategory::Edit, "Edit");

        // Resolve B's call_id but addressed to session A. A has no such gate.
        let err = turn
            .resolve_approval("A", "call-b", approve_once())
            .await
            .expect_err("A cannot resolve B's gate");
        assert!(
            matches!(err, AcpError::Session(_)),
            "cross-session resolve maps to not-found, got {err:?}"
        );

        // B's gate is untouched: its receiver has NOT been resolved.
        assert!(
            matches!(rx_b.try_recv(), Err(TryRecvError::Empty)),
            "B's pending gate must survive a resolve aimed at A",
        );

        // And the correct (B-addressed) resolve still works afterwards.
        turn.resolve_approval("B", "call-b", approve_once())
            .await
            .expect("B resolves its own gate");
        assert!(
            matches!(rx_b.try_recv(), Ok(ToolApprovalResult::Approved { .. })),
            "B's gate resolves only under B's session id",
        );
    }

    /// (c) A second resolve of the same `(session_id, call_id)` returns the
    /// not-found error — engine-level idempotency (the gate is consumed once).
    #[tokio::test]
    async fn second_resolve_of_same_call_is_not_found() {
        let turn = EngineTurnEngine::new(placeholder_config(), ".".to_string());

        let session = live_session();
        turn.sessions
            .lock()
            .await
            .insert("S".to_string(), session.clone());
        let _rx =
            session
                .approval_manager()
                .request_approval("call-x", &ToolCategory::Exec, "Bash");

        // First resolve consumes the gate → Ok.
        turn.resolve_approval("S", "call-x", approve_once())
            .await
            .expect("first resolve succeeds");

        // Second resolve of the same id finds nothing → Session error.
        let err = turn
            .resolve_approval("S", "call-x", approve_once())
            .await
            .expect_err("second resolve must not succeed");
        assert!(
            matches!(err, AcpError::Session(_)),
            "re-resolving a consumed gate is idempotent not-found, got {err:?}"
        );
    }
}
