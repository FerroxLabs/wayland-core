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
//! The broadcast receive loop does ONLY admission — classify / dedup /
//! access / session-key (all in-memory, O(µs)) — then hands each admitted
//! event to a per-session worker over a bounded FIFO and immediately moves
//! on. Because the loop never `await`s a turn, a slow multi-minute turn can
//! no longer back-pressure the broadcast: the drain keeps up with producers
//! and a busy session does NOT starve other sessions (no cross-session
//! head-of-line blocking, and no broadcast `Lagged` caused by turn latency).
//!
//! Each session key gets exactly one worker task that drives its turns
//! SERIALLY — one turn completes and replies before the next for that
//! session — so per-conversation ordering stays deterministic and matches
//! the engine's per-session single-turn constraint. Workers for distinct
//! sessions run concurrently.
//!
//! Two bounds keep the decoupling honest. The per-session FIFO is bounded
//! ([`SESSION_FIFO_CAP`]): if one session receives turns faster than it can
//! run them, the inbound beyond the cap is dropped with a warning rather
//! than blocking the drain loop (blocking would reintroduce the very defect
//! this design removes). The live worker count is bounded
//! ([`MAX_SESSION_WORKERS`]) to cap task proliferation from a flood of
//! distinct conversation ids.
//!
//! ## Subscribe-before-start ordering
//!
//! tokio's broadcast drops events emitted before a receiver exists. The
//! subscriber acquires its receiver in [`InboundSubscriber::spawn`], so
//! callers should `spawn` the subscriber BEFORE (or around) the
//! `ChannelManager::start_all` call — otherwise early inbound events
//! emitted between `start_all` and `spawn` are lost.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::{RwLock, broadcast, mpsc};
use wcore_channels::{
    AckMode, AutoReplyRateLimiter, ChannelEvent, ChannelManager, DedupeCache, InboundPolicy,
    IncomingMessage, OutgoingMessage, TurnAdmission, evaluate,
};

/// Depth of each per-session FIFO. Bounds how far one session may fall
/// behind (turns are multi-minute) before the oldest-beyond-cap inbound is
/// dropped rather than growing memory unboundedly or — fatally — blocking
/// the broadcast drain loop.
const SESSION_FIFO_CAP: usize = 64;

/// Maximum number of concurrently live session workers. Caps task
/// proliferation from a flood of distinct conversation ids; a new session
/// beyond this — after idle eviction has reclaimed what it can — is dropped
/// with a warning. Mirrors the engine pool's intent to bound per-conversation
/// resource growth.
const MAX_SESSION_WORKERS: usize = 1000;

/// A worker idle (no admitted event) at least this long is eligible for
/// eviction when a new session needs a slot. Without this the worker map
/// would only ever grow — a `Sender` held in the map keeps its worker parked
/// on `recv()` forever, so workers never exit on their own, and a long-lived
/// bot serving many distinct conversations would permanently hit
/// [`MAX_SESSION_WORKERS`] and silently drop every new session. Eviction
/// drops the entry's `Sender`, the worker drains any remaining queue and
/// exits, and the session's engine state still persists in the dispatcher's
/// per-key engine pool — so a later message simply respawns a fresh worker.
const WORKER_IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Pick the outbound reply target for a turn's reply.
///
/// Prefers `reply_to_message_id` — the specific message the inbound quoted,
/// which reply-quoting platforms (Telegram, Discord, WhatsApp, Matrix, …) need
/// to thread in-context — and falls back to `thread_id` (the thread root that
/// Slack's `thread_ts` requires). For Slack the two coincide whenever
/// `reply_to_message_id` is set, so this is a strict no-op there; for the other
/// connectors it carries the quoted id that was previously dropped. Returns
/// `None` when the inbound is neither a reply nor in a thread.
fn outbound_reply_target(msg: &IncomingMessage) -> Option<String> {
    msg.reply_to_message_id
        .clone()
        .or_else(|| msg.thread_id.clone())
}

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
    manager: Arc<RwLock<ChannelManager>>,
    dispatcher: Arc<dyn TurnDispatcher>,
    /// Per-channel access policy, keyed by `channel_name`. A channel ABSENT
    /// from this map uses [`InboundPolicy::default`] — which is fail-closed,
    /// so unknown channels deny everything rather than getting an open
    /// policy.
    policies: HashMap<String, InboundPolicy>,
    /// Shared duplicate-suppression cache. Its key already namespaces by
    /// platform / account / message-id, so one cache covers all channels.
    dedupe: DedupeCache,
    /// Per-conversation rolling-window guard on autonomous auto-replies. Two
    /// agents wired to the same channel can auto-reply to each other forever
    /// (wayland#574); this caps how many autonomous sends one conversation may
    /// emit per window, breaking the ping-pong. Shared (behind a `std::Mutex`)
    /// because each per-session worker runs its own turn and must consult one
    /// limiter; the critical section is a bounded map op, never held across an
    /// `await`.
    rate_limiter: Arc<StdMutex<AutoReplyRateLimiter>>,
    /// Runtime kill switch. When `false`, inbound events are drained (to
    /// keep the broadcast from lagging) but processed no further.
    enabled: Arc<AtomicBool>,
}

impl InboundSubscriber {
    /// Construct a subscriber. `dedupe_ttl_ms` / `dedupe_max_size` size the
    /// shared [`DedupeCache`] (see its docs for the `== 0` "disabled"
    /// semantics).
    pub fn new(
        manager: Arc<RwLock<ChannelManager>>,
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
            rate_limiter: Arc::new(StdMutex::new(AutoReplyRateLimiter::default())),
            enabled: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Override the autonomous auto-reply rate limit (default:
    /// [`wcore_channels::DEFAULT_MAX_AUTO_REPLIES`] per
    /// [`wcore_channels::DEFAULT_AUTO_REPLY_WINDOW`]). A `window` of
    /// [`std::time::Duration::ZERO`] disables the guard entirely. Builder-style
    /// so the default construction path stays unchanged.
    pub fn with_auto_reply_limit(
        mut self,
        max_sends: usize,
        window: std::time::Duration,
        conversation_cap: usize,
    ) -> Self {
        self.rate_limiter = Arc::new(StdMutex::new(AutoReplyRateLimiter::new(
            max_sends,
            window,
            conversation_cap,
        )));
        self
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
            let guard = self.manager.read().await;
            guard.subscribe()
        };

        // Monotonic clock base; per-event millis are derived from this.
        let start = std::time::Instant::now();

        let manager = self.manager;
        let dispatcher = self.dispatcher;
        let policies = self.policies;
        let enabled = self.enabled;
        // Shared across the per-session workers so every autonomous reply is
        // counted against one per-conversation budget.
        let rate_limiter = self.rate_limiter;
        // The loop owns the dedupe cache (mutated per non-short-circuited
        // event).
        let mut dedupe = self.dedupe;

        tokio::spawn(async move {
            // Per-session FIFO workers, owned solely by this drain task (no
            // lock, single-threaded ownership). A worker exists iff an event
            // has been admitted for its session key and it has not been torn
            // down. Dropping/removing its `Sender` closes the worker.
            let mut workers: HashMap<String, SessionWorker> = HashMap::new();
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

                                // Hand the admitted event to its per-session
                                // worker over a bounded FIFO and move on. The
                                // turn — ack machine + dispatch + reply — runs
                                // in the worker (see `run_turn`), NEVER here:
                                // awaiting it inline is exactly the defect this
                                // design removes.
                                let ev = AdmittedEvent {
                                    channel_name: tagged.channel_name.clone(),
                                    msg,
                                    ack: policy.ack,
                                };
                                enqueue_to_worker(
                                    &mut workers,
                                    session_key,
                                    ev,
                                    &manager,
                                    &dispatcher,
                                    &rate_limiter,
                                );
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
                        // Producers outran the drain loop and the bounded
                        // broadcast overwrote `n` events before admission.
                        // With dispatch decoupled this should be rare (the
                        // loop only does O(µs) admission) — if it persists,
                        // producers are flooding faster than classify/dedup
                        // can run, not "a turn is slow". Surfaced under a
                        // distinct target so it can be alerted on.
                        tracing::warn!(
                            target: "wcore_agent::channel_inbound",
                            skipped = n,
                            "inbound broadcast lagged; events dropped before admission"
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Manager dropped its sender — no more events will
                        // ever arrive. Abort in-flight workers (turns are
                        // multi-minute; we do not wait them out on shutdown)
                        // and end the task. Dropping `workers` would also
                        // close each FIFO, but an explicit abort stops the
                        // current turn promptly.
                        for (_, w) in workers.drain() {
                            w.handle.abort();
                        }
                        break;
                    }
                }
            }
        })
    }
}

/// One admitted inbound event handed from the broadcast drain loop to the
/// per-session worker that owns its turn. Carries everything the ack +
/// dispatch + reply machine needs; the session key is the worker's map key
/// (and is passed to the worker separately), so it is not repeated here.
struct AdmittedEvent {
    channel_name: String,
    msg: IncomingMessage,
    ack: AckMode,
}

/// A per-session worker: a bounded FIFO `tx` feeding a spawned task that
/// drives turns SERIALLY for one session key. The drain loop owns one of
/// these per live session; dropping/removing it closes the FIFO and the
/// worker exits once its queue is empty.
struct SessionWorker {
    tx: mpsc::Sender<AdmittedEvent>,
    handle: tokio::task::JoinHandle<()>,
    /// When the most recent event was enqueued. Used for idle eviction so a
    /// flood of distinct sessions cannot permanently exhaust the worker cap.
    last_active: std::time::Instant,
}

/// Route an admitted event to its session worker, spawning one on first use.
///
/// This runs in the broadcast drain loop and must never block — every path
/// either enqueues in O(µs) or drops with a warning. A full per-session FIFO
/// drops the event (the session is already saturated with multi-minute
/// turns); exceeding [`MAX_SESSION_WORKERS`] drops a brand-new session.
fn enqueue_to_worker(
    workers: &mut HashMap<String, SessionWorker>,
    session_key: String,
    ev: AdmittedEvent,
    manager: &Arc<RwLock<ChannelManager>>,
    dispatcher: &Arc<dyn TurnDispatcher>,
    rate_limiter: &Arc<StdMutex<AutoReplyRateLimiter>>,
) {
    // Fast path: a live worker already exists for this session.
    let ev = match workers.get_mut(&session_key) {
        Some(worker) => match worker.tx.try_send(ev) {
            Ok(()) => {
                worker.last_active = std::time::Instant::now();
                return;
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    target: "wcore_agent::channel_inbound",
                    session_key = %session_key,
                    dropped = 1,
                    "per-session FIFO full; inbound message dropped"
                );
                return;
            }
            // The worker task ended (panicked, or torn down) and dropped its
            // receiver. Reclaim the returned event, evict the dead entry, and
            // fall through to respawn.
            Err(mpsc::error::TrySendError::Closed(returned)) => {
                workers.remove(&session_key);
                returned
            }
        },
        None => ev,
    };

    // No live worker for this session: we are about to spawn one. First
    // reclaim slots held by exited or long-idle workers so a flood of
    // distinct sessions cannot permanently exhaust the cap (the worker map
    // would otherwise only grow).
    prune_idle_workers(workers);

    if workers.len() >= MAX_SESSION_WORKERS {
        tracing::warn!(
            target: "wcore_agent::channel_inbound",
            session_key = %session_key,
            live_workers = workers.len(),
            "session worker cap reached; inbound message dropped"
        );
        return;
    }

    let (tx, rx) = mpsc::channel(SESSION_FIFO_CAP);
    let handle = spawn_session_worker(
        session_key.clone(),
        rx,
        Arc::clone(manager),
        Arc::clone(dispatcher),
        Arc::clone(rate_limiter),
    );
    // The FIFO is fresh and we hold the only sender, so this can be neither
    // Full nor Closed; the event is enqueued unconditionally.
    let _ = tx.try_send(ev);
    workers.insert(
        session_key,
        SessionWorker {
            tx,
            handle,
            last_active: std::time::Instant::now(),
        },
    );
}

/// Remove workers that have exited (panicked / drained-and-closed) or been
/// idle past [`WORKER_IDLE_TTL`]. Dropping an entry drops its `Sender`, which
/// lets the worker drain any queued events and exit. Called only on the
/// spawn-a-new-session path, so its O(n) scan does not run per event for
/// already-live sessions. Eviction is safe: per-session turn ordering
/// ultimately rests on the dispatcher's per-key engine mutex, and the
/// session's engine state survives in that pool, so a later message for an
/// evicted session simply respawns a fresh worker.
fn prune_idle_workers(workers: &mut HashMap<String, SessionWorker>) {
    workers.retain(|_, w| !w.handle.is_finished() && w.last_active.elapsed() < WORKER_IDLE_TTL);
}

/// Spawn the worker task for one session key. It consumes its FIFO in order,
/// running each turn to completion (and reply) before the next, and exits
/// when the FIFO closes (all senders dropped).
fn spawn_session_worker(
    session_key: String,
    mut rx: mpsc::Receiver<AdmittedEvent>,
    manager: Arc<RwLock<ChannelManager>>,
    dispatcher: Arc<dyn TurnDispatcher>,
    rate_limiter: Arc<StdMutex<AutoReplyRateLimiter>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            run_turn(&session_key, &ev, &manager, &dispatcher, &rate_limiter).await;
        }
    })
}

/// Drive one agent turn for an admitted event: the best-effort ack state
/// machine (👀 on receipt → typing keepalive while running → ✅/❌ on
/// completion, gated by the channel's [`AckMode`]) around the dispatch, then
/// send any reply back through the originating channel. Runs inside the
/// session worker, so a slow turn here never touches the broadcast drain loop.
async fn run_turn(
    session_key: &str,
    ev: &AdmittedEvent,
    manager: &Arc<RwLock<ChannelManager>>,
    dispatcher: &Arc<dyn TurnDispatcher>,
    rate_limiter: &Arc<StdMutex<AutoReplyRateLimiter>>,
) {
    let AdmittedEvent {
        channel_name,
        msg,
        ack,
    } = ev;
    let ack = *ack;

    if ack.reactions() {
        let g = manager.read().await;
        if let Err(e) = g
            .react_on(channel_name, &msg.conversation_id, &msg.id, "👀")
            .await
        {
            tracing::debug!(
                channel = %channel_name,
                error = %e,
                "ack 'seen' reaction failed (non-fatal)"
            );
        }
    }

    // Abort-on-drop guard: the keepalive is killed the instant the turn
    // completes AND if this worker task is itself cancelled mid-dispatch (a
    // bare JoinHandle drop does NOT abort the task; the guard's Drop does).
    let _typing_guard = ack.typing().then(|| {
        AbortOnDrop(spawn_typing_keepalive(
            Arc::clone(manager),
            channel_name.clone(),
            msg.conversation_id.clone(),
        ))
    });

    let dispatch_result = dispatcher.dispatch(session_key, channel_name, msg).await;

    drop(_typing_guard);
    if ack.reactions() {
        let emoji = if dispatch_result.is_ok() {
            "✅"
        } else {
            "❌"
        };
        let g = manager.read().await;
        let _ = g
            .react_on(channel_name, &msg.conversation_id, &msg.id, emoji)
            .await;
    }

    match dispatch_result {
        Ok(Some(reply)) => {
            // Per-conversation ping-pong guard (wayland#574): suppress the
            // autonomous reply once this conversation has emitted its quota of
            // autonomous sends within the rolling window. The lock is released
            // before any `.await` (bounded map op only — never held across the
            // send); a poisoned mutex is recovered rather than panicking, since
            // the critical section cannot itself panic.
            let allowed = {
                let mut limiter = rate_limiter
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                limiter.check_and_record(session_key, std::time::Instant::now())
            };
            if !allowed {
                // Content-free: log only the session key, never message text.
                tracing::warn!(
                    target: "wcore_agent::channel_inbound",
                    session_key = %session_key,
                    "autonomous reply suppressed: per-conversation rate limit hit (ping-pong guard)"
                );
                return;
            }

            let outgoing = OutgoingMessage {
                conversation_id: msg.conversation_id.clone(),
                text: reply,
                reply_to: outbound_reply_target(msg),
                attachments: Vec::new(),
            };
            let guard = manager.read().await;
            if let Err(e) = guard.send_to(channel_name, outgoing).await {
                tracing::warn!(
                    channel = %channel_name,
                    error = %e,
                    "failed to send inbound reply"
                );
            }
            drop(guard);
        }
        Ok(None) => {
            // Turn produced no reply; nothing to send.
        }
        Err(e) => {
            tracing::warn!(error = %e, "inbound turn dispatch failed");
        }
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
    manager: Arc<RwLock<ChannelManager>>,
    channel: String,
    conversation_id: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            {
                let guard = manager.read().await;
                let _ = guard.send_typing_to(&channel, &conversation_id).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    use std::collections::VecDeque;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use wcore_channels::{Channel, ChannelError, ChatType, DmPolicy, MessageReceipt};

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

    /// Build a DM inbound with an explicit conversation id, so distinct
    /// `conv` values yield distinct session keys (and thus distinct workers).
    fn dm_conv(id: &str, conv: &str) -> IncomingMessage {
        let mut m = IncomingMessage::new(id, conv, "alice", "ping", 0);
        m.sender_id = "u1".into();
        m.chat_type = ChatType::Direct;
        m
    }

    /// Dispatcher that sleeps `delay` per call before counting it — models a
    /// slow multi-minute turn. Returns `Ok(None)` (no reply) to keep the test
    /// focused on dispatch admission, not outbound.
    struct SlowDispatcher {
        count: CallCount,
        delay: Duration,
    }

    impl SlowDispatcher {
        fn new(delay: Duration) -> (Self, CallCount) {
            let count = Arc::new(AtomicUsize::new(0));
            let d = Self {
                count: Arc::clone(&count),
                delay,
            };
            (d, count)
        }
    }

    #[async_trait]
    impl TurnDispatcher for SlowDispatcher {
        async fn dispatch(
            &self,
            _session_key: &str,
            _channel_name: &str,
            _msg: &IncomingMessage,
        ) -> anyhow::Result<Option<String>> {
            tokio::time::sleep(self.delay).await;
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    #[test]
    fn outbound_reply_target_prefers_reply_id_over_thread() {
        let mut m = dm("1");
        m.reply_to_message_id = Some("wamid.QUOTE".into());
        m.thread_id = Some("thread-root".into());
        // A reply must quote the specific message, not the thread root.
        assert_eq!(outbound_reply_target(&m), Some("wamid.QUOTE".to_string()));
    }

    #[test]
    fn outbound_reply_target_falls_back_to_thread() {
        let mut m = dm("2");
        m.thread_id = Some("1700000001.000100".into());
        // No quoted id (Slack thread root / in-thread message): use thread_id.
        assert_eq!(
            outbound_reply_target(&m),
            Some("1700000001.000100".to_string())
        );
    }

    #[test]
    fn outbound_reply_target_none_when_neither() {
        // A fresh, unthreaded message replies without a target.
        assert_eq!(outbound_reply_target(&dm("3")), None);
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
        Arc<RwLock<ChannelManager>>,
        OutboundLog,
        Arc<Mutex<Vec<(String, String)>>>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let (ch, outbound) = CapturingChannel::new(channel_name, inbound);

        // Fast poll so the queued inbound surfaces quickly under test.
        let mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
        let manager = Arc::new(RwLock::new(mgr));

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
            let mut guard = manager.write().await;
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

        let (manager, outbound, calls, count, handle) = harness("slack", q, policies, |_| {}).await;

        // Wait for the dispatcher to be called and the reply to be sent.
        let dispatched = wait_for_len(&calls, 1, Duration::from_secs(2)).await;
        let replied = wait_for_len(&outbound, 1, Duration::from_secs(2)).await;

        assert_eq!(count.load(Ordering::SeqCst), 1, "dispatched exactly once");
        assert_eq!(dispatched, 1);
        assert_eq!(replied, 1, "exactly one reply sent");

        let calls = calls.lock().await;
        assert_eq!(
            calls[0],
            ("agent:main:slack:dm:c1".to_string(), "slack".to_string())
        );

        let out = outbound.lock().await;
        assert_eq!(out[0].text, "pong");
        assert_eq!(out[0].conversation_id, "c1");

        manager.write().await.stop_all().await.unwrap();
        handle.abort();
    }

    #[tokio::test]
    async fn denied_dm_not_dispatched() {
        // Empty policy map -> fail-closed default for the channel.
        let policies = HashMap::new();

        let mut q = VecDeque::new();
        q.push_back(dm("m1"));

        let (manager, outbound, calls, count, handle) = harness("slack", q, policies, |_| {}).await;

        // Give the loop ample time to process and (not) dispatch.
        let dispatched = wait_for_len(&calls, 1, Duration::from_millis(500)).await;

        assert_eq!(count.load(Ordering::SeqCst), 0, "fail-closed: no dispatch");
        assert_eq!(dispatched, 0);
        assert_eq!(outbound.lock().await.len(), 0, "no reply sent");

        manager.write().await.stop_all().await.unwrap();
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

        manager.write().await.stop_all().await.unwrap();
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

        let (manager, outbound, calls, count, handle) = harness("slack", q, policies, |_| {}).await;

        let dispatched = wait_for_len(&calls, 1, Duration::from_millis(500)).await;

        assert_eq!(count.load(Ordering::SeqCst), 0, "loop-guard: no dispatch");
        assert_eq!(dispatched, 0);
        assert_eq!(outbound.lock().await.len(), 0);

        manager.write().await.stop_all().await.unwrap();
        handle.abort();
    }

    /// R13 regression: a slow turn must NOT cause inbound events to be
    /// silently dropped by lagging the bounded broadcast.
    ///
    /// 300 distinct-session DMs are produced ~1ms apart while every dispatch
    /// sleeps 50ms. Under the old inline design the receive loop consumed one
    /// event per ~50ms, so the 256-slot broadcast overflowed within ~300ms
    /// and `Lagged` silently discarded most of the 300. With dispatch
    /// decoupled into per-session workers, the drain loop only does O(µs)
    /// admission, keeps up with producers, and fans every session out to its
    /// own worker — so all 300 are dispatched, none dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_turns_do_not_drop_inbound_via_broadcast_lag() {
        const N: usize = 300;
        // The assertion below is `dispatched == N`. Keep N strictly under the
        // worker cap so a shortfall can ONLY mean broadcast-lag drops (the
        // defect under test), never a worker-cap drop. Enforced at compile
        // time so the invariant cannot silently rot.
        const _: () = assert!(N < MAX_SESSION_WORKERS);

        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        // Distinct conversation ids => distinct session keys => N workers.
        let mut q = VecDeque::new();
        for i in 0..N {
            q.push_back(dm_conv(&format!("m{i}"), &format!("c{i}")));
        }

        let (ch, _outbound) = CapturingChannel::new("slack", q);
        // Produce far faster than any single turn runs.
        let mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(1));
        let manager = Arc::new(RwLock::new(mgr));

        let (dispatcher, count) = SlowDispatcher::new(Duration::from_millis(50));
        let subscriber = InboundSubscriber::new(
            Arc::clone(&manager),
            Arc::new(dispatcher),
            policies,
            60_000,
            4096,
        );
        let handle = subscriber.spawn().await;

        {
            let mut guard = manager.write().await;
            guard.register(Box::new(ch)).await;
            guard.start_all().await.unwrap();
        }

        // All N workers run their 50ms turn concurrently, so completion is
        // bounded by production time + one turn, not N * 50ms. Generous slack.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if count.load(Ordering::SeqCst) >= N || std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(
            count.load(Ordering::SeqCst),
            N,
            "every inbound must be dispatched; a shortfall means events were dropped by broadcast lag"
        );

        manager.write().await.stop_all().await.unwrap();
        handle.abort();
    }

    /// R13: the worker map must self-heal so a long-lived bot serving many
    /// distinct conversations cannot permanently exhaust the worker cap.
    /// `prune_idle_workers` evicts exited and long-idle workers but keeps
    /// recently-active ones.
    #[tokio::test]
    async fn prune_idle_workers_evicts_idle_and_finished_keeps_active() {
        use std::time::Instant;

        let mut workers: HashMap<String, SessionWorker> = HashMap::new();

        // Active: recent activity, live worker → must survive.
        let (tx_a, mut rx_a) = mpsc::channel::<AdmittedEvent>(SESSION_FIFO_CAP);
        let h_a = tokio::spawn(async move { while rx_a.recv().await.is_some() {} });
        workers.insert(
            "active".into(),
            SessionWorker {
                tx: tx_a,
                handle: h_a,
                last_active: Instant::now(),
            },
        );

        // Idle: live worker but last active past the TTL → must be evicted.
        let (tx_i, mut rx_i) = mpsc::channel::<AdmittedEvent>(SESSION_FIFO_CAP);
        let h_i = tokio::spawn(async move { while rx_i.recv().await.is_some() {} });
        let stale = Instant::now()
            .checked_sub(WORKER_IDLE_TTL + Duration::from_secs(1))
            .expect("stale instant in range");
        workers.insert(
            "idle".into(),
            SessionWorker {
                tx: tx_i,
                handle: h_i,
                last_active: stale,
            },
        );

        // Finished: worker task already exited (recent activity) → evicted
        // because the handle is finished even though it is not idle.
        let (tx_f, _rx_f) = mpsc::channel::<AdmittedEvent>(SESSION_FIFO_CAP);
        let h_f = tokio::spawn(async move {});
        // Let the empty task run to completion before we check is_finished.
        tokio::time::sleep(Duration::from_millis(20)).await;
        workers.insert(
            "finished".into(),
            SessionWorker {
                tx: tx_f,
                handle: h_f,
                last_active: Instant::now(),
            },
        );

        prune_idle_workers(&mut workers);

        assert!(workers.contains_key("active"), "active worker must survive");
        assert!(!workers.contains_key("idle"), "idle worker must be evicted");
        assert!(
            !workers.contains_key("finished"),
            "finished worker must be evicted"
        );
    }

    #[tokio::test]
    async fn kill_switch_disables_dispatch() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        let mut q = VecDeque::new();
        q.push_back(dm("m1"));

        // Flip the kill switch OFF before the event is injected/processed.
        let (manager, outbound, calls, count, handle) = harness("slack", q, policies, |sub| {
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

        manager.write().await.stop_all().await.unwrap();
        handle.abort();
    }

    /// Build a subscriber with an explicit autonomous auto-reply limit, register
    /// a `CapturingChannel` feeding `inbound`, and spawn it. Mirrors [`harness`]
    /// but applies [`InboundSubscriber::with_auto_reply_limit`].
    async fn harness_with_limit(
        channel_name: &str,
        inbound: VecDeque<IncomingMessage>,
        policies: HashMap<String, InboundPolicy>,
        max_sends: usize,
        window: Duration,
    ) -> (
        Arc<RwLock<ChannelManager>>,
        OutboundLog,
        Arc<Mutex<Vec<(String, String)>>>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let (ch, outbound) = CapturingChannel::new(channel_name, inbound);

        let mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(5));
        let manager = Arc::new(RwLock::new(mgr));

        let (dispatcher, calls, count) = MockDispatcher::new();
        let subscriber = InboundSubscriber::new(
            Arc::clone(&manager),
            Arc::new(dispatcher),
            policies,
            60_000,
            1024,
        )
        .with_auto_reply_limit(max_sends, window, 1024);

        let handle = subscriber.spawn().await;

        {
            let mut guard = manager.write().await;
            guard.register(Box::new(ch)).await;
            guard.start_all().await.unwrap();
        }

        (manager, outbound, calls, count, handle)
    }

    /// wayland#574: once a conversation hits its autonomous-send quota within
    /// the window, further auto-replies are suppressed even though every inbound
    /// is distinct (not a self-echo, not a duplicate) — the two-agent ping-pong
    /// case the self/bot loop guard and the Message-ID echo guard both miss.
    /// The turns still run; only the outbound SEND is throttled.
    #[tokio::test]
    async fn over_limit_autonomous_replies_are_suppressed() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        // Five distinct-id messages, all in the same conversation "c1" -> same
        // session key. Cap autonomous sends at 2 within a wide window.
        let mut q = VecDeque::new();
        for i in 0..5 {
            q.push_back(dm_conv(&format!("m{i}"), "c1"));
        }

        let (manager, outbound, calls, count, handle) =
            harness_with_limit("slack", q, policies, 2, Duration::from_secs(600)).await;

        // All five turns dispatch (dispatch is not gated, only the send is).
        let dispatched = wait_for_len(&calls, 5, Duration::from_secs(3)).await;
        assert_eq!(dispatched, 5, "every distinct inbound drives a turn");
        assert_eq!(count.load(Ordering::SeqCst), 5);

        // Give any (incorrect) extra sends a chance to land, then assert the cap.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            outbound.lock().await.len(),
            2,
            "autonomous sends capped at the per-conversation limit"
        );

        manager.write().await.stop_all().await.unwrap();
        handle.abort();
    }

    /// The rate limit is per-conversation: one conversation exhausting its quota
    /// must not throttle a different conversation.
    #[tokio::test]
    async fn rate_limit_is_per_conversation() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        // Two messages each for conversations "c1" and "c2", cap of 1 per
        // conversation. Expect exactly one send per conversation = two total.
        let mut q = VecDeque::new();
        q.push_back(dm_conv("a0", "c1"));
        q.push_back(dm_conv("b0", "c2"));
        q.push_back(dm_conv("a1", "c1"));
        q.push_back(dm_conv("b1", "c2"));

        let (manager, outbound, calls, count, handle) =
            harness_with_limit("slack", q, policies, 1, Duration::from_secs(600)).await;

        let dispatched = wait_for_len(&calls, 4, Duration::from_secs(3)).await;
        assert_eq!(dispatched, 4, "all four turns run");
        assert_eq!(count.load(Ordering::SeqCst), 4);

        tokio::time::sleep(Duration::from_millis(200)).await;
        let out = outbound.lock().await;
        assert_eq!(out.len(), 2, "one allowed send per conversation");
        let mut convs: Vec<String> = out.iter().map(|m| m.conversation_id.clone()).collect();
        convs.sort();
        assert_eq!(convs, vec!["c1".to_string(), "c2".to_string()]);

        drop(out);
        manager.write().await.stop_all().await.unwrap();
        handle.abort();
    }

    /// A disabled guard (`window == 0`) never throttles: all autonomous replies
    /// for one conversation flow through regardless of count.
    #[tokio::test]
    async fn disabled_limit_lets_all_autonomous_replies_through() {
        let mut policies = HashMap::new();
        policies.insert("slack".to_string(), open_dm_policy());

        let mut q = VecDeque::new();
        for i in 0..4 {
            q.push_back(dm_conv(&format!("m{i}"), "c1"));
        }

        // Cap of 1 but a ZERO window disables the limiter entirely.
        let (manager, outbound, calls, _count, handle) =
            harness_with_limit("slack", q, policies, 1, Duration::ZERO).await;

        let _ = wait_for_len(&calls, 4, Duration::from_secs(3)).await;
        let replied = wait_for_len(&outbound, 4, Duration::from_secs(3)).await;
        assert_eq!(replied, 4, "disabled guard sends every reply");

        manager.write().await.stop_all().await.unwrap();
        handle.abort();
    }

    /// Human / operator sends go through `ChannelManager::send_to` directly and
    /// are NEVER gated by the auto-reply limiter — only the inbound-driven
    /// `run_turn` reply path consults it. This drives more direct sends than any
    /// auto-reply cap would allow and asserts all are delivered.
    #[tokio::test]
    async fn interactive_sends_bypass_the_rate_limit() {
        let (ch, outbound) = CapturingChannel::new("slack", VecDeque::new());
        let mgr = ChannelManager::new();
        let manager = Arc::new(RwLock::new(mgr));
        {
            let mut guard = manager.write().await;
            guard.register(Box::new(ch)).await;
            guard.start_all().await.unwrap();
        }

        // Ten operator-initiated sends to the same conversation — far above any
        // conservative auto-reply cap — all reach the channel.
        for i in 0..10 {
            let msg = OutgoingMessage::text("c1", format!("operator-{i}"));
            manager.read().await.send_to("slack", msg).await.unwrap();
        }

        assert_eq!(
            outbound.lock().await.len(),
            10,
            "direct operator sends are not rate-limited"
        );

        manager.write().await.stop_all().await.unwrap();
    }
}
