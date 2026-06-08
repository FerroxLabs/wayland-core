//! v0.8.1 U2 — production subscriber for `AgentBus` lifecycle events.
//!
//! W7/v0.8.0 Task J wired the publisher side (`AgentSpawner::with_bus`
//! emits `Spawned` / `FirstMessage` / `Completed` / `Errored`) but
//! nothing subscribed in production: the broadcast channel published
//! to zero receivers. This module closes that loop with a tokio task
//! that forwards every lifecycle event to `tracing` so operators and
//! protocol clients can subscribe via the `wcore_agent::agents::bus`
//! tracing target.
//!
//! **v0.9.1.1 B4 fix:** events were previously also forwarded to
//! `OutputSink::emit_info`, which the TUI bridge translates into
//! transcript system turns. That meant every sub-agent session leaked
//! 4-N+ `agent.bus Spawned …` / `Completed …` lines into the user's
//! transcript. The fix demotes the forward to `tracing::debug!` only —
//! the SubAgentView feed already carries the user-facing signal, and
//! protocol clients that need raw bus events can subscribe to the bus
//! directly or to the tracing target.
//!
//! Lifecycle: `AgentBusObserver::spawn(bus, sink)` returns a small
//! handle. Drop / explicit `abort()` cancels the background task. The
//! observer's `JoinHandle` may also be parked on the engine's
//! `decay_handles` vec — `AgentEngine::Drop` aborts every handle, so
//! engine shutdown automatically tears the observer down.

use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::agents::bus::{AgentBus, AgentMessage};
use crate::output::OutputSink;

/// Production subscriber for `AgentBus` lifecycle events.
///
/// Holds the spawned tokio task; dropping the handle aborts the task.
/// The internal `JoinHandle` can also be detached via
/// [`AgentBusObserver::into_join_handle`] and parked on the engine's
/// background-task vec so engine shutdown owns the lifetime.
pub struct AgentBusObserver {
    handle: JoinHandle<()>,
}

impl AgentBusObserver {
    /// Spawn the production subscriber. `bus` must be the same bus
    /// attached to the production `AgentSpawner` via `with_bus(...)`.
    /// `sink` is retained on the signature for backwards compatibility
    /// with existing bootstrap call-sites but is intentionally unused —
    /// see the v0.9.1.1 B4 note in the module docstring: forwarding to
    /// `emit_info` leaked sub-agent bus chatter into the user transcript
    /// because the TUI bridge routes `Info` events to system turns. Bus
    /// events now flow ONLY to `tracing::debug!` on the
    /// `wcore_agent::agents::bus` target; the user-facing signal lives
    /// in the SubAgentView feed.
    pub fn spawn(bus: Arc<AgentBus>, sink: Arc<dyn OutputSink>) -> Self {
        // `sink` is intentionally dropped without use — see B4 note.
        // Keeping it on the signature avoids churning every bootstrap
        // call-site and asserts a non-null sink existed at wire-up
        // time (early detection of misconfigured engines).
        let _ = sink;
        let handle = tokio::spawn(async move {
            let mut rx = bus.subscribe();
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        let line = format_event(&msg);
                        // v0.9.1.1 B4: was `info!` + `sink.emit_info`. The
                        // info-level forward to the sink leaked into the
                        // TUI transcript via the bridge's `Info` arm.
                        // Debug-level + tracing-only keeps the diagnostic
                        // signal off the user-facing channel.
                        debug!(target: "wcore_agent::agents::bus", "{}", line);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            target: "wcore_agent::agents::bus",
                            lagged = n,
                            "AgentBusObserver dropped events (broadcast lag)",
                        );
                    }
                }
            }
        });
        Self { handle }
    }

    /// Explicitly abort the background task. Drop also aborts, so
    /// callers normally do not need to call this directly.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Detach the inner `JoinHandle` and consume the observer. Used by
    /// the production bootstrap to park the handle on the engine's
    /// `decay_handles` vec so `Drop for AgentEngine` aborts it.
    pub fn into_join_handle(self) -> JoinHandle<()> {
        // Move the handle out without running our own Drop (which would
        // abort the task we're about to hand off).
        let observer = std::mem::ManuallyDrop::new(self);
        // SAFETY: ManuallyDrop guarantees `observer.handle` is not
        // dropped by us; we read it out and return it directly.
        unsafe { std::ptr::read(&observer.handle) }
    }
}

impl Drop for AgentBusObserver {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Render an `AgentMessage` as a single human-readable line. The
/// per-variant formatting keeps the production log/sink output short
/// and predictable; subscribers that need the raw struct can
/// `bus.subscribe()` directly instead.
fn format_event(msg: &AgentMessage) -> String {
    match msg {
        AgentMessage::Spawned {
            agent,
            parent_call_id,
            timestamp_ms,
        } => {
            let pc = parent_call_id.as_deref().unwrap_or("-");
            format!("agent.bus Spawned agent={agent} parent_call_id={pc} ts_ms={timestamp_ms}")
        }
        AgentMessage::FirstMessage {
            agent,
            content_preview,
        } => format!("agent.bus FirstMessage agent={agent} preview={content_preview:?}"),
        AgentMessage::Completed {
            agent,
            turns,
            output_tokens,
        } => {
            format!("agent.bus Completed agent={agent} turns={turns} output_tokens={output_tokens}")
        }
        AgentMessage::Errored { agent, error } => {
            format!("agent.bus Errored agent={agent} error={error:?}")
        }
        AgentMessage::StatusUpdate { agent, message } => {
            format!("agent.bus StatusUpdate agent={agent} message={message:?}")
        }
        AgentMessage::ResultFragment { agent, payload } => {
            format!("agent.bus ResultFragment agent={agent} payload={payload}")
        }
        AgentMessage::RequestHelp { agent, question } => {
            format!("agent.bus RequestHelp agent={agent} question={question:?}")
        }
        AgentMessage::Abort { reason } => format!("agent.bus Abort reason={reason:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::bus::{AgentBus, AgentBusError, now_ms};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use wcore_types::message::FinishReason;

    /// Minimal sink that counts `emit_info` calls and records the last
    /// message; every other trait method is a no-op (defaults work for
    /// the methods that have them; the rest are explicit no-ops below).
    struct CountingSink {
        count: Arc<AtomicUsize>,
        last: parking_lot::Mutex<Option<String>>,
    }

    impl CountingSink {
        fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
            let count = Arc::new(AtomicUsize::new(0));
            let sink = Arc::new(Self {
                count: count.clone(),
                last: parking_lot::Mutex::new(None),
            });
            (sink, count)
        }
    }

    impl OutputSink for CountingSink {
        fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
        fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
        fn emit_tool_call(&self, _name: &str, _input: &str) {}
        fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {}
        fn emit_stream_start(&self, _msg_id: &str) {}
        fn emit_stream_end(
            &self,
            _msg_id: &str,
            _turns: usize,
            _input_tokens: u64,
            _output_tokens: u64,
            _cache_creation_tokens: u64,
            _cache_read_tokens: u64,
            _finish_reason: FinishReason,
        ) {
        }
        fn emit_error(&self, _msg: &str, _retryable: bool) {}
        fn emit_info(&self, msg: &str) {
            self.count.fetch_add(1, Ordering::Relaxed);
            *self.last.lock() = Some(msg.to_string());
        }
    }

    /// v0.9.1.1 B4 regression: the observer must NOT forward bus events
    /// to `OutputSink::emit_info`, because the TUI bridge routes `Info`
    /// events to transcript system turns — every multi-agent session
    /// leaked 4-N+ `agent.bus …` lines as user-visible noise. Bus
    /// events now flow only to `tracing::debug!`; the sink-counting
    /// channel must stay at zero across a representative event burst.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agent_bus_observer_emits_to_tracing_not_info_sink_v0911() {
        let bus = Arc::new(AgentBus::new(64));
        let (sink, count) = CountingSink::new();
        let observer = AgentBusObserver::spawn(Arc::clone(&bus), sink as Arc<dyn OutputSink>);

        // Give the spawned task a tick to install its subscription
        // before we publish; otherwise the broadcast channel drops the
        // events that arrive before `rx` exists.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Fire a representative cross-section of lifecycle events —
        // the v0.9.1 leak hit all of them.
        bus.publish(AgentMessage::Spawned {
            agent: "child".into(),
            parent_call_id: Some("spawn:child".into()),
            timestamp_ms: now_ms(),
        });
        bus.publish(AgentMessage::FirstMessage {
            agent: "child".into(),
            content_preview: "do the thing".into(),
        });
        bus.publish(AgentMessage::StatusUpdate {
            agent: "child".into(),
            message: "working on step 3".into(),
        });
        bus.publish(AgentMessage::Completed {
            agent: "child".into(),
            turns: 2,
            output_tokens: 42,
        });

        // Let the observer drain the channel.
        tokio::time::sleep(Duration::from_millis(60)).await;

        assert_eq!(
            count.load(Ordering::Relaxed),
            0,
            "AgentBusObserver must NOT forward bus events to emit_info \
             (would leak `agent.bus …` lines into the TUI transcript) — \
             B4 fix expects zero sink invocations across an event burst",
        );

        observer.abort();
    }

    #[tokio::test]
    async fn observer_drop_aborts_task() {
        let bus = Arc::new(AgentBus::new(16));
        let (sink, _count) = CountingSink::new();
        let observer = AgentBusObserver::spawn(Arc::clone(&bus), sink as Arc<dyn OutputSink>);
        drop(observer);
        // Nothing to assert beyond "no panic"; the abort itself is
        // best-effort and tokio will reclaim the task.
    }

    #[tokio::test]
    async fn await_completion_times_out_when_no_event() {
        let bus = AgentBus::new(16);
        let result = bus
            .await_completion("nonexistent", Duration::from_millis(50))
            .await;
        assert!(matches!(result, Err(AgentBusError::Timeout)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn await_completion_returns_on_match() {
        let bus = Arc::new(AgentBus::new(16));
        let bus_clone = Arc::clone(&bus);
        let waiter = tokio::spawn(async move {
            bus_clone
                .await_completion("child", Duration::from_millis(500))
                .await
        });

        // Give the waiter a moment to install its subscription.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Publish an unrelated event first, then the matching one.
        bus.publish(AgentMessage::Spawned {
            agent: "child".into(),
            parent_call_id: None,
            timestamp_ms: 0,
        });
        bus.publish(AgentMessage::Completed {
            agent: "child".into(),
            turns: 1,
            output_tokens: 7,
        });

        let got = waiter.await.expect("task did not panic");
        assert!(matches!(got, Ok(AgentMessage::Completed { .. })));
    }
}
