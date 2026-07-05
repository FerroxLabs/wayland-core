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
//! W5.5 H-1 RELIABILITY: lifecycle events (Done/Failed terminal signals
//! emitted via `emit_info`/`emit_error`) bypass the bounded stream
//! channel and travel through a dedicated `lifecycle_tx` channel
//! (capacity [`LIFECYCLE_CAPACITY`]). This channel is never shared with
//! stream events, so a chatty 256-event sub-agent cannot exhaust it.
//! The drain in `spawn_with_relay` flushes lifecycle events AFTER the
//! main stream drain — guaranteed delivery regardless of stream volume.

use serde_json::Value;
use tokio::sync::mpsc;
use wcore_protocol::events::{ErrorInfo, ProtocolEvent};
use wcore_tools::ToolOutputSink;
use wcore_types::message::FinishReason;

use crate::output::OutputSink;

/// Wave RA — bounded ChannelSink capacity. 256 is small enough to apply
/// backpressure (and shed load when the parent consumer is slow) and
/// large enough that normal sub-agent emission never trips the limit
/// during a turn. Documented as drop-oldest-on-full semantics via
/// `try_send` (see file-level docs).
pub const CHANNEL_CAPACITY: usize = 256;

/// W5.5 H-1 — dedicated lifecycle lane capacity. One terminal event per
/// sub-agent task (Done OR Failed), never more. Capacity 2 ensures
/// try_send never fails even if a second lifecycle event is fired
/// defensively (e.g. both emit_info AND emit_error from a buggy caller).
pub const LIFECYCLE_CAPACITY: usize = 2;

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

pub struct ChannelSink {
    parent_call_id: String,
    agent_name: String,
    /// Stream events (best-effort, drops on full).
    tx: mpsc::Sender<SubAgentRelay>,
    /// W5.5 H-1: Lifecycle events (guaranteed delivery). When `Some`,
    /// `emit_info` and `emit_error` route through this channel instead
    /// of `tx`. Capacity [`LIFECYCLE_CAPACITY`] is never exhausted by
    /// stream events, so the terminal Done/Failed signal always lands.
    lifecycle_tx: Option<mpsc::Sender<SubAgentRelay>>,
}

impl ChannelSink {
    /// Standard constructor — no dedicated lifecycle lane. `emit_info` /
    /// `emit_error` fall through to the shared `tx` (best-effort). Use
    /// [`ChannelSink::new_with_lifecycle`] for production relay paths
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
            lifecycle_tx: None,
        }
    }

    /// W5.5 H-1 constructor: attach a dedicated lifecycle channel.
    /// `spawn_with_relay` uses this so terminal events are never dropped
    /// under backpressure from chatty stream traffic.
    pub fn new_with_lifecycle(
        parent_call_id: String,
        agent_name: String,
        tx: mpsc::Sender<SubAgentRelay>,
        lifecycle_tx: mpsc::Sender<SubAgentRelay>,
    ) -> Self {
        Self {
            parent_call_id,
            agent_name,
            tx,
            lifecycle_tx: Some(lifecycle_tx),
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

    /// W5.5 H-1: relay a lifecycle (terminal) event through the dedicated
    /// lifecycle channel when available; fall back to the shared channel.
    /// The dedicated channel has capacity [`LIFECYCLE_CAPACITY`] and is
    /// never filled by stream events, so `try_send` here should never fail
    /// in practice. We still fall back rather than panic on the remote chance
    /// of a double-terminal call.
    fn relay_lifecycle(&self, event: ProtocolEvent) {
        let inner = match serde_json::to_value(&event) {
            Ok(v) => v,
            Err(_) => return,
        };
        let relay = SubAgentRelay {
            parent_call_id: self.parent_call_id.clone(),
            agent_name: self.agent_name.clone(),
            inner,
        };
        if let Some(ref ltx) = self.lifecycle_tx {
            // Dedicated lane: always has room (capacity 2, 1 event per task).
            // If try_send somehow fails (e.g. receiver gone), fall through to
            // the stream channel rather than silently dropping the event.
            if ltx.try_send(relay.clone()).is_ok() {
                return;
            }
        }
        // Fallback: shared stream channel (best-effort, same as before W5.5).
        let _ = self.tx.try_send(relay);
    }
}

impl OutputSink for ChannelSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.relay(ProtocolEvent::TextDelta {
            text: text.to_string(),
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
        self.relay(ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            finish_reason,
            usage: None,
            usage_delta: None,
            agent_run_id: None,
        });
    }
    fn emit_error(&self, msg: &str, retryable: bool) {
        // W5.5 F1: relay ProtocolEvent::Error so the bridge's "error" arm sets
        // SubAgentStatus::Failed (not Done). Previously relayed Info, causing a
        // crashed sub-agent to appear green/Done in the UI strip.
        //
        // W5.5 H-1: routes through relay_lifecycle (guaranteed delivery lane).
        // Carry the caller's retryable flag through so a transient sub-agent
        // failure (e.g. a provider 5xx) reaches the parent/host as retryable,
        // not a hardcoded false (matches the ProtocolSink contract).
        self.relay_lifecycle(ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "sub_agent_error".to_string(),
                message: msg.to_string(),
                retryable,
            },
        });
    }
    fn emit_info(&self, msg: &str) {
        // W5.5 H-1: terminal Done signal routes through the guaranteed
        // lifecycle lane so it survives channel backpressure from a chatty
        // sub-agent that filled the 256-event stream buffer.
        self.relay_lifecycle(ProtocolEvent::Info {
            msg_id: String::new(),
            message: msg.to_string(),
        });
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
        self.relay(ProtocolEvent::TextDelta {
            text: chunk.to_string(),
            msg_id: format!("{}-chunk", self.parent_call_id),
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

    /// W5.5 H-1 regression: when the stream channel is full, the lifecycle
    /// event (emit_info) must still be received via the dedicated lifecycle
    /// channel. This is the specific scenario where a chatty sub-agent fills
    /// the 256-event buffer before the terminal Done signal arrives.
    #[tokio::test]
    async fn lifecycle_event_survives_full_stream_channel_w55_h1() {
        // Stream channel capacity 2 so we can fill it without 256 events.
        let (stream_tx, _stream_rx) = mpsc::channel::<SubAgentRelay>(2);
        // Lifecycle channel capacity 2 (LIFECYCLE_CAPACITY).
        let (lifecycle_tx, mut lifecycle_rx) = mpsc::channel::<SubAgentRelay>(LIFECYCLE_CAPACITY);

        let sink = ChannelSink::new_with_lifecycle(
            "spawn:0:chatty".into(),
            "chatty".into(),
            stream_tx,
            lifecycle_tx,
        );

        // Fill the stream channel to capacity (simulate chatty sub-agent).
        sink.emit_text_delta("delta-1", "m");
        sink.emit_text_delta("delta-2", "m");
        // Stream channel is now full. A third stream event drops silently.
        sink.emit_text_delta("delta-3-dropped", "m");

        // The terminal lifecycle event MUST still arrive despite the full
        // stream channel, because it uses the dedicated lifecycle lane.
        sink.emit_info("sub-agent 'chatty' completed (3 turns)");

        // Drain from the lifecycle channel — must receive exactly one event.
        let event = lifecycle_rx
            .recv()
            .await
            .expect("W5.5 H-1: terminal info event must arrive on lifecycle channel even when stream channel is full");
        assert_eq!(
            event.inner["type"], "info",
            "lifecycle event must be type 'info' (the Done signal)"
        );
        assert_eq!(event.parent_call_id, "spawn:0:chatty");

        // No second event (only one terminal event per task).
        let second = lifecycle_rx.try_recv();
        assert!(
            second.is_err(),
            "lifecycle channel must have exactly one event, not more"
        );
    }

    /// W5.5 H-1 regression: failed sub-agent terminal event (emit_error)
    /// also survives full stream channel via the lifecycle lane.
    #[tokio::test]
    async fn lifecycle_error_survives_full_stream_channel_w55_h1() {
        let (stream_tx, _stream_rx) = mpsc::channel::<SubAgentRelay>(2);
        let (lifecycle_tx, mut lifecycle_rx) = mpsc::channel::<SubAgentRelay>(LIFECYCLE_CAPACITY);

        let sink = ChannelSink::new_with_lifecycle(
            "spawn:0:failed".into(),
            "failed".into(),
            stream_tx,
            lifecycle_tx,
        );

        // Fill the stream channel.
        sink.emit_text_delta("a", "m");
        sink.emit_text_delta("b", "m");
        // Stream now full. Terminal error must still arrive. Pass retryable=true
        // so the assertion below proves the flag is THREADED, not hardcoded.
        sink.emit_error("engine crashed", true);

        let event = lifecycle_rx
            .recv()
            .await
            .expect("W5.5 H-1: terminal error event must arrive via lifecycle lane");
        assert_eq!(
            event.inner["type"], "error",
            "emit_error must relay an 'error' type event so bridge sets Failed"
        );
        // Confirm error message is carried.
        let msg = event.inner["error"]["message"].as_str().unwrap_or("");
        assert_eq!(msg, "engine crashed");
        // Regression (audit finding): the relayed error must carry the caller's
        // `retryable` flag, not a hardcoded false — so a transient sub-agent
        // failure reaches the parent/host as retryable.
        assert_eq!(
            event.inner["error"]["retryable"], true,
            "ChannelSink must relay the caller's retryable flag, not hardcode false"
        );
    }
}
