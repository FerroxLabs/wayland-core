//! The two `CronRunner` teardown survivors (wayland-core#449).
//!
//! | mutant (`crates/wcore-cron/src/`) | killed by |
//! |---|---|
//! | `runner.rs:592:9` replace `CronRunner::shutdown` with `()` | `shutdown_waits_for_a_dispatch_already_in_flight` |
//! | `runner.rs:606:9` replace `<impl Drop for CronRunner>::drop` with `()` | `dropping_a_runner_stops_the_task_it_spawned` |
//!
//! Both survived because a runner's fields still drop when its own teardown
//! body is deleted, and the field drops do most of the same work: the lease is
//! surrendered either way, and the tick loop stops producing fires either way.
//! What the two bodies uniquely provide is `handle.await` in one and
//! `handle.abort()` in the other, so both tests are written against the TASK
//! rather than against the schedule.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use tokio::sync::Notify;
use wcore_cron::job::Target;
use wcore_cron::runner::{CronRunner, JobHandler, RecordingHandler};
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::{CronJob, Result};

fn due_job() -> CronJob {
    let mut job = CronJob::new(
        "0 9 * * *",
        Target::Slash {
            command: "/y".into(),
        },
    )
    .unwrap();
    // Two days back, so the cron anchor is long past and the first tick is due.
    job.created_at = Utc::now() - ChronoDuration::days(2);
    job
}

/// Announces that it has entered `dispatch`, then takes its time finishing.
struct Slow {
    entered: Arc<Notify>,
    completed: Arc<AtomicUsize>,
    work: Duration,
}

#[async_trait]
impl JobHandler for Slow {
    async fn dispatch(&self, _target: &Target) -> Result<()> {
        self.entered.notify_one();
        tokio::time::sleep(self.work).await;
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Kills `runner.rs:592:9` (`CronRunner::shutdown` -> `()`).
///
/// `shutdown` is documented as "signal shutdown and AWAIT task exit", and the
/// await is the whole of its value over just dropping the runner: `Drop` is the
/// safety net and it `abort()`s, which cancels a dispatch that is already on
/// the wire. Deleting the body leaves `self` to drop at the end of the call, so
/// the lease is still surrendered and the watch is still flipped — every
/// assertion about the SCHEDULE still passes. Only the in-flight fire tells the
/// two apart, and it is the thing a graceful shutdown exists to protect.
///
/// The handler announces that it has entered `dispatch` before it starts
/// working, so the shutdown below is guaranteed to land on a fire that is
/// genuinely in flight rather than on a lucky interleaving.
#[tokio::test]
async fn shutdown_waits_for_a_dispatch_already_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn CronStore> = Arc::new(FileCronStore::new(dir.path().join("jobs.json")));
    store.insert(due_job()).await.unwrap();

    let entered = Arc::new(Notify::new());
    let completed = Arc::new(AtomicUsize::new(0));
    let handler: Arc<dyn JobHandler> = Arc::new(Slow {
        entered: entered.clone(),
        completed: completed.clone(),
        work: Duration::from_millis(800),
    });

    let runner = CronRunner::spawn(store, handler, Duration::from_millis(25));

    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("control: the runner must have entered a dispatch to shut down over");
    assert_eq!(
        completed.load(Ordering::SeqCst),
        0,
        "control: the dispatch must still be in flight when shutdown is called"
    );

    runner.shutdown().await;

    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "a graceful shutdown must let a fire already on the wire finish; \
         returning before it does is the abort path wearing shutdown's name, \
         and it cancels a job the schedule has already committed to running"
    );
}

/// Kills `runner.rs:606:9` (`<impl Drop for CronRunner>::drop` -> `()`).
///
/// Deleting the body does NOT leave the task running visibly: the watch sender
/// is a field, so it drops anyway, and the tick loop's `rx.changed()` then
/// resolves `Err` forever — the task stops firing without ever stopping. So
/// "no more jobs ran" is true of both the real teardown and the mutant, and
/// every schedule-shaped assertion passes.
///
/// `handle.abort()` is what actually ends it, and the observable consequence is
/// that the task releases the handler it was spawned with. A live task holds
/// its `Arc<dyn JobHandler>` forever, which is the engine outliving its own
/// shutdown — the exact thing this `Drop` says it is the safety net for.
///
/// `multi_thread` deliberately: under the mutant the orphaned task spins, and a
/// current-thread runtime would starve this test into a timeout instead of a
/// failure. A timeout is not a kill.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_runner_stops_the_task_it_spawned() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn CronStore> = Arc::new(FileCronStore::new(dir.path().join("jobs.json")));
    store.insert(due_job()).await.unwrap();

    let handler: Arc<dyn JobHandler> = Arc::new(RecordingHandler::new());
    assert_eq!(
        Arc::strong_count(&handler),
        1,
        "control: nothing else holds the handler yet"
    );

    // A long tick, so the task is parked in its select rather than mid-fire.
    let runner = CronRunner::spawn(store, handler.clone(), Duration::from_secs(30));
    assert_eq!(
        Arc::strong_count(&handler),
        2,
        "control: the spawned task must actually hold the handler, or the \
         assertion below could never distinguish anything"
    );

    drop(runner);

    // Cancellation is delivered asynchronously, so wait for it rather than
    // assuming an interleaving. Bounded, so the mutant fails rather than hangs.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && Arc::strong_count(&handler) > 1 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(
        Arc::strong_count(&handler),
        1,
        "dropping the runner must end the task it spawned; a task that merely \
         stops firing still holds the engine's handler and keeps running after \
         the engine that owns it is gone"
    );
}
