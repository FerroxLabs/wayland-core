//! What `TriggerBound::max_in_flight` actually does. Measured, not read.
//!
//! Phase 24 Criterion 2, lane `24-c4-support`. `24-PHASE-VERDICT.md` §3 records
//! the field as *"stored and clamped but not enforced at dispatch"*. That is
//! nearly right and the difference matters, so it is measured here rather than
//! restated: the field is not an unenforced bound, it is a bound on a quantity
//! **the runner's dispatch model cannot produce**. `dispatch_and_record` is
//! `.await`ed inline inside the selection loop and the production handler
//! (`wcore_agent::cron::EngineJobHandler`) does not spawn, so a job's fires are
//! serialized end to end and no job can ever have two outstanding.
//!
//! # Why that is a finding and not a shrug
//!
//! `cron show` renders `max_in_flight=<N>` for any N the job carries, up to
//! `CEILING_IN_FLIGHT` (16). An operator reading that is told they may have up
//! to N fires of this job outstanding. They may have one. This is the same
//! shape as the `poll:` trigger this phase already retired — a surface
//! promising behaviour the runtime does not implement — except inverted: poll
//! claimed a fire that never happened, this claims concurrency that never
//! happens.
//!
//! # The measurement is DIFFERENTIAL, and the probe carries a positive control
//!
//! A bare "peak concurrency was 1" is free on a broken probe: a counter that
//! never increments, a handler that is never called, a scenario that produced
//! no fires at all. So this file
//!
//!   1. proves the probe CAN see concurrency above 1, by driving two
//!      dispatches through it concurrently on purpose;
//!   2. proves the scenario really fires, by asserting a non-zero fire count;
//!   3. compares `max_in_flight = 1` against `max_in_flight = 8` and shows the
//!      two are INDISTINGUISHABLE.
//!
//! Assertion 3 is the one that carries the claim. Assertions 1 and 2 are what
//! make it worth anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use wcore_cron::job::Target;
use wcore_cron::lease::LeaseHandle;
use wcore_cron::runner::JobHandler;
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::trigger::{CEILING_IN_FLIGHT, Trigger, TriggerBound};
use wcore_cron::{CronJob, Result, tick_once_at};

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap()
}

/// Records the PEAK number of dispatches inside the handler at one time.
///
/// Holds an await point between the increment and the decrement, so two
/// overlapping dispatches are observable. Without that yield the probe could
/// report 1 purely because it never gave the executor a chance to interleave —
/// which would be a dead instrument producing the answer this file is looking
/// for.
#[derive(Default, Clone)]
struct ConcurrencyProbe {
    inside: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    fires: Arc<AtomicUsize>,
}

impl ConcurrencyProbe {
    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
    fn fires(&self) -> usize {
        self.fires.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JobHandler for ConcurrencyProbe {
    async fn dispatch(&self, _t: &Target) -> Result<()> {
        let now = self.inside.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.fires.fetch_add(1, Ordering::SeqCst);
        // A real await point, so an overlapping dispatch can actually be seen.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        self.inside.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

fn slash(cmd: &str) -> Target {
    Target::Slash {
        command: cmd.to_string(),
    }
}

/// One interval job, long overdue, carrying `max_in_flight = n`. Returns the
/// probe after a single tick.
async fn drive_overdue_interval(n: u32) -> ConcurrencyProbe {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn CronStore> = Arc::new(FileCronStore::new(dir.path().join("jobs.json")));
    let probe = ConcurrencyProbe::default();
    let handler: Arc<dyn JobHandler> = Arc::new(probe.clone());

    let mut job = CronJob::with_trigger(
        Trigger::Interval { every_secs: 60 },
        slash("/in-flight-probe"),
    )
    .unwrap();
    job.created_at = t0();
    job.last_fired = Some(t0());
    // The bound under test. `clamp_to` narrows it against the variant default,
    // so the stored value is whatever the product would really carry.
    job.bound = Some(TriggerBound::new(1, n));
    store.insert(job.clone()).await.unwrap();

    // Thirty periods overdue. If the dispatcher had any notion of outstanding
    // fires, this is the scenario in which it would use it.
    let now = t0() + Duration::minutes(30);
    tick_once_at(&store, &handler, None, &LeaseHandle::unleased(), now)
        .await
        .unwrap();
    probe
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_probe_can_observe_concurrency_above_one() {
    // POSITIVE CONTROL, and the load-bearing one. Every other assertion in
    // this file is "concurrency stayed at 1", which a probe that cannot count
    // past 1 — or that is never called — satisfies for free.
    let probe = ConcurrencyProbe::default();
    let a = probe.clone();
    let b = probe.clone();
    let (ra, rb) = tokio::join!(async move { a.dispatch(&slash("/a")).await }, async move {
        b.dispatch(&slash("/b")).await
    },);
    ra.unwrap();
    rb.unwrap();
    assert_eq!(probe.fires(), 2, "both dispatches must have run");
    assert_eq!(
        probe.peak(),
        2,
        "the probe must be able to SEE two overlapping dispatches, or every \
         'peak was 1' result below is meaningless"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generous_in_flight_bound_buys_no_concurrency_at_all() {
    // The measurement. `max_in_flight = 8` against `max_in_flight = 1`, same
    // scenario, and the two must be indistinguishable — which is what makes
    // the field decorative rather than merely permissive.
    let one = drive_overdue_interval(1).await;
    let eight = drive_overdue_interval(8).await;

    // Anti-vacuity: the scenario really did fire. A run that dispatched
    // nothing would report peak 1 (in fact 0) and prove nothing.
    assert!(
        one.fires() > 0 && eight.fires() > 0,
        "the scenario must actually fire: one={} eight={}",
        one.fires(),
        eight.fires()
    );

    assert_eq!(
        one.peak(),
        1,
        "a bound of 1 should hold at 1 (it does, but not because it is enforced)"
    );
    assert_eq!(
        eight.peak(),
        1,
        "MEASURED: `max_in_flight = 8` produced peak concurrency {}. The runner \
         awaits `dispatch_and_record` inline, so a job's fires are serialized \
         end to end and the bound has no subject. If this assertion ever \
         reddens, the dispatch model grew real concurrency and \
         `max_in_flight` must be enforced at the new dispatch point.",
        eight.peak()
    );
    assert_eq!(
        one.peak(),
        eight.peak(),
        "the two bounds must be indistinguishable — that is the finding"
    );
    assert_eq!(
        one.fires(),
        eight.fires(),
        "and they must produce the same number of fires: {} vs {}",
        one.fires(),
        eight.fires()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn even_the_ceiling_buys_nothing() {
    // The most generous bound the product will accept at all. If any value
    // were going to produce concurrency, it is this one.
    let probe = drive_overdue_interval(CEILING_IN_FLIGHT).await;
    assert!(probe.fires() > 0, "the scenario must actually fire");
    assert_eq!(
        probe.peak(),
        1,
        "`max_in_flight = CEILING_IN_FLIGHT` ({CEILING_IN_FLIGHT}) produced peak \
         concurrency {} — the ceiling constant bounds a quantity the runtime \
         does not produce",
        probe.peak()
    );
}
