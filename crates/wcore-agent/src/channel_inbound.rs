//! `InboundSubscriber` — the inbound consumer for channel traffic.
//!
//! Structurally, the channel stack already had three of four parts wired:
//! the `Channel` adapters poll their platforms, the `ChannelManager` fans
//! every `ChannelEvent` onto a broadcast, and the pure dispatch kernel
//! (`wcore_channels::evaluate`) decides admit / observe / drop + routes a
//! session key. The fourth part — the consumer that actually *subscribes*
//! to that broadcast, runs the kernel, and drives an agent turn on admit —
//! was missing. This module is that consumer.
//!
//! The subscriber owns no engine logic itself: it drives turns through the
//! [`TurnDispatcher`] trait seam. The real engine-backed dispatcher is a
//! separate later increment; here we build the seam, the subscriber loop,
//! and tests against a mock dispatcher.
//!
//! ## Concurrency model
//!
//! Dispatch is `await`ed inline in the receive loop, so channel turns are
//! naturally SERIALIZED — one turn runs to completion (and its reply is
//! sent) before the next inbound event is processed. This is intentional:
//! it matches the single-engine constraint of the future real dispatcher
//! and keeps per-conversation ordering deterministic. The cost is that a
//! slow turn back-pressures the broadcast (which is bounded), so a long
//! turn can cause `Lagged` for very bursty channels — that is logged and
//! tolerated, not fatal.
//!
//! ## Subscribe-before-start ordering
//!
//! tokio's broadcast drops events emitted before a receiver exists. The
//! subscriber acquires its receiver in [`InboundSubscriber::spawn`], so
//! callers should `spawn` the subscriber BEFORE (or around) the
//! `ChannelManager::start_all` call — otherwise early inbound events
//! emitted between `start_all` and `spawn` are lost.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};
use wcore_channels::{
    evaluate, ChannelEvent, ChannelManager, DedupeCache, InboundPolicy, IncomingMessage,
    OutgoingMessage, TurnAdmission,
};

/// Seam between the inbound subscriber and the agent engine.
///
/// An implementation drives one agent turn for `session_key` from the
/// inbound `msg` (arriving on `channel_name`) and returns the reply text
/// to send back to the conversation, or `None` to send nothing. The real
/// engine-backed implementation lands in a later increment; the subscriber
/// only depends on this trait.
#[async_trait]
pub trait TurnDispatcher: Send + Sync {
    /// Drive one agent turn for `session_key` from `msg` arriving on
    /// `channel_name`. Return `Some(reply_text)` to send back to the
    /// conversation, or `None` to send nothing. Errors are logged by the
    /// subscriber and do not kill the loop.
    async fn dispatch(
        &self,
        session_key: &str,
        channel_name: &str,
        msg: &IncomingMessage,
    ) -> anyhow::Result<Option<String>>;
}

/// Subscribes to the channel broadcast, runs the dispatch kernel per
/// event, and on admit drives an agent turn through a [`TurnDispatcher`],
/// then sends the reply back through the originating channel.
pub struct InboundSubscriber {
    manager: Arc<Mutex<ChannelManager>>,
    dispatcher: Arc<dyn TurnDispatcher>,
    /// Per-channel access policy, keyed by `channel_name`. A channel ABSENT
    /// from this map uses [`InboundPolicy::default`] — which is fail-closed,
    /// so unknown channels deny everything rather than getting an open
    /// policy.
    policies: HashMap<String, InboundPolicy>,
    /// Shared duplicate-suppression cache. Its key already namespaces by
    /// platform / account / message-id, so one cache covers all channels.
    dedupe: DedupeCache,
    /// Runtime kill switch. When `false`, inbound events are drained (to
    /// keep the broadcast from lagging) but processed no further.
    enabled: Arc<AtomicBool>,
}

impl InboundSubscriber {
    /// Construct a subscriber. `dedupe_ttl_ms` / `dedupe_max_size` size the
    /// shared [`DedupeCache`] (see its docs for the `== 0` "disabled"
    /// semantics).
    pub fn new(
        manager: Arc<Mutex<ChannelManager>>,
        dispatcher: Arc<dyn TurnDispatcher>,
        policies: HashMap<String, InboundPolicy>,
        dedupe_ttl_ms: u64,
        dedupe_max_size: usize,
    ) -> Self {
        Self {
            manager,
            dispatcher,
            policies,
            dedupe: DedupeCache::new(dedupe_ttl_ms, dedupe_max_size),
            enabled: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Clone of the kill switch so the host can disable the subscriber at
    /// runtime. Setting it to `false` stops dispatch (events keep draining
    /// so the broadcast does not lag); setting it back to `true` resumes.
    pub fn kill_switch(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.enabled)
    }

    /// Spawn the subscribe loop. Consumes `self` and returns the task
    /// handle.
    ///
    /// The broadcast receiver is acquired ONCE here (the manager lock is
    /// dropped immediately afterward — it is never held across the loop).
    /// Because tokio broadcast drops events emitted before a receiver
    /// exists, callers should `spawn` BEFORE/around `ChannelManager::
    /// start_all` so early inbound events are not missed.
    pub async fn spawn(self) -> tokio::task::JoinHandle<()> {
        // Acquire the broadcast receiver once, then drop the manager lock.
        let mut rx = {
            let guard = self.manager.lock().await;
            guard.subscribe()
        };

        // Monotonic clock base; per-event millis are derived from this.
        let start = std::time::Instant::now();

        let manager = self.manager;
        let dispatcher = self.dispatcher;
        let policies = self.policies;
        let enabled = self.enabled;
        // The loop owns the dedupe cache (mutated per non-short-circuited
        // event).
        let mut dedupe = self.dedupe;

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(tagged) => {
                        // Kill switch: keep draining so the broadcast does
                        // not lag, but process nothing.
                        if !enabled.load(Ordering::Relaxed) {
                            continue;
                        }

                        // Only message events drive turns; lifecycle
                        // variants are ignored.
                        let msg = match tagged.event {
                            ChannelEvent::MessageReceived { msg } => msg,
                            _ => continue,
                        };

                        // Saturating cast of monotonic millis since base.
                        let now_ms = start.elapsed().as_millis() as u64;

                        // Absent channel -> fail-closed default policy.
                        let policy = policies
                            .get(&tagged.channel_name)
                            .cloned()
                            .unwrap_or_default();

                        let outcome =
                            evaluate(&tagged.channel_name, &msg, &policy, &mut dedupe, now_ms);

                        match outcome.admission {
                            TurnAdmission::Dispatch => {
                                let session_key = match outcome.session_key {
                                    Some(k) => k,
                                    None => {
                                        // Kernel contract: Dispatch always
                                        // carries a session key. Defensive.
                                        tracing::error!(
                                            channel = %tagged.channel_name,
                                            "dispatch admission without a session key; skipping"
                                        );
                                        continue;
                                    }
                                };

                                // Ack state machine (best-effort, never
                                // fatal): 👀 on receipt, a typing keepalive
                                // while the turn runs, then ✅/❌ on
                                // completion — gated by the channel's ack mode.
                                let ack = policy.ack;
                                if ack.reactions() {
                                    let g = manager.lock().await;
                                    if let Err(e) = g
                                        .react_on(
                                            &tagged.channel_name,
                                            &msg.conversation_id,
                                            &msg.id,
                                            "👀",
                                        )
                                        .await
                                    {
                                        tracing::debug!(
                                            channel = %tagged.channel_name,
                                            error = %e,
                                            "ack 'seen' reaction failed (non-fatal)"
                                        );
                                    }
                                }
                                // Abort-on-drop guard: the keepalive is killed
                                // the instant the turn completes AND if this
                                // subscriber task is itself cancelled
                                // mid-dispatch (a bare JoinHandle drop does NOT
                                // abort the task; the guard's Drop does).
                                let _typing_guard = ack.typing().then(|| {
                                    AbortOnDrop(spawn_typing_keepalive(
                                        std::sync::Arc::clone(&manager),
                                        tagged.channel_name.clone(),
                                        msg.conversation_id.clone(),
                                    ))
                                });

                                let dispatch_result = dispatcher
                                    .dispatch(&session_key, &tagged.channel_name, &msg)
                                    .await;

                                drop(_typing_guard);
                                if ack.reactions() {
                                    let emoji = if dispatch_result.is_ok() { "✅" } else { "❌" };
                                    let g = manager.lock().await;
                                    let _ = g
                                        .react_on(
                                            &tagged.channel_name,
                                            &msg.conversation_id,
                                            &msg.id,
                                            emoji,
                                        )
                                        .await;
                                }

                                match dispatch_result {
                                    Ok(Some(reply)) => {
                                        let outgoing = OutgoingMessage {
                                            conversation_id: msg.conversation_id.clone(),
                                            text: reply,
                                            reply_to: msg.thread_id.clone(),
                                            attachments: Vec::new(),
                                        };
                                        let guard = manager.lock().await;
                                        if let Err(e) =
                                            guard.send_to(&tagged.channel_name, outgoing).await
                                        {
                                            tracing::warn!(
                                                channel = %tagged.channel_name,
                                                error = %e,
                                                "failed to send inbound reply"
                                            );
                                        }
                                        drop(guard);
                                    }
                                    Ok(None) => {
                                        // Turn produced no reply; nothing to
                                        // send.
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "inbound turn dispatch failed");
                                    }
                                }
                            }
                            TurnAdmission::ObserveOnly => {
                                tracing::debug!(
                                    channel = %tagged.channel_name,
                                    "observed, no turn"
                                );
                                // TODO(phase): record observe-only into session history
                            }
                            TurnAdmission::Drop { .. } => {
                                // Never log message content or sender ids —
                                // only the channel name + content-free
                                // reason.
                                if let Some(reason) = outcome.deny_reason {
                                    tracing::info!(
                                        channel = %tagged.channel_name,
                                        reason = %reason,
                                        "inbound denied"
                                    );
                                } else {
                                    tracing::trace!(
                                        channel = %tagged.channel_name,
                                        "inbound dropped"
                                    );
                                }
                            }
                            TurnAdmission::Handled => {
                                // Already handled upstream; take no action.
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "inbound subscriber lagged");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Manager dropped its sender — no more events will
                        // ever arrive. End the task.
                        break;
                    }
                }
            }
        })
    }
}

/// Aborts the wrapped task when dropped. Used for the typing keepalive so it
/// is killed both on normal turn completion (explicit `drop`) and if the
/// owning subscriber task is cancelled mid-turn (a dropped `JoinHandle` alone
/// does NOT abort the task it refers to).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Spawn a best-effort typing-indicator keepalive for `conversation_id` on
/// `channel`. Sends a typing signal immediately, then refreshes every 5s
/// until the wrapping [`AbortOnDrop`] guard is dropped (on turn completion or
/// subscriber cancellation). Each send locks the manager only briefly;
/// failures (platform has no typing API, transient error) are ignored.
fn spawn_typing_keepalive(
    manager: Arc<Mutex<ChannelManager>>,
    channel: String,
    conversation_id: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            {
                let guard = manager.lock().await;
                let _ = guard.send_typing_to(&channel, &conversation_id).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use wcore_channels::{
        Channel, ChannelError, ChatType, DmPolicy, MessageReceipt,
    };

    /// Shared outbound log handle — what a `CapturingChannel` records.
    type OutboundLog = Arc<Mutex<Vec<OutgoingMessage>>>;

    /// Test channel that emits a fixed queue of inbound messages (one per
    /// `poll_events` call, in order) and records every outbound into a
    /// shared log that the test clones out BEFORE registration. Unlike
    /// `MockChannel.sent`, the log is reachable once the channel is boxed.
    struct CapturingChannel {
        name: String,
        started: bool,
        inbound: VecDeque<IncomingMessage>,
        outbound: OutboundLog,
        next_id: u64,
    }

    impl CapturingChannel {
        fn new(name: &str, inbound: VecDeque<IncomingMessage>) -> (Self, OutboundLog) {
            let outbound: OutboundLog = Arc::new(Mutex::new(Vec::new()));
            let ch = Self {
                name: name.to_string(),
                started: false,
                inbound,
                outbound: Arc::clone(&outbound),
                next_id: 0,
            };
            (ch, outbound)
        }
    }

    #[async_trait]
    impl Channel for CapturingChannel {
        fn name(&self) -> &str {
            &self.name
        }

        fn platform(&self) -> &str {
            "mock"
        }

        async fn start(&mut self) -> Result<(), ChannelError> {
            self.started = true;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), ChannelError> {
            self.started = false;
            Ok(())
        }

        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            if !self.started && self.inbound.is_empty() {
                return Err(ChannelError::NotStarted);
            }
            // Emit one queued inbound per poll, then nothing.
            match self.inbound.pop_front() {
                Some(msg) => Ok(vec![ChannelEvent::MessageReceived { msg }]),
                None => Ok(Vec::new()),
            }
        }

        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            let id = format!("cap-out-{}", self.next_id);
            self.next_id += 1;
            let receipt = MessageReceipt {
                id,
                conversation_id: msg.conversation_id.clone(),
                ts_secs: 0,
            };
            self.outbound.lock().await.push(msg);
            Ok(receipt)
        }

        fn config_schema(&self) -> &str {
            r#"{"name":"string","platform":"mock"}"#
        }
    }

    /// `(session_key, channel_name)` recorded per dispatcher call.
    type CallLog = Arc<Mutex<Vec<(String, String)>>>;
    /// Dispatcher invocation counter.
    type CallCount = Arc<AtomicUsize>;

    /// Records `(session_key, channel_name)` per call + a counter, and
    /// always returns `Ok(Some("pong"))`.
    struct MockDispatcher {
        calls: CallLog,
        count: CallCount,
    }

    impl MockDispatcher {
        fn new() -> (Self, CallLog, CallCount) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let count = Arc::new(AtomicUsize::new(0));
            let d = Self {
                calls: Arc::clone(&calls),
                count: Arc::clone(&count),
            };
            (d, calls, count)
        }
    }

    #[async_trait]
    impl TurnDispatcher for MockDispatcher {
        async fn dispatch(
            &self,
            session_key: &str,
            channel_name: &str,
            _msg: &IncomingMessage,
        ) -> anyhow::Result<Option<String>> {
            self.calls
                .lock()
                .await
                .push((session_key.to_string(), channel_name.to_string()));
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(Some("pong".into()))
        }
    }

    /// Build a Direct (DM) inbound with the given id.
    fn dm(id: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new(id, "c1", "alice", "ping", 0);
        m.sender_id = "u1".into();
        m.chat_type = ChatType::Direct;
        m
    }

    /// Register a `CapturingChannel` with the queued inbound, build a
    /// subscriber over the given policy map, and spawn it. Returns the
    /// shared manager, the outbound log, the dispatcher call log, and the
    /// dispatch counter.
    async fn harness(
        channel_name: &str,
        inbound: VecDeque<IncomingMessage>,
        policies: HashMap<String, InboundPolicy>,
        pre_spawn: impl FnOnce(&InboundSubscriber),
    ) -> (
        Arc<Mutex<ChannelManager>>,
        OutboundLog,
        Arc<Mutex<Vec<(String, String)>>>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let (ch, outbound) = CapturingChannel::new(channel_name, inbound);

        // Fast poll so the queued inbound surfaces quickly under test.
        let mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
        let manager = Arc::new(Mutex::new(mgr));

        let (dispatcher, calls, count) = MockDispatcher::new();
        let subscriber = InboundSubscriber::new(
            Arc::clone(&manager),
            Arc::new(dispatcher),
            policies,
            60_000,
            1024,
        );
        pre_spawn(&subscriber);

        // Spawn the subscriber BEFORE start_all so no early event is lost.
        let handle = subscriber.spawn().await;

        {
            let mut guard = manager.lock().await;
            guard.register(Box::new(ch)).await;
            guard.start_all().await.unwrap();
        }

        (manager, outbound, calls, count, handle)
    }

    /// Poll a shared `Vec` log until it reaches `want` len or the deadline
    /// elapses. Returns the final length observed.
    async fn wait_for_len<T>(log: &Arc<Mutex<Vec<T>>>, want: usize, within: Duration) -> usize {
        let deadline = std::time::Instant::now() + within;
        loop {
            let len = log.lock().await.len();
            if len >= want || std::time::Instant::now() >= deadline {
                return len;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn open_dm_policy() -> InboundPolicy {
        InboundPolicy {
            dm: DmPolicy::Open,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn allowed_dm_dispatches_and_replies() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        let mut q = VecDeque::new();
        q.push_back(dm("m1"));

        let (manager, outbound, calls, count, handle) =
            harness("slack", q, policies, |_| {}).await;

        // Wait for the dispatcher to be called and the reply to be sent.
        let dispatched = wait_for_len(&calls, 1, Duration::from_secs(2)).await;
        let replied = wait_for_len(&outbound, 1, Duration::from_secs(2)).await;

        assert_eq!(count.load(Ordering::SeqCst), 1, "dispatched exactly once");
        assert_eq!(dispatched, 1);
        assert_eq!(replied, 1, "exactly one reply sent");

        let calls = calls.lock().await;
        assert_eq!(
            calls[0],
            (
                "agent:main:slack:dm:c1".to_string(),
                "slack".to_string()
            )
        );

        let out = outbound.lock().await;
        assert_eq!(out[0].text, "pong");
        assert_eq!(out[0].conversation_id, "c1");

        manager.lock().await.stop_all().await.unwrap();
        handle.abort();
    }

    #[tokio::test]
    async fn denied_dm_not_dispatched() {
        // Empty policy map -> fail-closed default for the channel.
        let policies = HashMap::new();

        let mut q = VecDeque::new();
        q.push_back(dm("m1"));

        let (manager, outbound, calls, count, handle) =
            harness("slack", q, policies, |_| {}).await;

        // Give the loop ample time to process and (not) dispatch.
        let dispatched = wait_for_len(&calls, 1, Duration::from_millis(500)).await;

        assert_eq!(count.load(Ordering::SeqCst), 0, "fail-closed: no dispatch");
        assert_eq!(dispatched, 0);
        assert_eq!(outbound.lock().await.len(), 0, "no reply sent");

        manager.lock().await.stop_all().await.unwrap();
        handle.abort();
    }

    #[tokio::test]
    async fn duplicate_id_dispatched_once() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        // Same message id twice — across two polls.
        let mut q = VecDeque::new();
        q.push_back(dm("m1"));
        q.push_back(dm("m1"));

        let (manager, _outbound, calls, count, handle) =
            harness("slack", q, policies, |_| {}).await;

        // Wait for the first dispatch, then give the duplicate time to be
        // (correctly) suppressed.
        let _ = wait_for_len(&calls, 1, Duration::from_secs(2)).await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "duplicate id deduped to a single dispatch"
        );

        manager.lock().await.stop_all().await.unwrap();
        handle.abort();
    }

    #[tokio::test]
    async fn self_message_not_dispatched() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        let mut self_msg = dm("m1");
        self_msg.is_self = true;
        let mut q = VecDeque::new();
        q.push_back(self_msg);

        let (manager, outbound, calls, count, handle) =
            harness("slack", q, policies, |_| {}).await;

        let dispatched = wait_for_len(&calls, 1, Duration::from_millis(500)).await;

        assert_eq!(count.load(Ordering::SeqCst), 0, "loop-guard: no dispatch");
        assert_eq!(dispatched, 0);
        assert_eq!(outbound.lock().await.len(), 0);

        manager.lock().await.stop_all().await.unwrap();
        handle.abort();
    }

    #[tokio::test]
    async fn kill_switch_disables_dispatch() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        let mut q = VecDeque::new();
        q.push_back(dm("m1"));

        // Flip the kill switch OFF before the event is injected/processed.
        let (manager, outbound, calls, count, handle) =
            harness("slack", q, policies, |sub| {
                sub.kill_switch().store(false, Ordering::Relaxed);
            })
            .await;

        let dispatched = wait_for_len(&calls, 1, Duration::from_millis(500)).await;

        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "kill switch off: event drained, not dispatched"
        );
        assert_eq!(dispatched, 0);
        assert_eq!(outbound.lock().await.len(), 0);

        manager.lock().await.stop_all().await.unwrap();
        handle.abort();
    }
}
