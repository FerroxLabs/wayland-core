//! `ChannelManager` — drives a registry of `Channel` impls.
//!
//! v0.7.0 2.A.2: each channel runs on its own tokio task that
//! polls `poll_events()` and forwards results to a single broadcast
//! channel the engine + UI subscribe to. Outbound sends go through
//! `send_to(name, msg)` which routes to the channel's send_message.
//!
//! Concurrency model: each channel is held in an `Arc<Mutex<Box<dyn
//! Channel>>>` so the poll task and the send path serialize against
//! the same instance (most platform SDKs aren't `Sync`-on-write).
//! Polling cadence is configurable; default 250ms.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;

use crate::Channel;
use crate::error::ChannelError;
use crate::event::{ChannelEvent, ConnectionState, MessageReceipt};
use crate::health::{ChannelHealth, HealthState};
use crate::outgoing::OutgoingMessage;
use crate::probe::ProbeReport;

/// Shared per-adapter health map. A `std::sync::Mutex` on purpose: every
/// critical section is a map write with no `await` inside it, so an async mutex
/// would buy nothing and would make the poll task's hot path yield.
type HealthMap = Arc<std::sync::Mutex<HashMap<String, ChannelHealth>>>;

/// Take the health lock, recovering from a poisoned mutex.
///
/// A panic in another thread must not turn the health surface into a permanent
/// error — a health surface that stops answering after an unrelated panic is
/// worse than one reporting stale data, because nothing reports that it stopped.
fn health_lock(map: &HealthMap) -> std::sync::MutexGuard<'_, HashMap<String, ChannelHealth>> {
    map.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Record an observed health transition for `name`.
///
/// `reason` is `None` only for [`HealthState::Healthy`]; the invariant is
/// asserted here rather than trusted, because every other caller of this
/// function is a code path that could forget.
fn record_health(
    map: &HealthMap,
    name: &str,
    state: HealthState,
    reason: Option<String>,
    consecutive_errors: u32,
    reconnect_delta: u32,
) {
    let mut guard = health_lock(map);
    let Some(entry) = guard.get_mut(name) else {
        return;
    };
    entry.state = state;
    entry.reason = if state.requires_reason() {
        Some(reason.unwrap_or_else(|| "no reason recorded".to_string()))
    } else {
        None
    };
    entry.consecutive_errors = consecutive_errors;
    entry.reconnects = entry.reconnects.saturating_add(reconnect_delta);
}

const DEFAULT_POLL_INTERVAL_MS: u64 = 250;
const EVENT_CHANNEL_CAP: usize = 256;

/// Consecutive non-`NotStarted` poll errors tolerated before the poll
/// task treats the channel as disconnected and enters supervised
/// reconnect. Below this, errors back off one tick and retry (the
/// historical behavior) to absorb transient blips without churn.
const RECONNECT_ERROR_THRESHOLD: u32 = 5;
/// First reconnect-attempt backoff. Doubles each failed `start()` up to
/// `RECONNECT_BACKOFF_CAP`.
const RECONNECT_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Upper bound on reconnect backoff so a permanently broken channel
/// retries at a steady, low rate rather than escalating unbounded.
const RECONNECT_BACKOFF_CAP: Duration = Duration::from_secs(30);

/// Driver for a set of `Channel` instances. Build with `new`, register
/// channels with `register`, then call `start_all` to spawn the poll
/// tasks. `subscribe()` returns a tokio broadcast receiver carrying
/// `ChannelEvent`s tagged with the originating channel name.
pub struct ChannelManager {
    channels: HashMap<String, Arc<Mutex<Box<dyn Channel>>>>,
    poll_tasks: HashMap<String, JoinHandle<()>>,
    poll_interval: Duration,
    events_tx: broadcast::Sender<TaggedEvent>,
    /// Observed per-adapter health, written by the poll tasks and read by the
    /// operator surfaces. Registered-but-unpolled adapters sit at
    /// [`HealthState::Unknown`], never `Healthy`.
    health: HealthMap,
}

/// Whether a [`ChannelManager::reload`] may begin polling what it registered.
///
/// F24-C3-H6b. Deliberately has **no `Default`** and is a required positional
/// argument: the right to poll a home belongs to whoever holds the single-owner
/// inbound polling lease, which is knowledge this crate does not have. A default
/// would be a guess made in the one place that cannot know the answer, and the
/// measured defect was exactly that guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartPolicy {
    /// Spawn poll tasks for adapters that do not already have one. The caller is
    /// asserting it holds the right to poll this home.
    StartNewlyRegistered,
    /// Register and replace adapters but spawn NO poll task. The adapter set is
    /// updated so outbound sends use current configuration, while inbound
    /// polling is left to whichever process owns it.
    LeaveStopped,
}

/// What a [`ChannelManager::reload`] did to the registered set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadReport {
    /// Newly configured adapters, registered and started.
    pub added: Vec<String>,
    /// Adapters whose configuration fingerprint changed — stopped, replaced by
    /// the new instance, and restarted.
    pub replaced: Vec<String>,
    /// Adapters no longer configured — stopped and removed.
    pub removed: Vec<String>,
    /// Adapters whose fingerprint matched. The RUNNING INSTANCE IS KEPT: not
    /// stopped, not restarted, not replaced. See [`ChannelManager::reload`].
    pub unchanged: Vec<String>,
}

/// One `ChannelEvent` annotated with the channel that produced it.
#[derive(Debug, Clone)]
pub struct TaggedEvent {
    pub channel_name: String,
    pub event: ChannelEvent,
}

impl ChannelManager {
    pub fn new() -> Self {
        let (events_tx, _) = broadcast::channel(EVENT_CHANNEL_CAP);
        Self {
            channels: HashMap::new(),
            poll_tasks: HashMap::new(),
            poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
            events_tx,
            health: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Override the polling interval. Default 250ms.
    pub fn with_poll_interval(mut self, dur: Duration) -> Self {
        self.poll_interval = dur;
        self
    }

    /// Register a channel. Replaces any existing channel under the
    /// same name (stops the old poll task first).
    pub async fn register(&mut self, ch: Box<dyn Channel>) {
        let name = ch.name().to_string();
        let platform = ch.platform().to_string();
        if let Some(handle) = self.poll_tasks.remove(&name) {
            handle.abort();
        }
        // Seed health at Unknown — registered, nothing observed. A newly
        // registered adapter that read `Healthy` would be claiming a liveness
        // nobody measured.
        health_lock(&self.health).insert(name.clone(), ChannelHealth::unknown(&name, &platform));
        self.channels.insert(name, Arc::new(Mutex::new(ch)));
    }

    /// Subscribe to the unified event stream. Late subscribers miss
    /// events emitted before they subscribed (broadcast semantics).
    pub fn subscribe(&self) -> broadcast::Receiver<TaggedEvent> {
        self.events_tx.subscribe()
    }

    /// Start every registered channel and spawn its poll task.
    /// Idempotent — channels already started skip re-start.
    pub async fn start_all(&mut self) -> Result<(), ChannelError> {
        for (name, slot) in self.channels.iter() {
            if self.poll_tasks.contains_key(name) {
                continue;
            }
            {
                let mut guard = slot.lock().await;
                if let Err(e) = guard.start().await {
                    // Don't abort the whole loop on one channel's failure
                    // (e.g. a missing credential) — the surviving channels
                    // would be left unstarted in hash order. Emit a
                    // Disconnected state for the failed channel and move on;
                    // the failure is surfaced, not silently swallowed.
                    tracing::warn!(
                        target: "wcore_channels::manager",
                        channel = %name,
                        error = %e,
                        "channel start() failed; skipping and continuing with the rest"
                    );
                    record_health(
                        &self.health,
                        name,
                        HealthState::Disconnected,
                        Some(format!("start() failed: {e}")),
                        0,
                        0,
                    );
                    let _ = self.events_tx.send(TaggedEvent {
                        channel_name: name.clone(),
                        event: ChannelEvent::ConnectionStateChanged {
                            state: ConnectionState::Disconnected,
                        },
                    });
                    continue;
                }
            }
            record_health(&self.health, name, HealthState::Healthy, None, 0, 0);
            let task_slot = Arc::clone(slot);
            let task_name = name.clone();
            let task_tx = self.events_tx.clone();
            let task_health = Arc::clone(&self.health);
            let interval = self.poll_interval;
            let handle = tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // Consecutive non-`NotStarted` poll errors. Reset to 0 on
                // any successful poll. Crossing `RECONNECT_ERROR_THRESHOLD`
                // promotes the channel to supervised reconnect.
                let mut consecutive_errors: u32 = 0;
                loop {
                    ticker.tick().await;
                    let evs = {
                        let mut guard = task_slot.lock().await;
                        // Detect a dead internal background task (longpoll/gateway/
                        // sync loop panicked or exited). The inbox-drain connectors
                        // return Ok(vec![]) forever once their task is gone, so
                        // without this check a silent task death looks alive. Read
                        // is_finished() into a bool here: task_handle() borrows
                        // &self while poll_events() needs &mut self, so the copy
                        // breaks the borrow.
                        let task_dead = guard.task_handle().is_some_and(|h| h.is_finished());
                        // DRAIN FIRST, judge the task dead second. A connector
                        // pushes its TERMINAL event — an auth rejection above all
                        // — immediately before its task exits, so a dead-task check
                        // that skips the drain strands the one event that explains
                        // WHY it exited and misreports a rejected credential as a
                        // generic transport fault. Only an EMPTY inbox on a dead
                        // task is a silent death; that is precisely the signal the
                        // check was written for, and it is preserved below.
                        let drained = guard.poll_events().await;
                        let poll_outcome = match drained {
                            Ok(ref evs) if evs.is_empty() && task_dead => {
                                tracing::warn!(
                                    target: "wcore_channels::manager",
                                    channel = %task_name,
                                    "connector internal task finished unexpectedly; forcing supervised reconnect"
                                );
                                Err(ChannelError::Transport(
                                    "connector internal task finished unexpectedly".into(),
                                ))
                            }
                            other => other,
                        };
                        match poll_outcome {
                            Ok(v) => {
                                if consecutive_errors > 0 {
                                    record_health(
                                        &task_health,
                                        &task_name,
                                        HealthState::Healthy,
                                        None,
                                        0,
                                        0,
                                    );
                                }
                                consecutive_errors = 0;
                                v
                            }
                            Err(ChannelError::NotStarted) => {
                                record_health(
                                    &task_health,
                                    &task_name,
                                    HealthState::Disconnected,
                                    Some("adapter reported not started; poll loop ended".into()),
                                    consecutive_errors,
                                    0,
                                );
                                break;
                            }
                            Err(e) => {
                                // A dead task jumps straight to the reconnect
                                // threshold; a normal poll error backs off one
                                // tick until it accumulates to the threshold.
                                if task_dead {
                                    consecutive_errors = RECONNECT_ERROR_THRESHOLD;
                                } else {
                                    consecutive_errors += 1;
                                    tracing::warn!(
                                        target: "wcore_channels::manager",
                                        channel = %task_name,
                                        error = %e,
                                        consecutive_errors,
                                        "poll_events errored; backing off one tick"
                                    );
                                }
                                record_health(
                                    &task_health,
                                    &task_name,
                                    HealthState::Degraded,
                                    Some(format!("poll_events failed: {e}")),
                                    consecutive_errors,
                                    0,
                                );
                                if consecutive_errors < RECONNECT_ERROR_THRESHOLD {
                                    continue;
                                }
                                // Drop the guard before the reconnect loop so we
                                // don't hold the slot lock across backoff sleeps
                                // (send_to / stop_all must still acquire it).
                                drop(guard);
                                // Supervised reconnect: announce Reconnecting and
                                // retry start() with exponential backoff until it
                                // succeeds. The task is stopped via handle.abort()
                                // (stop_all / register replace), so the sleeps
                                // below double as the abort points.
                                record_health(
                                    &task_health,
                                    &task_name,
                                    HealthState::Degraded,
                                    Some("supervised reconnect in progress".into()),
                                    consecutive_errors,
                                    0,
                                );
                                let _ = task_tx.send(TaggedEvent {
                                    channel_name: task_name.clone(),
                                    event: ChannelEvent::ConnectionStateChanged {
                                        state: ConnectionState::Reconnecting,
                                    },
                                });
                                let mut backoff = RECONNECT_BACKOFF_BASE;
                                loop {
                                    tokio::time::sleep(backoff).await;
                                    let start_result = {
                                        let mut guard = task_slot.lock().await;
                                        guard.start().await
                                    };
                                    match start_result {
                                        Ok(()) => {
                                            tracing::info!(
                                                target: "wcore_channels::manager",
                                                channel = %task_name,
                                                "channel reconnected; resuming polling"
                                            );
                                            consecutive_errors = 0;
                                            // The reconnect count is what
                                            // distinguishes a channel that is
                                            // healthy from one that is flapping
                                            // and happens to be up right now.
                                            record_health(
                                                &task_health,
                                                &task_name,
                                                HealthState::Healthy,
                                                None,
                                                0,
                                                1,
                                            );
                                            break;
                                        }
                                        Err(re) => {
                                            backoff = (backoff * 2).min(RECONNECT_BACKOFF_CAP);
                                            record_health(
                                                &task_health,
                                                &task_name,
                                                HealthState::Degraded,
                                                Some(format!("reconnect start() failed: {re}")),
                                                consecutive_errors,
                                                0,
                                            );
                                            tracing::warn!(
                                                target: "wcore_channels::manager",
                                                channel = %task_name,
                                                error = %re,
                                                next_backoff_ms = backoff.as_millis() as u64,
                                                "reconnect start() failed; will retry"
                                            );
                                        }
                                    }
                                }
                                // Reconnected — skip this tick's broadcast and
                                // resume the normal polling cadence.
                                continue;
                            }
                        }
                    };
                    // Set when this batch carried a credential rejection. An
                    // auth failure is TERMINAL: see the break below.
                    let mut auth_rejected = false;
                    for event in evs {
                        // The adapter's OWN published state outranks the poll
                        // loop's inference: a connector that knows its token was
                        // rejected is reporting a fact the poll loop can only
                        // guess at from a generic transport error.
                        match &event {
                            ChannelEvent::ConnectionStateChanged { state } => {
                                let mapped = HealthState::from_connection_state(*state);
                                if mapped == HealthState::Unauthenticated {
                                    auth_rejected = true;
                                }
                                record_health(
                                    &task_health,
                                    &task_name,
                                    mapped,
                                    Some(format!("adapter published {state:?}")),
                                    consecutive_errors,
                                    0,
                                );
                            }
                            ChannelEvent::AuthExpired { reason } => {
                                auth_rejected = true;
                                record_health(
                                    &task_health,
                                    &task_name,
                                    HealthState::Unauthenticated,
                                    Some(format!("adapter reported auth expired: {reason}")),
                                    consecutive_errors,
                                    0,
                                );
                            }
                            _ => {}
                        }
                        let _ = task_tx.send(TaggedEvent {
                            channel_name: task_name.clone(),
                            event,
                        });
                    }
                    // A rejected credential is terminal until an operator rotates
                    // it, which is the distinction `HealthState::Unauthenticated`
                    // exists to draw: "rotate a token", not "wait". Leaving the
                    // poll loop running would walk the channel straight back to a
                    // FALSE `Healthy` — the next tick's dead-task check forces
                    // supervised reconnect, and `start()` on these adapters only
                    // re-reads the credential out of the store and respawns, so it
                    // CANNOT fail on a token the platform is rejecting. It returns
                    // Ok, the reconnect arm records `Healthy`, and the surface is
                    // lying again within one tick. So stop here and leave the
                    // observation standing; `channel reload` and a gateway restart
                    // both re-register the adapter and clear it.
                    if auth_rejected {
                        tracing::error!(
                            target: "wcore_channels::manager",
                            channel = %task_name,
                            "channel credential was rejected by the platform; \
                             health is Unauthenticated and polling has stopped. \
                             Rotate the credential and run `channel reload`"
                        );
                        break;
                    }
                }
            });
            self.poll_tasks.insert(name.clone(), handle);
        }
        Ok(())
    }

    /// Stop every registered channel + abort its poll task.
    pub async fn stop_all(&mut self) -> Result<(), ChannelError> {
        let names: Vec<String> = self.channels.keys().cloned().collect();
        for name in names {
            if let Some(handle) = self.poll_tasks.remove(&name) {
                handle.abort();
            }
            if let Some(slot) = self.channels.get(&name) {
                let mut guard = slot.lock().await;
                let _ = guard.stop().await;
            }
            record_health(
                &self.health,
                &name,
                HealthState::Disconnected,
                Some("stopped by operator".into()),
                0,
                0,
            );
        }
        Ok(())
    }

    /// Per-adapter health as the manager has OBSERVED it, sorted by name.
    ///
    /// This reads the poll tasks' recorded observations. It does not ask the
    /// adapters how they are — an adapter reporting on its own liveness is the
    /// witness problem this phase measured at the delivery sink.
    pub fn health(&self) -> Vec<ChannelHealth> {
        let guard = health_lock(&self.health);
        let mut out: Vec<ChannelHealth> = guard.values().cloned().collect();
        out.sort_by(|a, b| a.channel.cmp(&b.channel));
        out
    }

    /// Health of one named adapter, or `None` if it is not registered.
    pub fn health_of(&self, name: &str) -> Option<ChannelHealth> {
        health_lock(&self.health).get(name).cloned()
    }

    /// Run the setup and authentication probe on one adapter.
    pub async fn probe_one(&self, name: &str) -> Result<ProbeReport, ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let guard = slot.lock().await;
        guard.probe().await
    }

    /// Probe every registered adapter, sorted by name.
    ///
    /// A probe that ERRORS is reported as [`crate::probe::ProbeOutcome::Unreachable`]
    /// rather than omitted: a channel missing from a probe listing is
    /// indistinguishable from one that was never configured.
    pub async fn probe_all(&self) -> Vec<ProbeReport> {
        let mut names = self.list_names();
        names.sort();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let Some(slot) = self.channels.get(&name) else {
                continue;
            };
            let guard = slot.lock().await;
            let platform = guard.platform().to_string();
            match guard.probe().await {
                Ok(report) => out.push(report),
                Err(e) => out.push(ProbeReport::unreachable(&name, &platform, e.to_string())),
            }
        }
        out
    }

    /// Edit an already-sent message through channel `name`. Unknown channel →
    /// `Config` error; platforms with no edit API →
    /// [`ChannelError::Unsupported`] via the trait default.
    pub async fn edit_on(
        &self,
        name: &str,
        conversation_id: &str,
        message_id: &str,
        new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let guard = slot.lock().await;
        guard
            .edit_message(conversation_id, message_id, new_text)
            .await
    }

    /// Delete an already-sent message through channel `name`. Mirrors
    /// [`Self::edit_on`].
    pub async fn delete_on(
        &self,
        name: &str,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<(), ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let guard = slot.lock().await;
        guard.delete_message(conversation_id, message_id).await
    }

    /// Take every registered adapter OUT of this manager, leaving it empty.
    ///
    /// Exists so a caller can build a DESIRED adapter set with the registry's
    /// existing loader — which registers into a `ChannelManager` and nothing
    /// else — and then hand that set to [`Self::reload`] on the live manager.
    /// The alternative was a second loader that produces a bare `Vec`, i.e. two
    /// code paths deciding which adapters exist, which is how the loaded set
    /// and the reloaded set drift apart.
    ///
    /// An adapter whose poll task is still running is SKIPPED rather than
    /// forcibly extracted: its `Arc` has a second owner, and tearing it out
    /// from under a live task is not something a staging helper should do.
    /// Callers use this on a freshly loaded, unstarted manager.
    pub async fn take_registered(&mut self) -> Vec<Box<dyn Channel>> {
        let names: Vec<String> = self.list_names();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            if let Some(handle) = self.poll_tasks.remove(&name) {
                handle.abort();
            }
            let Some(slot) = self.channels.remove(&name) else {
                continue;
            };
            match Arc::try_unwrap(slot) {
                Ok(mutex) => out.push(mutex.into_inner()),
                Err(shared) => {
                    tracing::warn!(
                        target: "wcore_channels::manager",
                        channel = %name,
                        "adapter is still shared with a running task; leaving it registered"
                    );
                    self.channels.insert(name, shared);
                }
            }
        }
        health_lock(&self.health).retain(|k, _| self.channels.contains_key(k));
        out
    }

    /// Apply a new configured adapter set without disturbing adapters whose
    /// configuration did not change.
    ///
    /// # The property that matters: an unchanged adapter keeps its INSTANCE
    ///
    /// The obvious implementation — clear the registry, register everything
    /// from the new set, start it all — is wrong in a way that is invisible
    /// from the outside. Adapters hold state that is not in their
    /// configuration: buffered inbound events not yet polled, an open socket, a
    /// platform session, outbound work handed to them but not yet acknowledged.
    /// Replacing an instance whose configuration did not change discards all of
    /// it, and the operator who edited ONE channel's token pays a reconnect and
    /// a dropped buffer on all ten. So an unchanged adapter is not stopped, not
    /// replaced, and not restarted; its running instance is kept.
    ///
    /// # Which direction "cannot tell" resolves, and why
    ///
    /// Sameness is decided by [`Channel::config_fingerprint`]. When EITHER side
    /// returns `None` the adapter is treated as CHANGED and replaced. The
    /// asymmetry is deliberate: treating unknown as unchanged means an operator
    /// rotates a credential, reloads, sees success, and keeps sending through
    /// the adapter holding the old one.
    ///
    /// # Why the caller must state a [`StartPolicy`]
    ///
    /// F24-C3-H6b. This used to end with an unconditional `let _ =
    /// self.start_all()`, which made "apply a new adapter set" and "begin
    /// polling" one indivisible act. They are not the same decision, because
    /// polling is gated by something this type knows nothing about: the
    /// single-owner inbound polling lease. The gateway gates its STARTUP
    /// `start_all` on owning that lease and then reached this method, which
    /// started the poll tasks anyway.
    ///
    /// Polling is a DESTRUCTIVE read — Telegram's `offset=` confirm deletes,
    /// IMAP sets `\Seen` — so a second poller does not cause a duplicate, it
    /// causes the rightful owner to see nothing at all. A reload silently
    /// re-acquiring that right is data loss, not a cosmetic defect.
    ///
    /// Measured on the shipped binary: a gateway that had correctly declined to
    /// poll (`state: Unknown, reason: "registered; no poll observed yet"`) was
    /// driven through one `channel reload` and came back `state: Disconnected,
    /// reason: "start() failed: …"` — proof that `start()` had been attempted on
    /// a process that did not hold the lease.
    ///
    /// So the decision is the caller's and there is no default. A caller that
    /// must not poll passes [`StartPolicy::LeaveStopped`] and cannot forget to,
    /// because the parameter has no default value to omit.
    pub async fn reload(
        &mut self,
        desired: Vec<Box<dyn Channel>>,
        start: StartPolicy,
    ) -> ReloadReport {
        let mut report = ReloadReport::default();

        let desired_names: std::collections::HashSet<String> =
            desired.iter().map(|c| c.name().to_string()).collect();

        // Remove adapters that are no longer configured.
        let registered: Vec<String> = self.list_names();
        for name in registered {
            if !desired_names.contains(&name) {
                if let Some(handle) = self.poll_tasks.remove(&name) {
                    handle.abort();
                }
                if let Some(slot) = self.channels.remove(&name) {
                    let mut guard = slot.lock().await;
                    let _ = guard.stop().await;
                }
                health_lock(&self.health).remove(&name);
                report.removed.push(name);
            }
        }

        for candidate in desired {
            let name = candidate.name().to_string();
            let existing_fp = match self.channels.get(&name) {
                Some(slot) => Some(slot.lock().await.config_fingerprint()),
                None => None,
            };
            match existing_fp {
                None => {
                    self.register(candidate).await;
                    report.added.push(name);
                }
                Some(current) => {
                    let incoming = candidate.config_fingerprint();
                    // `None` on either side means "cannot tell" — replace.
                    let same = match (current, incoming) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    };
                    if same {
                        report.unchanged.push(name);
                        // The candidate is dropped WITHOUT being started, and
                        // the running instance is left completely alone.
                    } else {
                        if let Some(handle) = self.poll_tasks.remove(&name) {
                            handle.abort();
                        }
                        if let Some(slot) = self.channels.get(&name) {
                            let mut guard = slot.lock().await;
                            let _ = guard.stop().await;
                        }
                        self.register(candidate).await;
                        report.replaced.push(name);
                    }
                }
            }
        }

        report.added.sort();
        report.replaced.sort();
        report.removed.sort();
        report.unchanged.sort();
        // Start anything newly registered, but ONLY if the caller holds
        // whatever right entitles this process to poll. `start_all` skips
        // adapters that already have a poll task, so an unchanged adapter is
        // untouched either way.
        if start == StartPolicy::StartNewlyRegistered {
            let _ = self.start_all().await;
        }
        report
    }

    /// Send a message through a named channel.
    pub async fn send_to(
        &self,
        name: &str,
        msg: OutgoingMessage,
    ) -> Result<MessageReceipt, ChannelError> {
        self.send_to_keyed(name, msg, None).await
    }

    /// Whether the named adapter transmits an idempotency key its destination
    /// will honour.
    ///
    /// The delivery spine reads this BEFORE it retries an outcome-unknown
    /// delivery. An unknown channel answers `false` rather than erroring: the
    /// question being asked is "is a retry safe here", and the safe answer for
    /// a destination that cannot even be resolved is no.
    ///
    /// # This answer is CAP-BLIND — prefer the per-message form
    ///
    /// This is a property of the ADAPTER, and the key only actually rides the
    /// wire when the body fits in one platform message: [`send_to_keyed`] drops
    /// it on the chunked path, for the reason documented there. So above
    /// [`Channel::max_message_len`] this method answers `true` about a send that
    /// will carry no key, and a caller that retries on the strength of it
    /// duplicates.
    ///
    /// The question a caller almost always means is "is a retry of THIS message
    /// safe", which is [`supports_outbound_idempotency_for`]. This form is
    /// retained for the callers that genuinely ask about the adapter (capability
    /// reporting, the `delivery-semantics` drift test) and for the case where no
    /// body is in hand yet.
    ///
    /// [`send_to_keyed`]: Self::send_to_keyed
    /// [`supports_outbound_idempotency_for`]: Self::supports_outbound_idempotency_for
    pub async fn supports_outbound_idempotency(&self, name: &str) -> bool {
        match self.channels.get(name) {
            Some(slot) => slot.lock().await.supports_outbound_idempotency(),
            None => false,
        }
    }

    /// Whether an idempotency key will actually ride **this body** to this
    /// destination — the truthful, per-message form of
    /// [`supports_outbound_idempotency`](Self::supports_outbound_idempotency).
    ///
    /// Answers `true` only when the adapter transmits a key the destination
    /// honours AND `text` fits in a single platform message, because those are
    /// exactly the two conditions under which
    /// [`send_to_keyed`](Self::send_to_keyed) puts a key on the wire. An
    /// over-cap body is N destination messages carrying no key at all, so a
    /// retry of it duplicates even on Matrix.
    ///
    /// The cap decision is read from [`Self::chunks_for`] — the same function
    /// the send itself uses — so this answer cannot drift away from the
    /// behaviour it describes. That sharing is the point: a parallel
    /// reimplementation here would be a second opinion about the send rather
    /// than a report of it.
    pub async fn supports_outbound_idempotency_for(&self, name: &str, text: &str) -> bool {
        match self.channels.get(name) {
            Some(slot) => {
                let guard = slot.lock().await;
                guard.supports_outbound_idempotency()
                    && Self::chunks_for(guard.max_message_len(), text).len() <= 1
            }
            None => false,
        }
    }

    /// The chunk split [`send_to_keyed`](Self::send_to_keyed) will perform for
    /// `text` under `max`.
    ///
    /// Factored out of the send so the send and
    /// [`supports_outbound_idempotency_for`](Self::supports_outbound_idempotency_for)
    /// share one decision. `None`, or a zero cap, means the connector declares
    /// no limit and the body goes as one message.
    fn chunks_for(max: Option<usize>, text: &str) -> Vec<String> {
        match max {
            Some(max) if max > 0 => crate::chunk::chunk_message(text, max),
            _ => vec![text.to_string()],
        }
    }

    /// [`send_to`](Self::send_to), optionally carrying the delivery ledger's
    /// idempotency key so the destination can recognise a replay.
    pub async fn send_to_keyed(
        &self,
        name: &str,
        msg: OutgoingMessage,
        key: Option<&str>,
    ) -> Result<MessageReceipt, ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let mut guard = slot.lock().await;

        // Split over-long bodies to the platform cap so a long reply is
        // delivered in pieces rather than rejected+dropped (HIGH-6). When the
        // connector declares no cap (or the body already fits) this is a
        // single send, byte-identical to the pre-chunking path.
        let chunks = Self::chunks_for(guard.max_message_len(), &msg.text);
        if chunks.len() <= 1 {
            return match key {
                // The key rides only the single-send path on purpose. A chunked
                // body is N messages at the destination under one logical
                // delivery, so one key cannot identify them; handing the same
                // key to every chunk would make a correct destination suppress
                // chunks 2..N as replays and silently truncate the message.
                //
                // That makes the guarantee CONDITIONAL on the body fitting, so
                // the question "may I retry this" is per-message, not
                // per-adapter: ask `supports_outbound_idempotency_for`, which
                // reads the same `chunks_for` decision this line does. The
                // cap-blind `supports_outbound_idempotency` answers about the
                // adapter and will say `true` about this send even when the
                // branch below is the one taken.
                Some(k) => guard.send_message_idempotent(msg, k).await,
                None => guard.send_message(msg).await,
            };
        }

        // Multi-chunk: each piece keeps the conversation, the DESTINATION
        // thread and the reply target; attachments ride the LAST chunk (so the
        // text precedes the media). Dropping `thread_id` here would send chunk
        // one into the topic and the rest into the channel root.
        // Returns the final chunk's receipt.
        let last = chunks.len() - 1;
        let mut receipt: Option<MessageReceipt> = None;
        for (i, chunk) in chunks.into_iter().enumerate() {
            let part = OutgoingMessage {
                conversation_id: msg.conversation_id.clone(),
                text: chunk,
                thread_id: msg.thread_id.clone(),
                reply_to: msg.reply_to.clone(),
                attachments: if i == last {
                    msg.attachments.clone()
                } else {
                    Vec::new()
                },
            };
            receipt = Some(guard.send_message(part).await?);
        }
        // INVARIANT: chunks.len() > 1 here, so the loop ran and set `receipt`.
        receipt.ok_or_else(|| ChannelError::Other("chunked send produced no receipt".into()))
    }

    /// Send a transient typing indicator to `conversation_id` on channel
    /// `name`. Best-effort: unknown channel → `Config` error; platforms
    /// without a typing API no-op via the trait default.
    pub async fn send_typing_to(
        &self,
        name: &str,
        conversation_id: &str,
    ) -> Result<(), ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let guard = slot.lock().await;
        guard.send_typing(conversation_id).await
    }

    /// React to `message_id` in `conversation_id` on channel `name` with a
    /// unicode emoji (ack state machine). Unknown channel → `Config` error;
    /// platforms without reactions → `Rejected` via the trait default.
    pub async fn react_on(
        &self,
        name: &str,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let guard = slot.lock().await;
        guard.react(conversation_id, message_id, emoji).await
    }

    /// Fetch an inbound attachment's bytes through the originating channel
    /// `name`, which holds its own credentials and platform media protocol.
    /// Mirrors [`Self::react_on`]. Unknown channel → `Config` error;
    /// connectors without media support → `Rejected` via the trait default.
    ///
    /// NOTE (concurrency): like `react_on`/`send_typing_to`, this holds the
    /// channel's mutex across the download, briefly pausing that one channel's
    /// poll/send while its own just-received media is fetched. The enricher
    /// bounds the call with a timeout so a slow media host can't stall it.
    ///
    /// # This is where a declared bound stops being decorative
    ///
    /// The payload is checked against the originating channel's
    /// [`Channel::media_bounds`](crate::Channel::media_bounds) before it is
    /// handed back. This is the ONLY production path to adapter media
    /// (`ChannelMediaEnricher` reaches every attachment through here), so it is
    /// the one site that can make every adapter's declaration load-bearing —
    /// including the adapters that carry no size check of their own and would
    /// otherwise be bounded by nothing but the trait default they never read.
    ///
    /// An adapter's own fetch path is still expected to cap the *streamed* read
    /// at the same number, so a hostile payload is refused before it is
    /// buffered rather than after. That per-adapter cap and this one are read
    /// from a single constant per crate precisely so they cannot drift apart —
    /// which is exactly what they had done: every adapter that enforced a cap
    /// enforced a different number from the one it advertised.
    pub async fn fetch_media_on(
        &self,
        name: &str,
        attachment: &crate::event::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let slot = self
            .channels
            .get(name)
            .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
        let guard = slot.lock().await;
        let bounds = guard.media_bounds();
        let bytes = guard.fetch_media(attachment).await?;
        let len = bytes.len() as u64;
        if len > bounds.max_bytes {
            return Err(ChannelError::Rejected(format!(
                "attachment is {len} bytes, over channel {name}'s declared \
                 {} byte media bound",
                bounds.max_bytes
            )));
        }
        Ok(bytes)
    }

    /// The bounds channel `name` declares via
    /// [`Channel::media_bounds`](crate::Channel::media_bounds), or `None` if no
    /// such channel is registered.
    ///
    /// Exposed so the inbound media enricher can apply `max_attachments`, which
    /// [`Self::fetch_media_on`] structurally cannot: that method sees one
    /// attachment at a time and a per-message count bound needs the whole list.
    pub async fn media_bounds_on(&self, name: &str) -> Option<crate::MediaBounds> {
        let slot = self.channels.get(name)?;
        let guard = slot.lock().await;
        Some(guard.media_bounds())
    }

    /// The native-action surface channel `name` declares via
    /// [`Channel::native_actions`](crate::Channel::native_actions), or `None`
    /// if no such channel is registered.
    ///
    /// Read this BEFORE calling [`Self::edit_on`] / [`Self::delete_on`] when the
    /// answer matters and the call does not: a delete is a request a caller may
    /// not want to issue speculatively just to discover the platform has no
    /// delete endpoint.
    pub async fn native_actions_on(&self, name: &str) -> Option<crate::NativeActions> {
        let slot = self.channels.get(name)?;
        let guard = slot.lock().await;
        Some(guard.native_actions())
    }

    /// Every registered adapter's `(name, platform, declared actions)`, sorted
    /// by name — the machine-readable native-action matrix.
    ///
    /// Sorted so a rendered matrix is diffable between runs; an unsorted matrix
    /// whose rows shuffle looks changed when it is not.
    pub async fn native_action_matrix(&self) -> Vec<(String, String, crate::NativeActions)> {
        let mut out = Vec::with_capacity(self.channels.len());
        for name in self.list_names() {
            let Some(slot) = self.channels.get(&name) else {
                continue;
            };
            let guard = slot.lock().await;
            out.push((
                name.clone(),
                guard.platform().to_string(),
                guard.native_actions(),
            ));
        }
        out
    }

    /// Names of every registered channel whose [`Channel::platform`] equals
    /// `platform`, sorted alphabetically.
    ///
    /// A channel's INSTANCE NAME and its PLATFORM are independent: an operator
    /// may call an email channel "mail", "work", or anything else, and the
    /// name is what [`Self::send_to`] keys on. Callers that hold a platform
    /// token (`send_message`'s `email:...` target, say) and need a channel
    /// name must ask the adapters what platform they actually are — guessing
    /// from the name is what made every non-default-named channel
    /// unaddressable.
    pub async fn names_for_platform(&self, platform: &str) -> Vec<String> {
        let mut out = Vec::new();
        for name in self.list_names() {
            let Some(slot) = self.channels.get(&name) else {
                continue;
            };
            let guard = slot.lock().await;
            if guard.platform() == platform {
                out.push(name);
            }
        }
        out
    }

    /// List names of registered channels, sorted alphabetically.
    pub fn list_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.channels.keys().cloned().collect();
        names.sort();
        names
    }

    /// Route an inbound webhook request to channel `name`'s
    /// [`Channel::ingest_webhook`](crate::Channel::ingest_webhook). The
    /// connector verifies the platform signature, parses the body, and
    /// enqueues any resulting event(s) for the next `poll_events()` (which
    /// the inbound subscriber drains). The returned
    /// [`WebhookResponse`](crate::webhook::WebhookResponse) is what the host
    /// writes back to the platform. Unknown channel → `Config` error (the
    /// host maps it to a 404). Mirrors [`Self::send_to`] for inbound.
    ///
    /// Concurrency (rank 73): the `self.channels` map is only borrowed long
    /// enough to *clone the slot handle* (`Arc::clone`); that borrow is
    /// released before the async signature-verify + parse runs, so a webhook
    /// ingest never pins the `ChannelManager`'s own borrow across the await.
    /// The per-slot `Mutex` is still held across the connector's
    /// `ingest_webhook` because the channel is owned inside that mutex (the
    /// `&mut`-taking lifecycle methods `start`/`stop`/`poll_events`/
    /// `send_message` require exclusive access to the same instance, so the
    /// instance cannot also be exposed as a lock-free shared `Arc<dyn Channel>`
    /// without interior mutability). Fully de-serializing concurrent
    /// same-channel deliveries (e.g. parallel Slack event batches) would
    /// require migrating the `Channel` lifecycle methods to `&self` +
    /// interior mutability — a cross-crate trait change touching every
    /// connector — and is intentionally NOT done here.
    pub async fn ingest_webhook(
        &self,
        name: &str,
        req: &crate::webhook::WebhookRequest,
    ) -> Result<crate::webhook::WebhookResponse, ChannelError> {
        // Clone the slot handle out of the map, then drop the map borrow so the
        // async ingest below holds neither `&self.channels` nor `&self`.
        let slot = {
            let slot = self
                .channels
                .get(name)
                .ok_or_else(|| ChannelError::Config(format!("unknown channel: {name}")))?;
            Arc::clone(slot)
        };
        let guard = slot.lock().await;
        guard.ingest_webhook(req).await
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::IncomingMessage;
    use crate::mock::MockChannel;
    use async_trait::async_trait;
    use std::time::Duration;

    /// Test-only channel whose `poll_events` errors until the manager
    /// re-`start()`s it (the reconnect primitive), after which it recovers
    /// and delivers a single injected message. Models a channel whose
    /// polling breaks until supervised reconnect heals it.
    struct FlakyChannel {
        name: String,
        /// True once the channel has been started at least once.
        started_once: bool,
        /// True after a second `start()` (the manager's reconnect).
        recovered: bool,
        /// True once the recovery message has been delivered.
        delivered: bool,
    }

    impl FlakyChannel {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                started_once: false,
                recovered: false,
                delivered: false,
            }
        }
    }

    #[async_trait]
    impl Channel for FlakyChannel {
        fn name(&self) -> &str {
            &self.name
        }

        fn platform(&self) -> &str {
            "flaky"
        }

        async fn start(&mut self) -> Result<(), ChannelError> {
            // First start() = initial connect. Any later start() is the
            // manager's reconnect attempt, which heals the channel.
            if self.started_once {
                self.recovered = true;
            }
            self.started_once = true;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }

        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            if self.recovered {
                if !self.delivered {
                    self.delivered = true;
                    return Ok(vec![ChannelEvent::MessageReceived {
                        msg: IncomingMessage::new("flaky-1", "c1", "alice", "back online", 0),
                    }]);
                }
                return Ok(Vec::new());
            }
            // Still in the failing window: error until reconnect heals us.
            Err(ChannelError::Transport("simulated poll failure".into()))
        }

        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            Ok(MessageReceipt {
                id: "flaky-out".into(),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }

        fn config_schema(&self) -> &str {
            r#"{"name": "string", "platform": "flaky"}"#
        }
    }

    /// Test-only channel with a small `max_message_len` that records every
    /// `send_message` into a shared log, so a test can assert how `send_to`
    /// chunked an over-long body.
    struct CappedChannel {
        name: String,
        cap: usize,
        sent: std::sync::Arc<tokio::sync::Mutex<Vec<OutgoingMessage>>>,
    }

    impl CappedChannel {
        fn new(
            name: &str,
            cap: usize,
        ) -> (
            Self,
            std::sync::Arc<tokio::sync::Mutex<Vec<OutgoingMessage>>>,
        ) {
            let sent = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
            (
                Self {
                    name: name.into(),
                    cap,
                    sent: std::sync::Arc::clone(&sent),
                },
                sent,
            )
        }
    }

    #[async_trait]
    impl Channel for CappedChannel {
        fn name(&self) -> &str {
            &self.name
        }
        fn platform(&self) -> &str {
            "capped"
        }
        async fn start(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            Ok(Vec::new())
        }
        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            let idx = {
                let mut log = self.sent.lock().await;
                log.push(msg.clone());
                log.len() - 1
            };
            Ok(MessageReceipt {
                id: format!("capped-out-{idx}"),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }
        fn config_schema(&self) -> &str {
            r#"{"name":"string","platform":"capped"}"#
        }
        fn max_message_len(&self) -> Option<usize> {
            Some(self.cap)
        }
    }

    /// A capped adapter that DOES transmit an idempotency key, and records the
    /// key that rode each send (`None` for the unkeyed path).
    ///
    /// This is the Matrix shape: `supports_outbound_idempotency() == true` plus
    /// a finite `max_message_len`. Without both, the conditional guarantee of
    /// `docs/delivery-semantics.md` §4.1 cannot be exercised at all.
    struct KeyedCappedChannel {
        name: String,
        cap: usize,
        keys: std::sync::Arc<tokio::sync::Mutex<Vec<Option<String>>>>,
    }

    impl KeyedCappedChannel {
        #[allow(clippy::type_complexity)]
        fn new(
            name: &str,
            cap: usize,
        ) -> (
            Self,
            std::sync::Arc<tokio::sync::Mutex<Vec<Option<String>>>>,
        ) {
            let keys = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
            (
                Self {
                    name: name.into(),
                    cap,
                    keys: std::sync::Arc::clone(&keys),
                },
                keys,
            )
        }
    }

    #[async_trait]
    impl Channel for KeyedCappedChannel {
        fn name(&self) -> &str {
            &self.name
        }
        fn platform(&self) -> &str {
            "keyed-capped"
        }
        async fn start(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            Ok(Vec::new())
        }
        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            let idx = {
                let mut log = self.keys.lock().await;
                log.push(None);
                log.len() - 1
            };
            Ok(MessageReceipt {
                id: format!("keyed-out-{idx}"),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }
        async fn send_message_idempotent(
            &mut self,
            msg: OutgoingMessage,
            key: &str,
        ) -> Result<MessageReceipt, ChannelError> {
            let idx = {
                let mut log = self.keys.lock().await;
                log.push(Some(key.to_string()));
                log.len() - 1
            };
            Ok(MessageReceipt {
                id: format!("keyed-out-{idx}"),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }
        fn supports_outbound_idempotency(&self) -> bool {
            true
        }
        fn config_schema(&self) -> &str {
            r#"{"name":"string","platform":"keyed-capped"}"#
        }
        fn max_message_len(&self) -> Option<usize> {
            Some(self.cap)
        }
    }

    /// Under the cap: the per-message answer is `true` AND the key really rode.
    ///
    /// Asserting the answer alone would be a claim about a bool. The point is
    /// that the bool describes the wire, so the wire is read back.
    #[tokio::test]
    async fn under_the_cap_the_key_rides_and_the_per_message_answer_says_so() {
        let (ch, keys) = KeyedCappedChannel::new("kc", 10);
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(ch)).await;

        let body = "short";
        assert!(
            mgr.supports_outbound_idempotency_for("kc", body).await,
            "a body inside the cap is a single keyed send"
        );

        mgr.send_to_keyed("kc", OutgoingMessage::text("c1", body), Some("delivery-1"))
            .await
            .expect("send");

        let log = keys.lock().await;
        assert_eq!(log.len(), 1, "one message");
        assert_eq!(
            log[0].as_deref(),
            Some("delivery-1"),
            "the key must actually be on the wire"
        );
    }

    /// Over the cap: no key rides, and the per-message answer says `false`
    /// while the cap-blind answer still says `true`.
    ///
    /// This is the whole defect in one test. The per-adapter bit is what the
    /// delivery spine used to consult before deciding a retry was safe, and it
    /// is `true` here for a send that carried no key at all — so a retry would
    /// have produced a second full copy.
    #[tokio::test]
    async fn over_the_cap_no_key_rides_and_only_the_per_message_answer_notices() {
        let (ch, keys) = KeyedCappedChannel::new("kc", 10);
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(ch)).await;

        // 25 chars at cap 10 → three chunks.
        let body = "abcdefghijklmnopqrstuvwxy";

        assert!(
            mgr.supports_outbound_idempotency("kc").await,
            "known-positive: the cap-blind form answers true, which is the whole problem. If \
             this were false the test below would pass for the wrong reason."
        );
        assert!(
            !mgr.supports_outbound_idempotency_for("kc", body).await,
            "an over-cap body is sent unkeyed, so a retry of it is NOT safe"
        );

        mgr.send_to_keyed("kc", OutgoingMessage::text("c1", body), Some("delivery-1"))
            .await
            .expect("send");

        let log = keys.lock().await;
        assert_eq!(log.len(), 3, "25 chars at cap 10 → 3 sends");
        assert!(
            log.iter().all(|k| k.is_none()),
            "NO chunk may carry the key — one key cannot identify N destination messages, and \
             reusing it would make a correct destination suppress chunks 2..N: {log:?}"
        );
    }

    /// An adapter that cannot deduplicate at all answers `false` at every
    /// length, so the per-message form never over-promises for it.
    #[tokio::test]
    async fn a_non_idempotent_adapter_is_false_under_and_over_the_cap() {
        let (ch, _sent) = CappedChannel::new("capped", 10);
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(ch)).await;

        assert!(!mgr.supports_outbound_idempotency_for("capped", "hi").await);
        assert!(
            !mgr.supports_outbound_idempotency_for("capped", "abcdefghijklmnopqrstuvwxy")
                .await
        );
    }

    /// An uncapped adapter keeps the unconditional answer at any length — the
    /// cap check must not turn `None` into a false negative.
    #[tokio::test]
    async fn an_uncapped_idempotent_adapter_is_true_at_any_length() {
        let (ch, _keys) = KeyedCappedChannel::new("kc", 0);
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(ch)).await;

        // cap 0 is the "no cap declared / disabled" sentinel `chunks_for` honours.
        let long = "x".repeat(100_000);
        assert!(mgr.supports_outbound_idempotency_for("kc", &long).await);
    }

    /// Unknown channel: the safe answer, matching the cap-blind form.
    #[tokio::test]
    async fn an_unknown_channel_is_not_replay_safe() {
        let mgr = ChannelManager::new();
        assert!(!mgr.supports_outbound_idempotency_for("nope", "hi").await);
    }

    #[tokio::test]
    async fn send_to_chunks_overlong_body_to_the_cap() {
        let (ch, sent) = CappedChannel::new("capped", 10);
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(ch)).await;

        // 25 chars, no break points → hard-split into 10/10/5.
        let body = "abcdefghijklmnopqrstuvwxy".to_string();
        let receipt = mgr
            .send_to(
                "capped",
                OutgoingMessage {
                    conversation_id: "c1".into(),
                    text: body.clone(),
                    thread_id: None,
                    reply_to: Some("t1".into()),
                    attachments: vec!["file://a".into()],
                },
            )
            .await
            .expect("send_to");

        let log = sent.lock().await;
        assert_eq!(log.len(), 3, "25 chars at cap 10 → 3 sends");
        assert!(
            log.iter().all(|m| m.text.chars().count() <= 10),
            "every chunk within the cap"
        );
        assert_eq!(
            log.iter().map(|m| m.text.clone()).collect::<String>(),
            body,
            "lossless reassembly across chunks"
        );
        // reply_to carried on every chunk; attachments only on the last.
        assert!(log.iter().all(|m| m.reply_to.as_deref() == Some("t1")));
        assert!(log[0].attachments.is_empty());
        assert!(log[1].attachments.is_empty());
        assert_eq!(log[2].attachments, vec!["file://a".to_string()]);
        // Receipt is the final chunk's.
        assert_eq!(receipt.id, "capped-out-2");
    }

    #[tokio::test]
    async fn send_to_does_not_chunk_when_within_cap() {
        let (ch, sent) = CappedChannel::new("capped", 100);
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(ch)).await;
        mgr.send_to(
            "capped",
            OutgoingMessage {
                conversation_id: "c1".into(),
                text: "short".into(),
                thread_id: None,
                reply_to: None,
                attachments: Vec::new(),
            },
        )
        .await
        .expect("send_to");
        assert_eq!(sent.lock().await.len(), 1, "a fitting body is one send");
    }

    #[tokio::test]
    async fn register_and_list() {
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.register(Box::new(MockChannel::new("beta"))).await;
        assert_eq!(
            mgr.list_names(),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[tokio::test]
    async fn start_all_emits_connection_state_changes() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(20));
        let mut rx = mgr.subscribe();
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.start_all().await.unwrap();

        // Each MockChannel emits a Connected event on start().
        let tagged = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("event arrived")
            .expect("ok");
        assert_eq!(tagged.channel_name, "alpha");
        assert!(matches!(
            tagged.event,
            ChannelEvent::ConnectionStateChanged { .. }
        ));
        mgr.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn send_to_unknown_channel_errors() {
        let mgr = ChannelManager::new();
        let err = mgr
            .send_to("missing", OutgoingMessage::text("c1", "x"))
            .await
            .expect_err("expected unknown-channel error");
        assert!(matches!(err, ChannelError::Config(_)));
    }

    #[tokio::test]
    async fn send_to_registered_channel_routes() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(20));
        mgr.register(Box::new(MockChannel::new("alpha"))).await;
        mgr.start_all().await.unwrap();
        // Drain initial state-change event.
        let rx = mgr.subscribe();

        let receipt = mgr
            .send_to("alpha", OutgoingMessage::text("c1", "hello"))
            .await
            .unwrap();
        assert!(!receipt.id.is_empty());
        let _ = rx; // suppress unused
        mgr.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn persistent_poll_failure_triggers_supervised_reconnect() {
        // Fail enough polls to cross the threshold, then recover on the
        // manager's reconnect start(). Assert a Reconnecting state is
        // broadcast and the channel resumes delivering messages.
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(5));
        let mut rx = mgr.subscribe();
        mgr.register(Box::new(FlakyChannel::new("flaky"))).await;
        mgr.start_all().await.unwrap();

        // Reconnect backoff base is 1s; allow margin for ticks + delivery.
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        let mut saw_reconnecting = false;
        let mut saw_recovery_msg = false;
        while std::time::Instant::now() < deadline && !(saw_reconnecting && saw_recovery_msg) {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(tagged)) => {
                    assert_eq!(tagged.channel_name, "flaky");
                    match tagged.event {
                        ChannelEvent::ConnectionStateChanged {
                            state: ConnectionState::Reconnecting,
                        } => saw_reconnecting = true,
                        ChannelEvent::MessageReceived { ref msg } if msg.text == "back online" => {
                            saw_recovery_msg = true;
                        }
                        _ => {}
                    }
                }
                _ => continue,
            }
        }
        assert!(
            saw_reconnecting,
            "expected a Reconnecting ConnectionStateChanged broadcast"
        );
        assert!(
            saw_recovery_msg,
            "expected the channel to resume delivering messages after reconnect"
        );
        mgr.stop_all().await.unwrap();
    }

    /// Test-only channel modelling an inbox-drain connector (Telegram/Discord/
    /// Email/iMessage/Matrix style) whose internal background task dies silently.
    /// `poll_events` always returns `Ok(vec![])` — so error-count supervision
    /// alone would never fire. The first `start()` spawns a task that exits
    /// immediately, so `task_handle().is_finished()` trips the manager's
    /// dead-task detection and forces supervised reconnect. The reconnect
    /// `start()` spawns a long-lived task (so it does NOT re-trip) and queues a
    /// recovery message, proving the channel heals.
    struct DeadTaskChannel {
        name: String,
        started_once: bool,
        recovered: bool,
        delivered: bool,
        inbox: std::collections::VecDeque<ChannelEvent>,
        handle: Option<JoinHandle<()>>,
    }

    impl DeadTaskChannel {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                started_once: false,
                recovered: false,
                delivered: false,
                inbox: std::collections::VecDeque::new(),
                handle: None,
            }
        }
    }

    #[async_trait]
    impl Channel for DeadTaskChannel {
        fn name(&self) -> &str {
            &self.name
        }

        fn platform(&self) -> &str {
            "deadtask"
        }

        fn task_handle(&self) -> Option<&JoinHandle<()>> {
            self.handle.as_ref()
        }

        async fn start(&mut self) -> Result<(), ChannelError> {
            if self.started_once {
                // The manager's reconnect: heal and spawn a long-lived task so
                // we don't immediately look dead again.
                self.recovered = true;
                self.handle = Some(tokio::spawn(async {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }));
            } else {
                // Initial connect: spawn a task that exits immediately, modelling
                // a background loop that died right after start().
                self.handle = Some(tokio::spawn(async {}));
            }
            self.started_once = true;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), ChannelError> {
            if let Some(h) = self.handle.take() {
                h.abort();
            }
            Ok(())
        }

        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            if self.recovered && !self.delivered {
                self.delivered = true;
                self.inbox.push_back(ChannelEvent::MessageReceived {
                    msg: IncomingMessage::new("dead-1", "c1", "alice", "back online", 0),
                });
            }
            // Always an Ok drain — the silent-death signature. With an empty
            // inbox this is the perpetual `Ok(vec![])` that hides a dead task.
            Ok(self.inbox.drain(..).collect())
        }

        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            Ok(MessageReceipt {
                id: "dead-out".into(),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }

        fn config_schema(&self) -> &str {
            r#"{"name":"string","platform":"deadtask"}"#
        }
    }

    #[tokio::test]
    async fn dead_internal_task_triggers_supervised_reconnect() {
        // The connector's `poll_events` returns Ok(vec![]) forever, but its
        // internal task finishes right after start(). The manager must detect the
        // finished task_handle and drive supervised reconnect even though no poll
        // ever errored. Mirrors `persistent_poll_failure_triggers_supervised_reconnect`.
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(5));
        let mut rx = mgr.subscribe();
        mgr.register(Box::new(DeadTaskChannel::new("dead"))).await;
        mgr.start_all().await.unwrap();

        // Reconnect backoff base is 1s; allow margin for the spawned task to
        // finish, the dead-task tick, the backoff, and recovery delivery.
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        let mut saw_reconnecting = false;
        let mut saw_recovery_msg = false;
        while std::time::Instant::now() < deadline && !(saw_reconnecting && saw_recovery_msg) {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(tagged)) => {
                    assert_eq!(tagged.channel_name, "dead");
                    match tagged.event {
                        ChannelEvent::ConnectionStateChanged {
                            state: ConnectionState::Reconnecting,
                        } => saw_reconnecting = true,
                        ChannelEvent::MessageReceived { ref msg } if msg.text == "back online" => {
                            saw_recovery_msg = true;
                        }
                        _ => {}
                    }
                }
                _ => continue,
            }
        }
        assert!(
            saw_reconnecting,
            "expected a Reconnecting broadcast from the dead-task detection"
        );
        assert!(
            saw_recovery_msg,
            "expected the channel to resume delivering messages after reconnect"
        );
        mgr.stop_all().await.unwrap();
    }

    /// Test-only channel whose `start()` always fails — models a channel
    /// with a missing credential. Used to prove `start_all` doesn't abort
    /// the whole loop on one channel's failure.
    struct FailingStartChannel {
        name: String,
    }

    #[async_trait]
    impl Channel for FailingStartChannel {
        fn name(&self) -> &str {
            &self.name
        }
        fn platform(&self) -> &str {
            "failing"
        }
        async fn start(&mut self) -> Result<(), ChannelError> {
            Err(ChannelError::Auth("missing credential".into()))
        }
        async fn stop(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            Ok(Vec::new())
        }
        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            Ok(MessageReceipt {
                id: "failing-out".into(),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }
        fn config_schema(&self) -> &str {
            r#"{"name":"string","platform":"failing"}"#
        }
    }

    #[tokio::test]
    async fn start_all_continues_past_a_failing_channel() {
        // One channel whose start() fails (missing credential) + one OK
        // channel. start_all must start the OK one, spawn its poll task, and
        // record the failure via a Disconnected event — not abort the loop.
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(15));
        let mut rx = mgr.subscribe();
        mgr.register(Box::new(FailingStartChannel {
            name: "broken".into(),
        }))
        .await;
        let mut ok = MockChannel::new("good");
        ok.inject_text("c1", "alice", "hi");
        mgr.register(Box::new(ok)).await;

        // Aggregate result is Ok even though one channel failed to start.
        mgr.start_all().await.unwrap();

        // The OK channel got a poll task; the failing one did not.
        assert!(
            mgr.poll_tasks.contains_key("good"),
            "the healthy channel must be started and supervised"
        );
        assert!(
            !mgr.poll_tasks.contains_key("broken"),
            "the failing channel must NOT have a poll task"
        );

        // Drain events: we must see a Disconnected for "broken" (the recorded
        // failure) and the OK channel's injected message (it really started).
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut saw_broken_disconnected = false;
        let mut saw_good_message = false;
        while std::time::Instant::now() < deadline && !(saw_broken_disconnected && saw_good_message)
        {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(tagged)) => match tagged.event {
                    ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Disconnected,
                    } if tagged.channel_name == "broken" => saw_broken_disconnected = true,
                    ChannelEvent::MessageReceived { .. } if tagged.channel_name == "good" => {
                        saw_good_message = true;
                    }
                    _ => {}
                },
                _ => continue,
            }
        }
        assert!(
            saw_broken_disconnected,
            "expected a Disconnected event recording the failed channel"
        );
        assert!(
            saw_good_message,
            "expected the healthy channel to actually poll + deliver"
        );
        mgr.stop_all().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Credential rejection — giving `HealthState::Unauthenticated` a producer.
    //
    // Measured defect: the gateway reported a Matrix channel `Healthy` while
    // the homeserver 401'd every `/sync`, through 21 consecutive failures and a
    // `delivered:false` send. `Unauthenticated` existed and nothing could
    // produce it on any of the three MVP channels.
    //
    // The four quadrants are asserted here at the health-projection layer, and
    // again over real HTTP in `wcore-channel-matrix`'s `sync.rs`.
    // -----------------------------------------------------------------------

    /// Models an adapter whose background task discovers a rejected credential:
    /// it pushes the terminal event into its inbox and its task exits
    /// immediately, which is exactly what `wcore-channel-matrix`'s `/sync` loop
    /// and `wcore-channel-telegram`'s long-poll both do.
    ///
    /// `starts` counts `start()` calls, so a test can prove the manager did NOT
    /// walk the channel back through supervised reconnect.
    struct AuthRejectingChannel {
        name: String,
        handle: Option<JoinHandle<()>>,
        inbox: std::collections::VecDeque<ChannelEvent>,
        starts: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AuthRejectingChannel {
        fn new(name: &str) -> (Self, Arc<std::sync::atomic::AtomicUsize>) {
            let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (
                Self {
                    name: name.into(),
                    handle: None,
                    inbox: std::collections::VecDeque::new(),
                    starts: Arc::clone(&starts),
                },
                starts,
            )
        }
    }

    #[async_trait]
    impl Channel for AuthRejectingChannel {
        fn name(&self) -> &str {
            &self.name
        }
        fn platform(&self) -> &str {
            "authreject"
        }
        fn task_handle(&self) -> Option<&JoinHandle<()>> {
            self.handle.as_ref()
        }
        async fn start(&mut self) -> Result<(), ChannelError> {
            // Note this returns Ok: `start()` on the real adapters only re-reads
            // the credential out of the store and respawns, so it CANNOT fail on
            // a token the platform rejects. That is precisely why supervised
            // reconnect would record a false `Healthy` here.
            self.starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inbox.push_back(ChannelEvent::AuthExpired {
                reason: "homeserver rejected the access token: HTTP 401 M_UNKNOWN_TOKEN".into(),
            });
            // The background task pushes the event and then dies.
            self.handle = Some(tokio::spawn(async {}));
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), ChannelError> {
            if let Some(h) = self.handle.take() {
                h.abort();
            }
            Ok(())
        }
        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            Ok(self.inbox.drain(..).collect())
        }
        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            Ok(MessageReceipt {
                id: "auth-out".into(),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }
        fn config_schema(&self) -> &str {
            r#"{"name":"string","platform":"authreject"}"#
        }
    }

    /// **QUADRANT 1 — the platform rejects a present credential.**
    ///
    /// Health must reach `Unauthenticated`, carry an actionable reason, and
    /// STAY there: an auth failure is terminal until an operator rotates the
    /// token.
    ///
    /// This reddens on the unfixed code twice over. (a) The adapter's task is
    /// already finished when the first tick runs, and the old `task_dead` check
    /// returned `Err(Transport)` *instead of* draining — so the `AuthExpired`
    /// was stranded in the inbox and never seen at all. (b) Even once seen, the
    /// old loop continued, and the next tick's dead-task detection drove
    /// supervised reconnect, whose `start()` returns `Ok` and records `Healthy`.
    #[tokio::test]
    async fn a_rejected_credential_reports_unauthenticated_and_stays_there() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
        let (ch, starts) = AuthRejectingChannel::new("rejected");
        mgr.register(Box::new(ch)).await;
        mgr.start_all().await.unwrap();

        // Bounded wait for the state to appear.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            let h = mgr.health_of("rejected").expect("channel is registered");
            if h.state == HealthState::Unauthenticated {
                got = Some(h);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let h = got.expect(
            "health never reached Unauthenticated — the adapter's AuthExpired was \
             either stranded by the dead-task check or overwritten by reconnect",
        );
        assert!(
            h.reason
                .as_deref()
                .is_some_and(|r| r.contains("M_UNKNOWN_TOKEN")),
            "the reason must name the platform's rejection so an operator can \
             act on it: {:?}",
            h.reason
        );

        // STICKINESS. Reconnect backoff base is 1s, so waiting past it is what
        // makes this assertion mean something: on the unfixed loop the channel
        // is back to `Healthy` by now.
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let after = mgr.health_of("rejected").expect("still registered");
        assert_eq!(
            after.state,
            HealthState::Unauthenticated,
            "a rejected credential must not drift back to {:?} — reconnecting \
             cannot fix a token the platform refuses",
            after.state
        );
        assert_eq!(
            starts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "supervised reconnect must NOT re-start a channel whose credential \
             was rejected; that is the loop that manufactured the false Healthy"
        );
        mgr.stop_all().await.unwrap();
    }

    /// **QUADRANT 2 — the credential is ABSENT. Proof the working case still
    /// works.**
    ///
    /// This is the behaviour the live UAT confirmed was already correct, and
    /// the one most at risk from this change. `start()` fails, so the channel
    /// must read `Disconnected` — NOT `Unauthenticated` — and the reason must
    /// still name the handle. Two distinct non-healthy states with two distinct
    /// operator actions: "configure the credential" vs "rotate it".
    #[tokio::test]
    async fn an_absent_credential_still_reports_disconnected_naming_the_handle() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
        mgr.register(Box::new(FailingStartChannel {
            name: "absent".into(),
        }))
        .await;
        mgr.start_all().await.unwrap();

        let h = mgr.health_of("absent").expect("registered");
        assert_eq!(
            h.state,
            HealthState::Disconnected,
            "an absent credential is Disconnected, not Unauthenticated"
        );
        assert_ne!(
            h.state,
            HealthState::Unauthenticated,
            "the new auth path must not swallow the absent-credential case"
        );
        assert!(
            h.reason
                .as_deref()
                .is_some_and(|r| r.contains("missing credential")),
            "the reason must still name what is missing: {:?}",
            h.reason
        );
        mgr.stop_all().await.unwrap();
    }

    /// **QUADRANT 3 — everything is fine.** A working channel must never be
    /// dragged into `Unauthenticated` by the new path. A health surface that
    /// cries "rotate your token" at a healthy channel is worse than the bug
    /// this fixes, so this control is load-bearing.
    ///
    /// The delivered message is the known-positive: without it a channel that
    /// polled nothing at all would pass the absence assertion for free.
    #[tokio::test]
    async fn a_working_channel_is_never_reported_unauthenticated() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
        let mut rx = mgr.subscribe();
        let mut ok = MockChannel::new("fine");
        ok.inject_text("c1", "alice", "hi");
        mgr.register(Box::new(ok)).await;
        mgr.start_all().await.unwrap();

        // Known-positive: the channel really is polling and delivering.
        let mut delivered = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline && !delivered {
            if let Ok(Ok(tagged)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
                && matches!(tagged.event, ChannelEvent::MessageReceived { .. })
            {
                delivered = true;
            }
        }
        assert!(
            delivered,
            "known-positive failed: the healthy channel delivered nothing, so \
             the assertion below would be vacuous"
        );

        // Poll repeatedly across many ticks — a spurious fire would show up as
        // a transition at some point, not necessarily the first sample.
        for _ in 0..40 {
            let h = mgr.health_of("fine").expect("registered");
            assert_ne!(
                h.state,
                HealthState::Unauthenticated,
                "a healthy channel was reported as having a rejected credential"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            mgr.health_of("fine").expect("registered").state,
            HealthState::Healthy,
            "a working channel must end Healthy"
        );
        mgr.stop_all().await.unwrap();
    }

    /// A dead task with an EMPTY inbox must still drive supervised reconnect.
    ///
    /// The drain-before-dead reordering could plausibly have disabled the
    /// silent-death detection entirely; `dead_internal_task_triggers_supervised_reconnect`
    /// above is that guard, and this asserts the health projection side of it —
    /// a silently dead task is `Degraded`, never `Unauthenticated`.
    #[tokio::test]
    async fn a_silently_dead_task_is_degraded_not_unauthenticated() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
        mgr.register(Box::new(DeadTaskChannel::new("silent"))).await;
        mgr.start_all().await.unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut saw_degraded = false;
        while std::time::Instant::now() < deadline {
            let h = mgr.health_of("silent").expect("registered");
            assert_ne!(
                h.state,
                HealthState::Unauthenticated,
                "a dead task is not a credential rejection"
            );
            if h.state == HealthState::Degraded {
                saw_degraded = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            saw_degraded,
            "the empty-inbox dead-task signal must still be detected after the \
             drain-first reordering"
        );
        mgr.stop_all().await.unwrap();
    }

    /// Test-only channel whose `ingest_webhook` rendezvouses on a shared
    /// barrier before returning. Two such channels sharing one barrier let a
    /// test prove the manager does NOT serialize concurrent ingests across
    /// different channels: both calls must reach the barrier for either to
    /// proceed, so if the manager pinned a borrow/lock across the async ingest
    /// the pair would deadlock. Returns a response carrying its own name so the
    /// test can confirm routing landed on the right connector.
    struct BarrierChannel {
        name: String,
        barrier: Arc<tokio::sync::Barrier>,
    }

    #[async_trait]
    impl Channel for BarrierChannel {
        fn name(&self) -> &str {
            &self.name
        }
        fn platform(&self) -> &str {
            "barrier"
        }
        async fn start(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), ChannelError> {
            Ok(())
        }
        async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
            Ok(Vec::new())
        }
        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<MessageReceipt, ChannelError> {
            // Rendezvous on the shared barrier exactly like `ingest_webhook`, so
            // a test can cross a *send* on one channel with an *ingest* on
            // another and prove the outer manager lock did not serialize them
            // (rank 14). Only unblocks if both halves run concurrently.
            self.barrier.wait().await;
            Ok(MessageReceipt {
                id: "barrier-out".into(),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }
        fn config_schema(&self) -> &str {
            r#"{"name":"string","platform":"barrier"}"#
        }
        async fn ingest_webhook(
            &self,
            _req: &crate::webhook::WebhookRequest,
        ) -> Result<crate::webhook::WebhookResponse, ChannelError> {
            // Block until the sibling channel's ingest also arrives. This only
            // unblocks if both ingests run concurrently — i.e. the manager
            // released its map/`self` borrow before awaiting the ingest.
            self.barrier.wait().await;
            Ok(crate::webhook::WebhookResponse::challenge(
                self.name.clone(),
            ))
        }
    }

    #[tokio::test]
    async fn concurrent_ingest_to_different_channels_does_not_serialize() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(BarrierChannel {
            name: "alpha".into(),
            barrier: Arc::clone(&barrier),
        }))
        .await;
        mgr.register(Box::new(BarrierChannel {
            name: "beta".into(),
            barrier: Arc::clone(&barrier),
        }))
        .await;

        let mgr = Arc::new(mgr);
        let req = crate::webhook::WebhookRequest::default();

        let m1 = Arc::clone(&mgr);
        let req1 = req.clone();
        let h1 = tokio::spawn(async move { m1.ingest_webhook("alpha", &req1).await });
        let m2 = Arc::clone(&mgr);
        let req2 = req.clone();
        let h2 = tokio::spawn(async move { m2.ingest_webhook("beta", &req2).await });

        // If the manager serialized the two ingests, neither barrier.wait()
        // would ever see its partner and this would time out.
        let (r1, r2) = tokio::time::timeout(Duration::from_secs(5), async {
            (h1.await.unwrap(), h2.await.unwrap())
        })
        .await
        .expect("concurrent ingests must not serialize (deadlocked on barrier)");

        // Routing landed on the correct connector: each response echoes the
        // channel name the request was addressed to.
        assert_eq!(r1.expect("alpha ok").body.as_deref(), Some("alpha"));
        assert_eq!(r2.expect("beta ok").body.as_deref(), Some("beta"));
    }

    /// rank 14: the manager is shared through the engine as
    /// `Arc<tokio::sync::RwLock<ChannelManager>>`. The read-path router methods
    /// (`ingest_webhook`, `send_to`, …) take `&self`, so concurrent callers
    /// acquire a *shared read guard* and run in parallel — the outer lock no
    /// longer serializes cross-channel traffic. This test crosses an
    /// `ingest_webhook` on one channel with a `send_to` on another, both under
    /// `.read().await`, on the same barrier: it only completes if the two read
    /// guards coexist. If the outer lock were a `Mutex` (or these were write
    /// guards) the second call would block until the first released, neither
    /// barrier half would meet its partner, and the test would time out.
    #[tokio::test]
    async fn concurrent_ingest_and_send_across_channels_share_read_guard() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(BarrierChannel {
            name: "alpha".into(),
            barrier: Arc::clone(&barrier),
        }))
        .await;
        mgr.register(Box::new(BarrierChannel {
            name: "beta".into(),
            barrier: Arc::clone(&barrier),
        }))
        .await;

        // Outer lock matches the engine wiring (rank 14): Arc<RwLock<_>>.
        let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

        // Half 1: webhook ingest on `alpha`, under a read guard.
        let m1 = Arc::clone(&mgr);
        let h1 = tokio::spawn(async move {
            let guard = m1.read().await;
            guard
                .ingest_webhook("alpha", &crate::webhook::WebhookRequest::default())
                .await
                .map(|resp| resp.body)
        });

        // Half 2: outbound send on `beta`, under a *second* concurrent read
        // guard. Both must be live at once for the shared barrier to release.
        let m2 = Arc::clone(&mgr);
        let h2 = tokio::spawn(async move {
            let guard = m2.read().await;
            guard
                .send_to("beta", OutgoingMessage::text("room", "ping"))
                .await
                .map(|receipt| receipt.id)
        });

        let (r1, r2) = tokio::time::timeout(Duration::from_secs(5), async {
            (h1.await.unwrap(), h2.await.unwrap())
        })
        .await
        .expect("a concurrent ingest + send across channels must not serialize on the outer lock");

        // Each landed on its own connector.
        assert_eq!(r1.expect("ingest ok").as_deref(), Some("alpha"));
        assert_eq!(r2.expect("send ok"), "barrier-out");
    }

    #[tokio::test]
    async fn ingest_webhook_unknown_channel_errors() {
        let mgr = ChannelManager::new();
        let err = mgr
            .ingest_webhook("missing", &crate::webhook::WebhookRequest::default())
            .await
            .expect_err("unknown channel must error");
        assert!(matches!(err, ChannelError::Config(_)));
    }

    #[tokio::test]
    async fn injected_inbound_reaches_subscriber() {
        let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(15));
        let mut rx = mgr.subscribe();
        let mut ch = MockChannel::new("alpha");
        ch.inject_text("c1", "alice", "hi");
        mgr.register(Box::new(ch)).await;
        mgr.start_all().await.unwrap();

        // Loop until we see the MessageReceived (skip state-change).
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut got_msg = false;
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(tagged)) => {
                    if matches!(tagged.event, ChannelEvent::MessageReceived { .. }) {
                        got_msg = true;
                        break;
                    }
                }
                _ => continue,
            }
        }
        assert!(
            got_msg,
            "expected to receive an injected MessageReceived event"
        );
        mgr.stop_all().await.unwrap();
    }
}
