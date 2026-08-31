//! W7 F2: ChannelSink — `OutputSink` that relays sub-agent events to
//! the parent via mpsc, tagged with `parent_call_id` + `agent_name`.
//! The parent engine wraps each relay in `ProtocolEvent::SubAgentEvent`
//! for emission, keeping wire-format control with the parent.
//!
//! Wave RA RELIABILITY MAJOR — backpressure. The channel is **bounded**
//! at [`CHANNEL_CAPACITY`]. The `OutputSink` trait methods are sync
//! (`&self`, no `.await`), so we cannot `.send().await` here. We use
//! `try_send`: on full-channel the relay is dropped on the floor (the
//! sub-agent's stream is best-effort visibility into the parent; a slow
//! parent consumer must not be allowed to OOM the engine by pinning
//! every sub-agent's emission queue in memory). This matches the
//! existing "receiver gone → drop silently" semantics already covered
//! by the `channel_sink_drops_silently_when_receiver_gone` test.
//!
//! Workflow child terminals never share the diagnostic stream. The spawner
//! emits exactly one typed [`SubAgentTerminalRelay`] after the child result is
//! known; ordinary `Info`/`Error` diagnostics remain ordered with stream data.
//!
//! **F21-04-01 — this sink is a per-child observability boundary, not just a
//! text pipe.** `OutputSink` gives every method a default empty body, so any
//! method this type does not override is a SILENT DROP for every spawned child.
//! Phase 21 measured the consequence: a host driving Core over the JSON-stream
//! protocol could see a child's prose and its terminal, but not the structured
//! lifecycle facts (budget caps, cancellation decisions) that Success Criterion
//! 2 requires it to attribute per child. When adding a lifecycle-bearing method
//! to `OutputSink`, decide explicitly whether a child's copy of it must reach
//! the parent, and override it here if so.

use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use wcore_protocol::events::{ErrorInfo, ProtocolEvent, WorkflowChildTerminalState};
use wcore_tools::ToolOutputSink;
use wcore_types::message::FinishReason;
use wcore_types::reasoning_filter::ReasoningFilter;

use crate::output::OutputSink;

/// Wave RA — bounded ChannelSink capacity. 256 is small enough to apply
/// backpressure (and shed load when the parent consumer is slow) and
/// large enough that normal sub-agent emission never trips the limit
/// during a turn. Documented as drop-new-on-full semantics via
/// `try_send` (see file-level docs).
pub const CHANNEL_CAPACITY: usize = 256;

/// Exactly one terminal is emitted per child result.
pub const TERMINAL_CAPACITY: usize = 1;

/// One unit of relay-back to the parent. The parent engine wraps each
/// of these in a `ProtocolEvent::SubAgentEvent` and emits via its
/// own sink — keeping wire-format control with the parent.
#[derive(Debug, Clone)]
pub struct SubAgentRelay {
    pub parent_call_id: String,
    pub agent_name: String,
    /// The sub-agent's event, already serialized to a JSON Value.
    pub inner: Value,
}

/// One authoritative child terminal, separate from best-effort diagnostics.
#[derive(Debug, Clone)]
pub struct SubAgentTerminalRelay {
    pub relay: SubAgentRelay,
    pub terminal_state: WorkflowChildTerminalState,
}

pub struct ChannelSink {
    parent_call_id: String,
    agent_name: String,
    /// Stream events (best-effort, drops on full).
    tx: mpsc::Sender<SubAgentRelay>,
    /// Authoritative once-only terminal lane. Diagnostics never enter it.
    terminal_tx: Option<mpsc::Sender<SubAgentTerminalRelay>>,
    terminal_sent: AtomicBool,
    /// #1129 - inline-reasoning split for the child's MODEL TEXT lane.
    ///
    /// `ProtocolSink` strips `<think>`-class tags out of `text_delta` and
    /// re-emits the body as `thinking`, because the shipped spec (S1.3)
    /// promises UNCONDITIONALLY that `text` never carries inline reasoning
    /// and tells hosts not to write their own stripper. A spawned child does
    /// not write to a `ProtocolSink` - its `OutputSink` is THIS type, and the
    /// parent's `emit_sub_agent_event` clones `inner` without inspecting it.
    /// Without this filter an R1/Qwen-class child put a literal
    /// `<think>...</think>` on the wire at `sub_agent_event.inner.text`.
    ///
    /// Per-child by construction: `spawn_tool` builds one `ChannelSink` per
    /// task, so one child's straddling tag can never eat a sibling's text.
    reasoning: parking_lot::Mutex<ReasoningFilter>,
    /// #1129 - a SEPARATE filter for the streaming-tool-output lane.
    ///
    /// `ToolOutputSink::emit_chunk` deliberately relays tool output on the
    /// same `text_delta` wire shape, so it is bound by the same S1.3 promise.
    /// It must NOT share the model-text filter: the two lanes interleave, and
    /// a `<` in a file a child happens to `cat` would otherwise swallow the
    /// child's answer (and vice versa).
    chunk_reasoning: parking_lot::Mutex<ReasoningFilter>,
}

impl ChannelSink {
    /// Standard constructor — no dedicated lifecycle lane. `emit_info` /
    /// `emit_error` fall through to the shared `tx` (best-effort). Use
    /// [`ChannelSink::new_with_terminal`] for production relay paths
    /// where the terminal event must survive channel backpressure.
    pub fn new(
        parent_call_id: String,
        agent_name: String,
        tx: mpsc::Sender<SubAgentRelay>,
    ) -> Self {
        Self {
            parent_call_id,
            agent_name,
            tx,
            terminal_tx: None,
            terminal_sent: AtomicBool::new(false),
            reasoning: parking_lot::Mutex::new(ReasoningFilter::new()),
            chunk_reasoning: parking_lot::Mutex::new(ReasoningFilter::new()),
        }
    }

    /// Attach the dedicated terminal channel used by production relay paths.
    pub fn new_with_terminal(
        parent_call_id: String,
        agent_name: String,
        tx: mpsc::Sender<SubAgentRelay>,
        terminal_tx: mpsc::Sender<SubAgentTerminalRelay>,
    ) -> Self {
        Self {
            parent_call_id,
            agent_name,
            tx,
            terminal_tx: Some(terminal_tx),
            terminal_sent: AtomicBool::new(false),
            reasoning: parking_lot::Mutex::new(ReasoningFilter::new()),
            chunk_reasoning: parking_lot::Mutex::new(ReasoningFilter::new()),
        }
    }

    fn relay(&self, event: ProtocolEvent) {
        let inner = match serde_json::to_value(&event) {
            Ok(v) => v,
            Err(_) => return, // dropping a malformed inner event is preferable to panicking
        };
        // Wave RA — `OutputSink` is a sync trait, so we cannot
        // `.send().await`. `try_send` drops the relay if the parent
        // consumer is slow enough to fill the [`CHANNEL_CAPACITY`]
        // buffer — best-effort visibility instead of OOM.
        let _ = self.tx.try_send(SubAgentRelay {
            parent_call_id: self.parent_call_id.clone(),
            agent_name: self.agent_name.clone(),
            inner,
        });
    }

    /// The msg_id the streaming-tool-output lane relays under.
    fn chunk_msg_id(&self) -> String {
        format!("{}-chunk", self.parent_call_id)
    }

    /// #1129 - drain whatever inline reasoning `filter` is holding and relay
    /// it as a typed `thinking` instead of deleting it.
    ///
    /// Called after every chunk (so reasoning streams incrementally rather
    /// than as one end-of-turn blob) and again at the turn boundaries, which
    /// is what recovers an UNCLOSED `<think>`: the filter eats to end of
    /// stream rather than leak a runaway tail, and without this flush that
    /// tail would be silently dropped instead of shown.
    /// #1242 - drain what `filter` is still WITHHOLDING and relay it as one
    /// last `text_delta` on `msg_id`, before the `stream_end` that closes the
    /// message.
    ///
    /// Same shape and same reason as `ProtocolSink::drain_withheld_text`: the
    /// relay IS the wire a parent host reads, so a child whose answer ends in
    /// `<` or in an unclosed `<thinking>` would otherwise be relayed short of
    /// what the child's own history stored. Ordered before
    /// [`Self::flush_reasoning`] for the same reason that sink is: one drain
    /// order for every consumer of the filter, NOT to prevent a double report.
    /// This sink also flushes after every chunk, so the capture buffer is
    /// already empty when `finish` truncates it and an unclosed block is
    /// relayed twice, exactly as `ProtocolSink::drain_withheld_text`
    /// documents.
    fn drain_withheld_text(&self, filter: &parking_lot::Mutex<ReasoningFilter>, msg_id: &str) {
        let recovered = filter.lock().finish();
        if recovered.is_empty() {
            return;
        }
        self.relay(ProtocolEvent::TextDelta {
            text: recovered,
            msg_id: msg_id.to_string(),
        });
    }

    fn flush_reasoning(&self, filter: &parking_lot::Mutex<ReasoningFilter>, msg_id: &str) {
        let captured = filter.lock().take_captured_delta();
        if !captured.is_empty() {
            self.relay(ProtocolEvent::Thinking {
                text: captured,
                msg_id: msg_id.to_string(),
                subject: None,
            });
        }
    }

    /// Emit the single authoritative terminal after the child result is known.
    /// There is deliberately no stream fallback: a terminal that can reorder
    /// behind diagnostics is not authoritative evidence.
    pub fn relay_terminal(&self, terminal_state: WorkflowChildTerminalState, message: &str) {
        if self.terminal_sent.swap(true, Ordering::AcqRel) {
            return;
        }
        let event = match terminal_state {
            WorkflowChildTerminalState::Succeeded => ProtocolEvent::Info {
                msg_id: format!("{}:terminal", self.parent_call_id),
                message: message.to_owned(),
            },
            WorkflowChildTerminalState::Failed => ProtocolEvent::Error {
                msg_id: Some(format!("{}:terminal", self.parent_call_id)),
                error: ErrorInfo {
                    code: "sub_agent_error".to_owned(),
                    message: message.to_owned(),
                    retryable: false,
                    category: wcore_protocol::events::FailureCategory::Unknown,
                },
            },
        };
        if let Some(tx) = &self.terminal_tx {
            let inner = match serde_json::to_value(&event) {
                Ok(v) => v,
                Err(_) => return,
            };
            let _ = tx.try_send(SubAgentTerminalRelay {
                relay: SubAgentRelay {
                    parent_call_id: self.parent_call_id.clone(),
                    agent_name: self.agent_name.clone(),
                    inner,
                },
                terminal_state,
            });
        } else {
            // Compatibility constructors predate the dedicated lifecycle lane.
            // Keep their terminal observable on the best-effort stream without
            // weakening production paths, which always use `new_with_terminal`.
            self.relay(event);
        }
    }
}

impl OutputSink for ChannelSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        // #1129: split inline reasoning out of the relayed visible stream,
        // mirroring `ProtocolSink::emit_text_delta`. The withheld body rides
        // the typed `thinking` relay rather than being deleted, so a host
        // renders a child's reasoning collapsed exactly as it does the
        // parent's.
        let visible = self.reasoning.lock().process(text);
        self.flush_reasoning(&self.reasoning, msg_id);
        // A chunk the filter consumed WHOLE (pure reasoning, or the leading
        // half of a tag straddling the boundary) must not become an empty
        // `text_delta`: a host that counts deltas to decide "the child
        // answered" would read a reasoning-only turn as an answer. An
        // already-empty input still passes through - that is the producer's
        // frame, not the filter's doing.
        if visible.is_empty() && !text.is_empty() {
            return;
        }
        self.relay(ProtocolEvent::TextDelta {
            text: visible,
            msg_id: msg_id.to_string(),
        });
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.relay(ProtocolEvent::Thinking {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
            subject: None,
        });
    }
    fn emit_thinking_subject(&self, subject: &str, msg_id: &str) {
        self.relay(ProtocolEvent::Thinking {
            text: String::new(),
            msg_id: msg_id.to_string(),
            subject: Some(subject.to_string()),
        });
    }
    fn emit_tool_call(&self, _name: &str, _input: &str) {
        // legacy bridge unused for relay
    }
    fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {
        // legacy bridge unused for relay
    }
    fn emit_stream_start(&self, msg_id: &str) {
        // #1129: a runaway unclosed `<think>` from a cancelled turn must not
        // suppress the next turn's visible output. Flush first so the tool
        // lane's tail - tool execution runs BETWEEN a `stream_end` and the
        // next `stream_start` - is shown rather than discarded by the reset.
        self.flush_reasoning(&self.chunk_reasoning, &self.chunk_msg_id());
        self.flush_reasoning(&self.reasoning, msg_id);
        self.reasoning.lock().reset();
        self.chunk_reasoning.lock().reset();
        self.relay(ProtocolEvent::StreamStart {
            msg_id: msg_id.to_string(),
        });
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        _turns: usize,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_create: u64,
        _cache_read: u64,
        finish_reason: FinishReason,
    ) {
        // #1242 then #1129, on BOTH lanes: withheld text first, then the
        // reasoning capture. The lanes interleave and each has its own state
        // machine, so each is drained against its own msg_id.
        self.drain_withheld_text(&self.reasoning, msg_id);
        self.flush_reasoning(&self.reasoning, msg_id);
        self.drain_withheld_text(&self.chunk_reasoning, &self.chunk_msg_id());
        self.flush_reasoning(&self.chunk_reasoning, &self.chunk_msg_id());
        self.relay(ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            finish_reason,
            usage: None,
            usage_delta: None,
            agent_run_id: None,
        });
    }
    fn emit_error(
        &self,
        msg: &str,
        retryable: bool,
        category: wcore_protocol::events::FailureCategory,
    ) {
        // W5.5 F1: relay ProtocolEvent::Error so the bridge's "error" arm sets
        // SubAgentStatus::Failed (not Done). Previously relayed Info, causing a
        // crashed sub-agent to appear green/Done in the UI strip.
        //
        // Engine errors can be retry diagnostics. The spawner publishes the
        // only authoritative child terminal after the final result is known.
        self.relay(ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "sub_agent_error".to_string(),
                message: msg.to_string(),
                retryable,
                // wayland#1266 c3 -- the CHILD's own category, relayed, not
                // the parent's read of it.
                //
                // #1237 hardcoded `ToolRuntime` here and defended it as right
                // "from the parent turn's point of view": the child is
                // something the parent invoked. That is true and it is also
                // lossy, and #1266 c3 says which of the two wins. Under the
                // hardcode a child that died on a context ceiling and a child
                // that died on a local authority fault were the same frame to
                // the host -- the classification the CHILD engine had already
                // made was thrown away at the relay, which is #1266's whole
                // complaint one boundary over.
                //
                // Passed through verbatim rather than remapped, because c3
                // asks for both halves and a remap can only serve one: a child
                // that hit a context limit must arrive as `context_limit`, AND
                // a child that died on an opaque upstream response must still
                // arrive as `unknown` rather than being upgraded to a
                // plausible-looking `tool_runtime`. Substituting `ToolRuntime`
                // for `Unknown` here would be exactly the guess #1237 c4
                // forbids, made on the child's behalf.
                //
                // The parent still knows this was a child: `code` is
                // `sub_agent_error` and the frame is relayed inside the
                // parent's `sub_agent_event` envelope. The category answers
                // "why did it die", not "whose was it".
                category,
            },
        });
    }
    fn emit_info(&self, msg: &str) {
        self.relay(ProtocolEvent::Info {
            msg_id: String::new(),
            message: msg.to_string(),
        });
    }

    /// F21-04-01 — a child's budget-cap event is a PER-CHILD observable.
    ///
    /// `AgentEngine` raises this on `self.output` at every reservation,
    /// settlement and mid-flight cap check, and for a spawned child
    /// `self.output` IS this sink (`spawner.rs::execute_resolved_launch`).
    /// Without this override the trait's empty default body swallowed it, so
    /// the only trace a host got was the free-form `Error` that follows each
    /// call site — text a host can render but cannot classify, address or
    /// audit. Relaying it keeps the structured `reason`/`observed`/`limit`
    /// triple and lets the parent tag it with `parent_call_id` + `agent_name`,
    /// which is how the host attributes it to the sibling that raised it.
    ///
    /// This rides the EXISTING `sub_agent_event` wire shape: `inner` is an open
    /// object in the pinned contract, so no event type is added and no contract
    /// regeneration is required.
    fn emit_budget_exceeded(&self, reason: &str, observed: &str, limit: &str) {
        self.relay(ProtocolEvent::BudgetExceeded {
            reason: reason.to_string(),
            observed: observed.to_string(),
            limit: limit.to_string(),
        });
    }

    /// F21-04-01 — the mid-flight monitor's decision is the child's own
    /// cancel/replan signal. `MidFlightMonitor` is constructed unconditionally
    /// in `AgentEngine::new_with_provider`, so a child reaches it; a
    /// `MonitorDirective::Stop` is a cancellation that terminated THAT child
    /// and nothing else. Dropped by the default body before this override, it
    /// left a host unable to distinguish a cancelled sibling from a sibling
    /// that simply errored.
    fn emit_midflight_monitor_decision(
        &self,
        directive: wcore_protocol::events::MonitorDirective,
        reason: wcore_protocol::events::MonitorReason,
    ) {
        self.relay(ProtocolEvent::MidFlightMonitorDecision { directive, reason });
    }
}

/// W8a A.3 (resolves audit F4) — bridge W7's `ChannelSink` to the new
/// `wcore_tools::ToolOutputSink` trait so `ToolContext.sink` can be
/// wired directly to a sub-agent's relay channel in A.4 body
/// migrations (BashTool streaming, Script DSL). Maps `emit_chunk` to
/// a `TextDelta` relay against the sub-agent's `parent_call_id`;
/// `emit_progress` lands as a structured `Info` relay since W7 did
/// not define a dedicated progress event.
impl ToolOutputSink for ChannelSink {
    fn emit_chunk(&self, chunk: &str) {
        // Reuse the existing TextDelta path so host decoders that
        // already render sub-agent text show streaming tool output
        // inline with no schema change.
        //
        // #1129: because this IS a `text_delta` on the wire, it is bound by
        // the same S1.3 promise as model text. Filtered through the lane's
        // OWN state machine, never the model-text one - the lanes interleave.
        let msg_id = self.chunk_msg_id();
        let visible = self.chunk_reasoning.lock().process(chunk);
        self.flush_reasoning(&self.chunk_reasoning, &msg_id);
        if visible.is_empty() && !chunk.is_empty() {
            return;
        }
        self.relay(ProtocolEvent::TextDelta {
            text: visible,
            msg_id,
        });
    }

    fn emit_progress(&self, pct: f32, message: &str) {
        // Progress goes through the stream channel (best-effort, not lifecycle).
        self.relay(ProtocolEvent::Info {
            msg_id: String::new(),
            message: format!(
                "[progress {:.0}%] {message}",
                (pct * 100.0).clamp(0.0, 100.0)
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn channel_sink_relays_text_delta_through_channel() {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = ChannelSink::new("c-1".into(), "reviewer".into(), tx);
        sink.emit_text_delta("hello", "m-sub-1");
        let relay = rx.recv().await.expect("relay must arrive");
        assert_eq!(relay.parent_call_id, "c-1");
        assert_eq!(relay.agent_name, "reviewer");
        assert_eq!(relay.inner["type"], "text_delta");
        assert_eq!(relay.inner["text"], "hello");
    }

    #[tokio::test]
    async fn channel_sink_drops_silently_when_receiver_gone() {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = ChannelSink::new("c-1".into(), "reviewer".into(), tx);
        drop(rx);
        // must not panic
        sink.emit_text_delta("dropped", "m");
    }

    /// W8a A.3 — ChannelSink relays tool-output streaming chunks back
    /// to the parent via the new `ToolOutputSink` surface.
    #[tokio::test]
    async fn channel_sink_tool_output_sink_chunk_relays_as_text_delta() {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = ChannelSink::new("c-2".into(), "builder".into(), tx);
        <ChannelSink as ToolOutputSink>::emit_chunk(&sink, "stdout-line");
        let relay = rx.recv().await.expect("relay must arrive");
        assert_eq!(relay.parent_call_id, "c-2");
        assert_eq!(relay.inner["type"], "text_delta");
        assert_eq!(relay.inner["text"], "stdout-line");
    }

    #[tokio::test]
    async fn channel_sink_tool_output_sink_progress_relays_as_info() {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = ChannelSink::new("c-3".into(), "scout".into(), tx);
        <ChannelSink as ToolOutputSink>::emit_progress(&sink, 0.42, "halfway");
        let relay = rx.recv().await.expect("relay must arrive");
        assert_eq!(relay.inner["type"], "info");
        let msg = relay.inner["message"].as_str().unwrap();
        assert!(msg.contains("42%"));
        assert!(msg.contains("halfway"));
    }

    /// Wave RA — when the bounded channel fills, `try_send` drops the
    /// new relay rather than blocking the sync `OutputSink` method. The
    /// sub-agent's emission path must remain non-blocking even when the
    /// parent consumer is slow / stalled.
    #[tokio::test]
    async fn channel_sink_drops_on_full_channel() {
        // Capacity 2 so we can fill it deterministically.
        let (tx, _rx) = mpsc::channel::<SubAgentRelay>(2);
        let sink = ChannelSink::new("c-full".into(), "agent".into(), tx);
        // Three emissions: first two land, third gets dropped silently.
        sink.emit_text_delta("a", "m");
        sink.emit_text_delta("b", "m");
        sink.emit_text_delta("c", "m"); // must not block / panic
    }

    /// A chatty child cannot crowd its authoritative terminal out of the
    /// dedicated terminal lane.
    #[tokio::test]
    async fn terminal_success_survives_full_stream_channel() {
        // Stream channel capacity 2 so we can fill it without 256 events.
        let (stream_tx, _stream_rx) = mpsc::channel::<SubAgentRelay>(2);
        let (terminal_tx, mut terminal_rx) =
            mpsc::channel::<SubAgentTerminalRelay>(TERMINAL_CAPACITY);

        let sink = ChannelSink::new_with_terminal(
            "spawn:0:chatty".into(),
            "chatty".into(),
            stream_tx,
            terminal_tx,
        );

        // Fill the stream channel to capacity (simulate chatty sub-agent).
        sink.emit_text_delta("delta-1", "m");
        sink.emit_text_delta("delta-2", "m");
        // Stream channel is now full. A third stream event drops silently.
        sink.emit_text_delta("delta-3-dropped", "m");

        // Diagnostics remain on the full best-effort stream.
        sink.emit_info("retrying provider");
        sink.emit_error(
            "transient provider failure",
            true,
            wcore_protocol::events::FailureCategory::Unknown,
        );
        sink.relay_terminal(
            WorkflowChildTerminalState::Succeeded,
            "sub-agent 'chatty' completed (3 turns)",
        );

        let event = terminal_rx
            .recv()
            .await
            .expect("terminal must arrive even when the stream is full");
        assert_eq!(event.terminal_state, WorkflowChildTerminalState::Succeeded);
        assert_eq!(event.relay.inner["type"], "info");
        assert_eq!(event.relay.inner["msg_id"], "spawn:0:chatty:terminal");
        assert_eq!(event.relay.parent_call_id, "spawn:0:chatty");

        sink.relay_terminal(WorkflowChildTerminalState::Failed, "late contradiction");
        assert!(terminal_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn legacy_constructor_relays_terminal_on_stream() {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let sink = ChannelSink::new("spawn:0:legacy".into(), "legacy".into(), tx);
        sink.relay_terminal(WorkflowChildTerminalState::Succeeded, "completed");

        let relay = rx
            .recv()
            .await
            .expect("legacy terminal must remain visible");
        assert_eq!(relay.inner["type"], "info");
        assert_eq!(relay.inner["msg_id"], "spawn:0:legacy:terminal");
    }

    /// Failed child results use the same isolated, typed terminal lane.
    #[tokio::test]
    async fn terminal_failure_survives_full_stream_channel() {
        let (stream_tx, _stream_rx) = mpsc::channel::<SubAgentRelay>(2);
        let (terminal_tx, mut terminal_rx) =
            mpsc::channel::<SubAgentTerminalRelay>(TERMINAL_CAPACITY);

        let sink = ChannelSink::new_with_terminal(
            "spawn:0:failed".into(),
            "failed".into(),
            stream_tx,
            terminal_tx,
        );

        // Fill the stream channel.
        sink.emit_text_delta("a", "m");
        sink.emit_text_delta("b", "m");
        // A retry diagnostic remains best-effort and is not a terminal.
        sink.emit_error(
            "engine crashed",
            true,
            wcore_protocol::events::FailureCategory::Unknown,
        );
        sink.relay_terminal(WorkflowChildTerminalState::Failed, "engine crashed");

        let event = terminal_rx
            .recv()
            .await
            .expect("failed terminal must arrive via terminal lane");
        assert_eq!(event.terminal_state, WorkflowChildTerminalState::Failed);
        assert_eq!(event.relay.inner["type"], "error");
        let msg = event.relay.inner["error"]["message"].as_str().unwrap_or("");
        assert_eq!(msg, "engine crashed");
        assert_eq!(event.relay.inner["error"]["retryable"], false);
    }
}
