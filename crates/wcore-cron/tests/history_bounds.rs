//! The history bound, enforced under sustained firing through the real tick.
//!
//! Phase 24 plan 24-02, Task 2.
//!
//! The bound is asserted through the RUNNER, not only through the history
//! module's own helper. A cap that the helper honours but the runner never
//! calls is a documented cap, which is exactly the state this replaced:
//! "ring-buffered" was in the module documentation and the code appended
//! forever.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, TimeZone, Utc};
use wcore_cron::history::{DEFAULT_MAX_RECORDS, append_bounded, count, read_recent};
use wcore_cron::job::{CronFireOutcome, CronFireRecord, Target};
use wcore_cron::lease::LeaseHandle;
use wcore_cron::runner::JobHandler;
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::{CronJob, Result, tick_once_at};

struct Ok_;

#[async_trait]
impl JobHandler for Ok_ {
    async fn dispatch(&self, _t: &Target) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn sustained_firing_through_the_runner_stops_growing_the_history_file() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn CronStore> = Arc::new(FileCronStore::new(dir.path().join("jobs.json")));
    let history = dir.path().join("history.jsonl");
    let arc: Arc<dyn JobHandler> = Arc::new(Ok_);
    let t0 = Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap();

    // A minute schedule, ticked once per minute for well past the bound.
    let mut job = CronJob::new(
        "* * * * *",
        Target::Slash {
            command: "/tick".into(),
        },
    )
    .unwrap();
    job.created_at = t0 - Duration::minutes(1);
    store.insert(job).await.unwrap();

    let fires = DEFAULT_MAX_RECORDS + 250;
    for i in 0..fires {
        tick_once_at(
            &store,
            &arc,
            Some(&history),
            &LeaseHandle::unleased(),
            t0 + Duration::minutes(i as i64),
        )
        .await
        .unwrap();
    }

    let n = count(&history).unwrap();
    assert!(
        n > 0,
        "the sequence must actually have fired; a zero-record history would make the bound assertion vacuous"
    );
    assert!(
        n <= DEFAULT_MAX_RECORDS,
        "history grew past its bound under sustained firing: {n} > {DEFAULT_MAX_RECORDS}"
    );
    assert_eq!(
        n, DEFAULT_MAX_RECORDS,
        "with more fires than the bound the file must sit exactly at it"
    );
}

#[tokio::test]
async fn the_verb_still_returns_the_most_recent_records_after_the_file_stopped_growing() {
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.jsonl");
    let t0 = Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap();

    for i in 0..(DEFAULT_MAX_RECORDS + 40) {
        append_bounded(
            &history,
            &CronFireRecord {
                job_id: format!("job-{i:05}"),
                fired_at: t0 + Duration::seconds(i as i64),
                outcome: CronFireOutcome::Success { duration_ms: 1 },
            },
            DEFAULT_MAX_RECORDS,
        )
        .unwrap();
    }

    let (recent, skipped) = read_recent(&history, 20).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(recent.len(), 20);
    let last = DEFAULT_MAX_RECORDS + 39;
    assert_eq!(
        recent.last().unwrap().job_id,
        format!("job-{last:05}"),
        "the newest record must still be returned after trimming"
    );
}

#[tokio::test]
async fn trimming_never_drops_a_record_that_is_still_inside_the_bound() {
    // The complement of the growth assertion: a file UNDER the bound must be
    // left completely alone. A trim that fired unconditionally would silently
    // rewrite every history file on every append.
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.jsonl");
    let t0 = Utc.with_ymd_and_hms(2026, 7, 27, 0, 0, 0).unwrap();

    for i in 0..25 {
        append_bounded(
            &history,
            &CronFireRecord {
                job_id: format!("job-{i}"),
                fired_at: t0,
                outcome: CronFireOutcome::Success { duration_ms: 0 },
            },
            DEFAULT_MAX_RECORDS,
        )
        .unwrap();
    }
    assert_eq!(count(&history).unwrap(), 25);
    let (all, _) = read_recent(&history, 1000).unwrap();
    assert_eq!(all.first().unwrap().job_id, "job-0");
}
