//! Boundary guards for the event-drain loop in `runner::drain_published_events`
//! (wayland-core#449).
//!
//! Every survivor graded here is an OFF-BY-ONE or a negated flag on the edge of
//! a comparison, which is exactly the class an integration suite built from
//! round numbers cannot see: the existing event tests publish at one instant
//! and tick at another, where `>` and `>=` agree. Each test here puts the two
//! operands on the SAME instant, or on the exact retry state, so the edge is
//! the only thing deciding the answer.
//!
//! | mutant (`crates/wcore-cron/src/`) | killed by |
//! |---|---|
//! | `runner.rs:1068:31` replace `>` with `>=` | `an_event_published_the_instant_a_job_was_created_is_consumed_by_it` |
//! | `runner.rs:1086:31` replace `<` with `<=` | `an_event_exactly_one_interval_after_the_last_fire_is_not_held` |
//! | `runner.rs:1101:20` delete `!` | `an_event_no_live_subscriber_can_take_is_not_queued_forever` |

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use wcore_cron::job::Target;
use wcore_cron::lease::LeaseHandle;
use wcore_cron::runner::{JobHandler, tick_once_at};
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::{CronJob, Trigger, TriggerBound};

#[derive(Default, Clone)]
struct Recording {
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl JobHandler for Recording {
    async fn dispatch(&self, target: &Target) -> wcore_cron::Result<()> {
        if let Target::Slash { command } = target {
            self.seen.lock().unwrap().push(command.clone());
        }
        Ok(())
    }
}

impl Recording {
    fn fired(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 28, 9, 0, 0).unwrap()
}

fn cron_dir(root: &Path) -> PathBuf {
    root.join("cron")
}

fn store_in(root: &Path) -> Arc<dyn CronStore> {
    Arc::new(FileCronStore::new(cron_dir(root).join("jobs.json")))
}

fn event_job(topic: &str, cmd: &str) -> CronJob {
    CronJob::with_trigger(
        Trigger::Event {
            topic: topic.into(),
        },
        Target::Slash {
            command: cmd.into(),
        },
    )
    .unwrap()
}

fn queued(root: &Path) -> usize {
    wcore_cron::events::pending(cron_dir(root)).len()
}

/// Kills `runner.rs:1068:31` (`>` -> `>=` in the "published before this job
/// existed" filter).
///
/// The filter exists so a job created today does not inherit a backlog
/// published yesterday. Its boundary is the SAME instant: an event published
/// at the very moment the job was created is not a backlog item, it is the
/// first event the job is entitled to. Turning `>` into `>=` makes a
/// subscriber deaf to exactly one instant — the one a caller is most likely to
/// hit, because a job created by an automation and an event published by the
/// same automation share a clock reading.
///
/// The existing event tests anchor `created_at` an hour before the event, where
/// `>` and `>=` cannot be told apart. Here the two are equal.
#[tokio::test]
async fn an_event_published_the_instant_a_job_was_created_is_consumed_by_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job = event_job("deploy.done", "/announce");
    job.created_at = t0();
    store.insert(job).await.unwrap();

    wcore_cron::publish_event(cron_dir(dir.path()), "deploy.done", t0()).unwrap();

    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(30),
    )
    .await
    .unwrap();

    assert_eq!(
        handler.fired(),
        vec!["/announce".to_string()],
        "an event published at the same instant the job was created is not a \
         backlog item; the filter is strictly 'published BEFORE this job existed'"
    );

    // Control: an event published strictly before the job was created is still
    // filtered out. Without this the assertion above would also be satisfied by
    // deleting the filter entirely.
    let dir2 = tempfile::tempdir().unwrap();
    let store2 = store_in(dir2.path());
    let handler2 = Recording::default();
    let arc2: Arc<dyn JobHandler> = Arc::new(handler2.clone());
    let mut later = event_job("deploy.done", "/announce");
    later.created_at = t0() + Duration::seconds(1);
    store2.insert(later).await.unwrap();
    wcore_cron::publish_event(cron_dir(dir2.path()), "deploy.done", t0()).unwrap();
    tick_once_at(
        &store2,
        &arc2,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(30),
    )
    .await
    .unwrap();
    assert!(
        handler2.fired().is_empty(),
        "control: an event published before the job existed must still be \
         filtered, got {:?}",
        handler2.fired()
    );
}

/// Kills `runner.rs:1086:31` (`<` -> `<=` in the rate bound).
///
/// `min_interval_secs` is the SMALLEST PERMITTED GAP between two fires, so a
/// gap of exactly that many seconds is permitted — that is what "minimum"
/// means, and it is the gap a periodic publisher produces on every single
/// cycle. `<=` holds that publisher's event on every cycle instead, and a held
/// event is not dropped: it stays queued and is retried next tick, so the
/// symptom is a subscriber that is permanently one tick late rather than one
/// that visibly fails.
#[tokio::test]
async fn an_event_exactly_one_interval_after_the_last_fire_is_not_held() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let now = t0() + Duration::seconds(600);
    let mut job = event_job("tick.minute", "/rate");
    job.created_at = t0();
    job.bound = Some(TriggerBound::new(120, 2));
    // The previous fire is EXACTLY one minimum interval back.
    job.last_fired = Some(now - Duration::seconds(120));
    store.insert(job.clone()).await.unwrap();

    // Control: the bound really is 120s after clamping, or "exactly one
    // interval" below would be measured against a different number.
    assert_eq!(
        job.effective_bound().min_interval_secs,
        120,
        "control: the effective minimum interval must be the 120s under test"
    );

    wcore_cron::publish_event(cron_dir(dir.path()), "tick.minute", now).unwrap();
    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), now)
        .await
        .unwrap();

    assert_eq!(
        handler.fired(),
        vec!["/rate".to_string()],
        "a gap of exactly the minimum interval satisfies the minimum; holding \
         it makes every periodic publisher permanently one tick late"
    );
    assert_eq!(
        queued(dir.path()),
        0,
        "a fired event must be consumed, not left queued"
    );
}

/// Kills `runner.rs:1101:20` (`delete !` in the defer decision).
///
/// The two retry states are deliberately treated differently. A job inside its
/// backoff WILL attempt again, so the event is worth holding in the queue for
/// it. A job that has GIVEN UP will not attempt again until a human edits it,
/// so holding the event for it means the queue never drains — and because the
/// queue has a hard `MAX_PENDING` cap, a permanently undrainable event
/// eventually refuses every later publish on that schedule. Dropping the `!`
/// swaps the two, which is the wrong one to wait for in both directions.
#[tokio::test]
async fn an_event_no_live_subscriber_can_take_is_not_queued_forever() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job = event_job("nightly.report", "/report");
    job.created_at = t0();
    job.retry_state.attempts = 10;
    job.retry_state.gave_up = true;
    store.insert(job).await.unwrap();

    wcore_cron::publish_event(cron_dir(dir.path()), "nightly.report", t0()).unwrap();
    assert_eq!(
        queued(dir.path()),
        1,
        "control: the publish must have queued"
    );

    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(30),
    )
    .await
    .unwrap();

    assert!(
        handler.fired().is_empty(),
        "control: a given-up job must not be dispatched, got {:?}",
        handler.fired()
    );
    assert_eq!(
        queued(dir.path()),
        0,
        "no live subscriber can ever take this event, so it must be consumed \
         rather than held; a permanently undrainable event fills MAX_PENDING \
         and refuses every later publish on this schedule"
    );
}

/// The other half of the same branch, and the control that stops the test
/// above from passing against a runner that simply consumes everything: a job
/// merely INSIDE its backoff will attempt again, so its event stays queued.
#[tokio::test]
async fn an_event_whose_subscriber_is_only_backing_off_stays_queued() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job = event_job("nightly.report", "/report");
    job.created_at = t0();
    job.retry_state.attempts = 1;
    job.retry_state.gave_up = false;
    job.retry_state.not_before = Some(t0() + Duration::hours(1));
    store.insert(job).await.unwrap();

    wcore_cron::publish_event(cron_dir(dir.path()), "nightly.report", t0()).unwrap();
    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(30),
    )
    .await
    .unwrap();

    assert!(handler.fired().is_empty());
    assert_eq!(
        queued(dir.path()),
        1,
        "a subscriber that will attempt again must keep its event queued, or \
         the backoff silently loses the trigger it was backing off from"
    );
}
