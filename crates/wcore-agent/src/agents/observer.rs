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

    /// Block until `bus` reports at least `n` live subscribers, or fail.
    ///
    /// REPLACES a fixed `sleep`, which is not a synchronisation primitive.
    /// `tokio::broadcast` DROPS anything published while no receiver exists,
    /// so a publisher that starts before the subscriber installs its receiver
    /// sends into the void. The old code slept 20ms and hoped; under the
    /// shared-process lib suite (`cargo test`, ~2,700 tests in ONE process)
    /// the spawned task is not reliably scheduled inside that window, and
    /// `await_completion_returns_on_match` failed for it on CI runs
    /// 33565695499 and 33569... -- the SAME COMMIT passing in one job and
    /// failing in another, which is what makes it a flake rather than a break.
    ///
    /// Bounded so a genuinely broken subscribe fails loudly instead of hanging
    /// the suite. The bound is a COUNT OF POLLS, not elapsed wall-clock time
    /// (wayland#1240 c2): a `std::time::Instant` deadline here is a second
    /// real-time race of exactly the kind this module is removing, and it is
    /// unreachable under `start_paused`, where it degrades into an unbounded
    /// spin instead of a loud failure. Under a real clock the poll interval
    /// makes 10,000 polls the same ~10 s ceiling the old deadline had; under a
    /// paused clock each poll advances virtual time only, so the ceiling costs
    /// no wall time at all.
    async fn await_subscribers(bus: &AgentBus, n: usize) {
        const MAX_POLLS: usize = 10_000;
        for _ in 0..MAX_POLLS {
            if bus.sender().receiver_count() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!(
            "no subscriber installed within {MAX_POLLS} polls (wanted {n}, saw {})",
            bus.sender().receiver_count(),
        );
    }
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
        fn emit_error(
            &self,
            _msg: &str,
            _retryable: bool,
            _category: wcore_protocol::events::FailureCategory,
        ) {
        }
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

        // Wait for the subscription to EXIST. The old fixed sleep made this
        // assertion VACUOUS in the other direction: this test asserts the sink
        // count stays ZERO, so if the events were dropped for want of a
        // subscriber it passed while proving nothing about the observer at all.
        // Now the burst below is guaranteed to reach the observer, so a zero
        // count is evidence that it does not forward to the sink.
        await_subscribers(&bus, 1).await;

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

    /// wayland#1240: this test used to decide on REAL ELAPSED TIME. The waiter's
    /// 500 ms deadline starts inside the spawned task, and the publisher only
    /// runs after `await_subscribers` observes the subscription — a loop whose
    /// 1 ms `sleep` is a *request*, not a guarantee. On the shared-process lib
    /// leg (`cargo test`, ~2,700 tests in ONE process on a loaded runner) that
    /// 1 ms sleep can return hundreds of milliseconds late, the 500 ms deadline
    /// expires before `Completed` is ever published, and the assertion below
    /// reds on a scheduling artefact rather than on the behaviour it names.
    ///
    /// The fix is the one wayland#1182 applied to the workspace-walk control:
    /// stop racing the clock. `start_paused` gives the test a VIRTUAL clock
    /// that advances only to the next armed timer when every task is idle, so
    /// the sequence is forced: the waiter subscribes and parks, virtual time
    /// steps to the 1 ms poll (never past it to the 500 ms deadline), the
    /// publisher runs, and the waiter is woken by the message. Load cannot
    /// reorder it because no step is a function of wall time.
    ///
    /// THE DEADLINE IS NOT MADE UNREACHABLE, which would pass for the wrong
    /// reason (wayland#1240 c3). `await_completion_times_out_when_completed_is_
    /// suppressed` below is the same construction with the matching event
    /// withheld, and it reaches the very same 500 ms deadline on the very same
    /// virtual clock and returns `Timeout`.
    #[tokio::test(start_paused = true)]
    async fn await_completion_returns_on_match() {
        let bus = Arc::new(AgentBus::new(16));
        let bus_clone = Arc::clone(&bus);
        let waiter = tokio::spawn(async move {
            bus_clone
                .await_completion("child", Duration::from_millis(500))
                .await
        });

        // Wait for the subscription to EXIST rather than sleeping and hoping;
        // see `await_subscribers`. This is the line that flaked.
        await_subscribers(&bus, 1).await;
        assert_eq!(
            bus.sender().receiver_count(),
            1,
            "the waiter's receiver must be installed before anything is \
             published — broadcast DROPS messages sent with no receiver, and \
             this count, not an elapsed duration, is what gates the publish",
        );

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
        assert!(
            matches!(got, Ok(AgentMessage::Completed { .. })),
            "waiter must resolve on the matching Completed event, got {got:?}",
        );
    }

    /// RED CONTROL for `await_completion_returns_on_match` (wayland#1240 c3).
    ///
    /// Identical construction — same virtual clock, same subscription gate,
    /// same 500 ms deadline, same unrelated `Spawned` event — with the matching
    /// `Completed` WITHHELD. If the paused clock had made the deadline
    /// unreachable, this would hang or resolve `Ok`; instead virtual time runs
    /// out at the deadline and the waiter returns `Timeout`. That is what makes
    /// the green arm above evidence about the observer rather than evidence
    /// that the timer was disarmed.
    #[tokio::test(start_paused = true)]
    async fn await_completion_times_out_when_completed_is_suppressed() {
        let bus = Arc::new(AgentBus::new(16));
        let bus_clone = Arc::clone(&bus);
        let waiter = tokio::spawn(async move {
            bus_clone
                .await_completion("child", Duration::from_millis(500))
                .await
        });

        await_subscribers(&bus, 1).await;

        // The unrelated event MUST NOT resolve the waiter.
        bus.publish(AgentMessage::Spawned {
            agent: "child".into(),
            parent_call_id: None,
            timestamp_ms: 0,
        });

        let got = waiter.await.expect("task did not panic");
        assert!(
            matches!(got, Err(AgentBusError::Timeout)),
            "with Completed suppressed the waiter must reach its deadline and \
             report Timeout — a deadline that cannot be reached would make the \
             matching-event test pass for the wrong reason, got {got:?}",
        );
    }
}
