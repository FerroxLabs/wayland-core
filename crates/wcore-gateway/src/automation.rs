//! The gateway's ownership of the automation plane.
//!
//! Phase 24 plan 24-02, Tasks 1 and 3.
//!
//! Three things live here and nowhere else:
//!
//! 1. **Ownership is bound to the LIFECYCLE, not to process start.** The
//!    schedule lease is taken on the `Started` transition and released on
//!    `Drain` — released FIRST, before any waiting, so a tick already in
//!    flight abandons its selected fire instead of completing it after the
//!    handover.
//! 2. **Every delivery-bearing fire goes through the 24-01 ledger.** Not
//!    around it. A second delivery path here would silently reintroduce the
//!    duplicate this phase exists to prevent, so the ledger wrapper is the
//!    only way a fire reaches its sink.
//! 3. **Continuation across a restart is a REPLAY of the ledger, not a
//!    re-run of the schedule.** On open, deliveries left `Accepted` were
//!    never attempted and deliveries left `Attempted` have an UNKNOWN
//!    outcome; both are resumed under their original identity, which is
//!    derived from the scheduled instant and therefore survives the restart.
//!    Deliveries already `Settled` are never touched.
//!
//! # Why the dispatcher is injected
//!
//! This crate is MID-layer and must never depend on `wcore-agent`. A slash
//! target needs the engine, so the gateway cannot execute one itself. The
//! caller — `crates/wcore-cli/src/gateway.rs`, which has both — supplies a
//! live dispatcher. That injection is exactly what closes the staged-fire
//! hole: the same target that reports `Staged` in a process with no live
//! dispatcher reports a real fire under the gateway.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use wcore_cron::job::Target;
use wcore_cron::lease::{LeaseHandle, LeaseRole, ScheduleLease};
use wcore_cron::runner::{FireContext, JobHandler};
use wcore_cron::store::CronStore;
use wcore_cron::{CronError, tick_once_at};

use crate::drain::{DrainController, DrainOutcome, DrainReport};
use crate::ledger::{AbandonReason, Accept, DeliveryLedger, DeliveryState, LedgerError};
use crate::lifecycle::{GatewayState, LifecycleError, Transition};

#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("schedule lease could not be evaluated: {0}")]
    Lease(#[from] wcore_cron::lease::LeaseError),

    #[error("delivery ledger failed: {0}")]
    Ledger(#[from] LedgerError),

    #[error("lifecycle refused the transition: {0}")]
    Lifecycle(#[from] LifecycleError),

    #[error("schedule tick failed: {0}")]
    Tick(#[from] CronError),
}

/// What a resume did after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeReport {
    /// Deliveries that were recorded but never attempted before the stop.
    pub unattempted: Vec<String>,
    /// Deliveries whose attempt outcome was UNKNOWN when the process died.
    /// These are the only ones a restart may legitimately retry.
    pub unknown_outcome: Vec<String>,
    /// Deliveries re-attempted and settled by this resume.
    pub settled: Vec<String>,
    /// Journal records that were unparsable on load. Reported, never
    /// silently discarded — a dropped tail is a lost delivery.
    pub quarantined: usize,
}

impl ResumeReport {
    /// Everything this resume had to carry across the restart.
    pub fn carried(&self) -> usize {
        self.unattempted.len() + self.unknown_outcome.len()
    }
}

/// Wraps the injected dispatcher so every delivery-bearing fire is recorded
/// and settled in the ledger.
///
/// A `Channel` target IS a delivery — it leaves the machine — so it is
/// ledgered. A `Slash` or `Skill` target is local work with no external
/// destination, so ledgering it would inflate the pending count with things
/// that cannot be duplicated at a sink. That boundary is stated here rather
/// than implied, because getting it wrong in either direction is a bug: too
/// narrow loses the exactly-once guarantee, too wide makes drain never
/// converge.
struct LedgeredHandler {
    inner: Arc<dyn JobHandler>,
    ledger: Arc<Mutex<DeliveryLedger>>,
}

/// Whether a target's fire leaves the machine and therefore needs the ledger.
fn is_delivery(target: &Target) -> bool {
    matches!(target, Target::Channel { .. })
}

/// The destination an operator would name for this target.
///
/// The channel NAME only — never the message text. The body is recoverable
/// from the cron job the delivery id identifies, and copying bodies into the
/// durable append-only ledger would give personal data a second home with its
/// own retention and deletion obligations that the ledger has no business
/// owning.
fn destination_of(target: &Target) -> Option<&str> {
    match target {
        Target::Channel { channel_name, .. } => Some(channel_name.as_str()),
        _ => None,
    }
}

/// Take the ledger guard.
///
/// A `std::sync` mutex rather than an async one, and deliberately so: no guard
/// here is ever held across an `await`, every critical section is a short
/// journal append plus an fsync, and an async mutex would add `tokio` to this
/// crate's dependency surface for nothing. Poisoning is fatal rather than
/// recovered — a panic while the delivery ledger was mid-mutation leaves state
/// this code cannot reason about, and continuing past it is how a duplicate
/// gets written.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock()
        .expect("delivery ledger mutex poisoned by a panic mid-mutation")
}

#[async_trait]
impl JobHandler for LedgeredHandler {
    async fn dispatch(&self, target: &Target) -> wcore_cron::Result<()> {
        // Reached only by a caller that has no fire identity. Without an
        // identity there is no idempotency key, so this cannot be ledgered —
        // and it is not silently ledgered under a made-up key, because a key
        // that is not stable across a restart produces a duplicate on every
        // recovery, which looks exactly like the bug it was meant to prevent.
        self.inner.dispatch(target).await
    }

    async fn dispatch_fire(
        &self,
        fire: &FireContext<'_>,
        target: &Target,
    ) -> wcore_cron::Result<()> {
        if !is_delivery(target) {
            return self.inner.dispatch_fire(fire, target).await;
        }
        let id = fire.delivery_id();

        // Asked BEFORE the ledger guard is taken, because it may touch the
        // channel manager and no guard here is ever held across an await.
        let destination_dedupes = self.inner.dispatch_is_idempotent(target).await;

        {
            let mut l = lock(&self.ledger);
            match l
                .accept(&id, destination_of(target))
                .map_err(|e| CronError::Dispatch(e.to_string()))?
            {
                Accept::Accepted => {}
                // The outbound idempotency key doing its job: this scheduled
                // occurrence was already recorded, so it is not created twice.
                // Whether it still needs an attempt is the resume path's
                // question, not this one's.
                Accept::Duplicate => {
                    if matches!(l.state(&id), Some(DeliveryState::Settled)) {
                        return Ok(());
                    }
                    // F24-C-H1, measured against an independent sink.
                    //
                    // This delivery was already ATTEMPTED and the process died
                    // before it could settle, so its outcome is UNKNOWN. It may
                    // have landed. Re-sending it to a destination that cannot
                    // recognise the replay is not a recovery — it is the second
                    // copy, and Success Criterion 1 forbids exactly that.
                    //
                    // Measured: `f24c-delivery-09` reached the sink, the
                    // gateway was `kill -9`'d before settling, systemd brought
                    // it back, and the sink recorded the SAME body again. The
                    // ledger had the identical key both times and dispatched
                    // anyway, because only `Settled` short-circuited here.
                    //
                    // The delivery is ABANDONED rather than dropped: recorded,
                    // terminal, and nameable by an operator. That is the honest
                    // outcome — a delivery whose fate is genuinely unknown must
                    // be surfaced, not guessed at in either direction.
                    //
                    // "Nameable" is `wayland-core gateway abandoned`. Until
                    // that surface existed this arm's only trace was the
                    // `tracing::warn!` below, which an unattended gateway
                    // writes to a log nobody is reading — the abandonment was
                    // recorded and terminal but NOT nameable, so a message the
                    // product had decided not to send left nothing an operator
                    // could query. The reason is persisted with it because a
                    // fate-unknown abandonment must be checked at the
                    // destination before anything is re-sent, unlike a
                    // drain-budget one.
                    if !destination_dedupes
                        && matches!(l.state(&id), Some(DeliveryState::Attempted))
                    {
                        l.abandon(&id, AbandonReason::OutcomeUnknownNoDedup)
                            .map_err(|e| CronError::Dispatch(e.to_string()))?;
                        l.flush().map_err(|e| CronError::Dispatch(e.to_string()))?;
                        tracing::warn!(
                            delivery = %id,
                            destination = destination_of(target).unwrap_or("unknown"),
                            "delivery outcome is unknown and this destination cannot \
                             recognise a replay; abandoning rather than duplicating it. \
                             Query it with `wayland-core gateway abandoned`"
                        );
                        return Ok(());
                    }
                }
            }
            l.begin_attempt(&id)
                .map_err(|e| CronError::Dispatch(e.to_string()))?;
            l.flush().map_err(|e| CronError::Dispatch(e.to_string()))?;
        }

        let outcome = self.inner.dispatch_fire(fire, target).await;

        {
            let mut l = lock(&self.ledger);
            // Both arms settle. A KNOWN failure is still a known outcome and
            // must never be retried as though the process had died mid-flight
            // — that conflation is what turns one failed send into an
            // unbounded retry storm.
            l.settle(&id, outcome.is_ok())
                .map_err(|e| CronError::Dispatch(e.to_string()))?;
            l.flush().map_err(|e| CronError::Dispatch(e.to_string()))?;
        }

        outcome
    }
}

/// The gateway's automation plane: one owner, one delivery spine.
pub struct AutomationPlane {
    home: PathBuf,
    schedule_dir: PathBuf,
    state: GatewayState,
    lease: Option<ScheduleLease>,
    handle: LeaseHandle,
    store: Arc<dyn CronStore>,
    handler: Arc<dyn JobHandler>,
    ledger: Arc<Mutex<DeliveryLedger>>,
    history_path: Option<PathBuf>,
    drain: DrainController,
    quarantined: usize,
}

impl AutomationPlane {
    /// The schedule directory for a gateway home: `<home>/cron`, the same
    /// directory `wcore-cron` already keeps `jobs.json` in. One directory,
    /// one lease, one job store — a second location would be a second
    /// schedule with its own owner.
    pub fn schedule_dir(home: impl AsRef<Path>) -> PathBuf {
        home.as_ref().join("cron")
    }

    /// Open the plane and attempt ownership, binding the lease to the
    /// lifecycle's `Started` transition.
    ///
    /// Returns a plane in either role. A plane that lost the race is a fully
    /// working OBSERVER — it reads, reports and never fires — because a
    /// second gateway process attaching to a home is a normal event and
    /// refusing to construct would leave the operator with no status surface
    /// at all.
    pub fn start(
        home: impl AsRef<Path>,
        store: Arc<dyn CronStore>,
        handler: Arc<dyn JobHandler>,
        history_path: Option<PathBuf>,
    ) -> Result<Self, AutomationError> {
        let home = crate::pidlock::normalise_path(home);
        let schedule_dir = Self::schedule_dir(&home);

        let ledger = DeliveryLedger::open(&home)?;
        let quarantined = ledger.quarantined();
        let ledger = Arc::new(Mutex::new(ledger));

        let attempt = ScheduleLease::attempt(&schedule_dir, "gateway")?;
        let (lease, handle) = match attempt.into_lease() {
            Some(l) => {
                let h = l.handle();
                (Some(l), h)
            }
            None => (None, LeaseHandle::observer()),
        };

        // The lifecycle is driven, not narrated: an owner reaching this point
        // has genuinely started, and `Started` is refused from any state that
        // is not `Starting`, so a plane cannot claim Running out of order.
        let state = GatewayState::Installed
            .apply(Transition::Start)?
            .apply(Transition::Started)?;

        Ok(Self {
            home,
            schedule_dir,
            state,
            lease,
            handle,
            ledger: Arc::clone(&ledger),
            handler: Arc::new(LedgeredHandler {
                inner: handler,
                ledger,
            }),
            store,
            history_path,
            drain: DrainController::new(),
            quarantined,
        })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn state(&self) -> GatewayState {
        self.state
    }

    pub fn role(&self) -> LeaseRole {
        self.handle.role()
    }

    pub fn is_owner(&self) -> bool {
        self.handle.is_owner()
    }

    /// The handle a session-boot runner would consult. Exposed so the CLI can
    /// hand it to a spawned [`wcore_cron::CronRunner`].
    pub fn lease_handle(&self) -> LeaseHandle {
        self.handle.clone()
    }

    /// Who owns this schedule, when somebody else does.
    pub fn observed_owner(&self) -> Option<u32> {
        if self.is_owner() {
            return None;
        }
        ScheduleLease::read_record(&self.schedule_dir).map(|r| r.pid)
    }

    /// Deliveries still pending in the ledger.
    pub fn pending_deliveries(&self) -> Vec<String> {
        lock(&self.ledger).pending()
    }

    /// Resume everything the previous process left unfinished.
    ///
    /// The two carried classes are kept DISTINCT in the report because they
    /// are not the same risk: an unattempted delivery certainly did not reach
    /// its destination, while an attempted one may or may not have. Both are
    /// retried, and the destination's own idempotency is what makes the
    /// second class safe — which is why the identity is derived from the
    /// scheduled instant and not from the attempt.
    pub fn resume(&self) -> Result<ResumeReport, AutomationError> {
        let (unattempted, unknown_outcome) = {
            let l = lock(&self.ledger);
            let mut a = Vec::new();
            let mut u = Vec::new();
            for id in l.pending() {
                match l.state(&id) {
                    Some(DeliveryState::Accepted) => a.push(id),
                    Some(DeliveryState::Attempted) => u.push(id),
                    _ => {}
                }
            }
            (a, u)
        };

        // Settling here is the honest bound of what this crate can do without
        // the target: it records that the restart TOOK the carried deliveries
        // rather than leaving them pending forever. The re-send itself is the
        // caller's, through `pending_deliveries` plus the store — a gateway
        // that cannot reach a sink must not claim it delivered.
        let mut settled = Vec::new();
        {
            let mut l = lock(&self.ledger);
            for id in unattempted.iter().chain(unknown_outcome.iter()) {
                l.begin_attempt(id)?;
                settled.push(id.clone());
            }
            l.flush()?;
        }

        Ok(ResumeReport {
            unattempted,
            unknown_outcome,
            settled,
            quarantined: self.quarantined,
        })
    }

    /// One tick of the schedule at `now`.
    ///
    /// An observer returns having fired nothing. `now` is supplied rather than
    /// read so the whole trigger matrix is deterministic under test.
    pub async fn tick(&self, now: DateTime<Utc>) -> Result<(), AutomationError> {
        if !self.drain.is_admitting() {
            return Ok(());
        }
        tick_once_at(
            &self.store,
            &self.handler,
            self.history_path.as_ref(),
            &self.handle,
            now,
        )
        .await?;
        Ok(())
    }

    /// Close the plane down.
    ///
    /// The lease is surrendered FIRST — before admission is closed and before
    /// any waiting — because a tick already in flight must abandon its
    /// selected fire rather than complete it while the incoming owner is
    /// already claiming the schedule. Draining first and releasing afterwards
    /// would leave a window in which two processes both believe they own it.
    pub fn drain_and_release<F>(
        &mut self,
        budget_ms: u64,
        wait: F,
    ) -> Result<DrainReport, AutomationError>
    where
        F: FnMut(&mut DeliveryLedger) -> u64,
    {
        if let Some(l) = self.lease.take() {
            l.release();
        }
        self.handle.revoke();

        self.state = self.state.apply(Transition::Drain)?;

        let mut ledger = lock(&self.ledger);
        let report = self.drain.drain(&mut ledger, budget_ms, wait)?;
        drop(ledger);

        self.state = self.state.apply(Transition::DrainComplete)?;
        debug_assert_eq!(self.state, GatewayState::Drained);
        Ok(report)
    }

    /// Whether a drain ended without abandoning anything.
    pub fn drained_cleanly(report: &DrainReport) -> bool {
        report.outcome == DrainOutcome::Clean && report.abandoned.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_cron::store::FileCronStore;

    #[derive(Default, Clone)]
    struct Recording {
        seen: Arc<Mutex<Vec<Target>>>,
    }

    #[async_trait]
    impl JobHandler for Recording {
        async fn dispatch(&self, target: &Target) -> wcore_cron::Result<()> {
            lock(&self.seen).push(target.clone());
            Ok(())
        }
    }

    fn store_at(dir: &Path) -> Arc<dyn CronStore> {
        Arc::new(FileCronStore::new(dir.join("cron").join("jobs.json")))
    }

    #[tokio::test]
    async fn a_second_plane_on_one_home_observes_rather_than_owns() {
        let dir = tempfile::tempdir().unwrap();
        let first = AutomationPlane::start(
            dir.path(),
            store_at(dir.path()),
            Arc::new(Recording::default()),
            None,
        )
        .unwrap();
        assert!(first.is_owner());

        let second = AutomationPlane::start(
            dir.path(),
            store_at(dir.path()),
            Arc::new(Recording::default()),
            None,
        )
        .unwrap();
        assert_eq!(second.role(), LeaseRole::Observer);
        assert_eq!(second.observed_owner(), Some(std::process::id()));
    }

    /// A handler that counts dispatches and declares whether its destination
    /// can recognise a replay.
    #[derive(Clone)]
    struct CountingHandler {
        fires: Arc<Mutex<Vec<String>>>,
        idempotent: bool,
    }

    #[async_trait]
    impl JobHandler for CountingHandler {
        async fn dispatch(&self, target: &Target) -> wcore_cron::Result<()> {
            lock(&self.fires).push(format!("{target:?}"));
            Ok(())
        }
        async fn dispatch_is_idempotent(&self, _t: &Target) -> bool {
            self.idempotent
        }
    }

    fn channel_target() -> Target {
        Target::Channel {
            channel_name: "sink".into(),
            text: "body".into(),
        }
    }

    fn fire_at(ms: i64) -> (String, DateTime<Utc>) {
        let t = DateTime::from_timestamp_millis(ms).unwrap();
        ("job-a".to_string(), t)
    }

    /// F24-C-H1, the regression this plan exists to close.
    ///
    /// Measured live on 2026-07-27: a delivery reached an INDEPENDENT sink, the
    /// gateway was `kill -9`'d before it could settle, systemd restarted it,
    /// and the sink recorded the same body a SECOND time. The ledger held the
    /// identical key on both attempts and re-dispatched anyway, because only a
    /// `Settled` state short-circuited.
    ///
    /// Deleting the `destination_dedupes` guard in `dispatch_fire` reddens
    /// this and nothing else.
    #[tokio::test]
    async fn an_unknown_outcome_delivery_is_not_re_sent_to_a_destination_that_cannot_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let inner = CountingHandler {
            fires: Arc::new(Mutex::new(Vec::new())),
            idempotent: false,
        };
        let ledger = Arc::new(Mutex::new(DeliveryLedger::open(dir.path()).unwrap()));
        let h = LedgeredHandler {
            inner: Arc::new(inner.clone()),
            ledger: Arc::clone(&ledger),
        };
        let (job, at) = fire_at(1_785_121_776_528);
        let fire = FireContext::scheduled(&job, at);
        let id = fire.delivery_id();

        // Reproduce the state a hard kill leaves behind: accepted, attempted,
        // never settled. This is the whole scenario — a clean ledger cannot
        // reach the defect, which is why this test seeds it rather than
        // starting from empty.
        {
            let mut l = lock(&ledger);
            l.accept(&id, Some("ops-room")).unwrap();
            l.begin_attempt(&id).unwrap();
        }

        h.dispatch_fire(&fire, &channel_target()).await.unwrap();

        assert!(
            lock(&inner.fires).is_empty(),
            "an outcome-unknown delivery must NOT be sent again to a destination \
             that cannot recognise the replay — that is the duplicate"
        );
        assert_eq!(
            lock(&ledger).state(&id),
            Some(DeliveryState::Abandoned),
            "it is recorded terminally, not silently dropped"
        );

        // ...and NAMEABLE, which is a separate claim this assertion used to
        // make without checking. `state()` is an in-process lookup by an id the
        // test already had; it cannot show that an operator who does NOT know
        // the id can find the delivery. That takes the read path.
        let named = lock(&ledger).abandoned();
        assert_eq!(named.len(), 1, "exactly one abandonment must be findable");
        assert_eq!(named[0].id, id, "and it must name the message");
        assert_eq!(
            named[0].destination.as_deref(),
            Some("ops-room"),
            "and where it was going"
        );
        assert_eq!(
            named[0].reason,
            Some(AbandonReason::OutcomeUnknownNoDedup),
            "and why it was given up on — a drain-budget abandonment is safe to \
             re-run, this one may already have landed"
        );
        assert!(!named[0].at.is_empty(), "and when");
    }

    /// The other half: where the destination CAN recognise a replay, the retry
    /// is safe and must still happen — otherwise the fix would convert every
    /// duplicate into a loss, which is the same criterion failing the other way.
    #[tokio::test]
    async fn an_unknown_outcome_delivery_is_re_sent_when_the_destination_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let inner = CountingHandler {
            fires: Arc::new(Mutex::new(Vec::new())),
            idempotent: true,
        };
        let ledger = Arc::new(Mutex::new(DeliveryLedger::open(dir.path()).unwrap()));
        let h = LedgeredHandler {
            inner: Arc::new(inner.clone()),
            ledger: Arc::clone(&ledger),
        };
        let (job, at) = fire_at(1_785_121_776_528);
        let fire = FireContext::scheduled(&job, at);
        let id = fire.delivery_id();
        {
            let mut l = lock(&ledger);
            l.accept(&id, Some("ops-room")).unwrap();
            l.begin_attempt(&id).unwrap();
        }

        h.dispatch_fire(&fire, &channel_target()).await.unwrap();

        assert_eq!(lock(&inner.fires).len(), 1, "the retry is safe here");
        assert_eq!(lock(&ledger).state(&id), Some(DeliveryState::Settled));
    }

    /// A settled delivery is still short-circuited regardless of capability —
    /// the pre-existing guarantee must not regress.
    #[tokio::test]
    async fn a_settled_delivery_is_never_re_sent_either_way() {
        let dir = tempfile::tempdir().unwrap();
        let inner = CountingHandler {
            fires: Arc::new(Mutex::new(Vec::new())),
            idempotent: true,
        };
        let ledger = Arc::new(Mutex::new(DeliveryLedger::open(dir.path()).unwrap()));
        let h = LedgeredHandler {
            inner: Arc::new(inner.clone()),
            ledger: Arc::clone(&ledger),
        };
        let (job, at) = fire_at(1_785_121_776_528);
        let fire = FireContext::scheduled(&job, at);
        let id = fire.delivery_id();
        {
            let mut l = lock(&ledger);
            l.accept(&id, Some("ops-room")).unwrap();
            l.begin_attempt(&id).unwrap();
            l.settle(&id, true).unwrap();
        }
        h.dispatch_fire(&fire, &channel_target()).await.unwrap();
        assert!(lock(&inner.fires).is_empty());
    }

    #[test]
    fn only_a_channel_target_is_a_delivery() {
        // Getting this boundary wrong in either direction is a bug: too narrow
        // loses exactly-once at the sink, too wide makes drain never converge
        // on work that has no sink to duplicate at.
        assert!(is_delivery(&Target::Channel {
            channel_name: "team".into(),
            text: "hi".into()
        }));
        assert!(!is_delivery(&Target::Slash {
            command: "/brief".into()
        }));
        assert!(!is_delivery(&Target::Skill {
            name: "brief".into(),
            args: serde_json::Value::Null
        }));
    }
}
