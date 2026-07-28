//! The trigger matrix: one case per type, every one driven through an
//! injected instant rather than a sleep.
//!
//! Phase 24 plan 24-02, Task 2.
//!
//! Backwards compatibility is proved here rather than assumed: the last case
//! loads a job written in the historical on-disk shape — `schedule` instead of
//! `expression`, `type` instead of `kind`, no trigger, no bound, no retry
//! state — and asserts it still fires.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use wcore_cron::history::read_recent;
use wcore_cron::job::{CronFireOutcome, Target};
use wcore_cron::lease::LeaseHandle;
use wcore_cron::retry::RetryPolicy;
use wcore_cron::runner::JobHandler;
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::trigger::{HeartbeatState, Trigger, TriggerBound};
use wcore_cron::{CronError, CronJob, Result, tick_once_at};

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap()
}

#[derive(Default, Clone)]
struct Counting {
    seen: Arc<tokio::sync::Mutex<Vec<Target>>>,
}

impl Counting {
    async fn count(&self) -> usize {
        self.seen.lock().await.len()
    }
}

#[async_trait]
impl JobHandler for Counting {
    async fn dispatch(&self, t: &Target) -> Result<()> {
        self.seen.lock().await.push(t.clone());
        Ok(())
    }
}

struct AlwaysFails;

#[async_trait]
impl JobHandler for AlwaysFails {
    async fn dispatch(&self, _t: &Target) -> Result<()> {
        Err(CronError::Dispatch("destination refused".into()))
    }
}

fn store_in(dir: &std::path::Path) -> Arc<dyn CronStore> {
    Arc::new(FileCronStore::new(dir.join("jobs.json")))
}

fn slash(cmd: &str) -> Target {
    Target::Slash {
        command: cmd.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Every kind resolves, bounds itself, and reports honestly
// ---------------------------------------------------------------------------

#[test]
fn every_trigger_kind_has_a_case_in_this_file() {
    // A new variant added without extending this file would otherwise ship
    // with no matrix case at all. The list below is the file's own inventory.
    let covered = [
        "once",
        "interval",
        "cron",
        "event",
        "webhook",
        "poll",
        "commitment",
    ];
    for k in Trigger::KINDS {
        assert!(
            covered.contains(k),
            "trigger kind {k:?} has no case in trigger_matrix.rs"
        );
    }
    assert_eq!(covered.len(), Trigger::KINDS.len());
}

#[tokio::test]
async fn a_one_shot_fires_once_and_is_then_spent() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job = CronJob::with_trigger(
        Trigger::Once {
            at: t0() + Duration::minutes(5),
        },
        slash("/once"),
    )
    .unwrap();
    job.created_at = t0();
    store.insert(job.clone()).await.unwrap();

    // Before its instant: nothing.
    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), t0())
        .await
        .unwrap();
    assert_eq!(handler.count().await, 0);

    // After its instant: exactly one fire.
    let after = t0() + Duration::minutes(6);
    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), after)
        .await
        .unwrap();
    assert_eq!(handler.count().await, 1);

    // And never again, however far the clock is advanced.
    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::days(400),
    )
    .await
    .unwrap();
    assert_eq!(handler.count().await, 1, "a one-shot must not re-arm, ever");
}

#[tokio::test]
async fn an_interval_fires_no_faster_than_its_bound() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    // Asks for five seconds. The interval variant's default bound floors it at
    // sixty, so it must NOT fire twelve times in a minute.
    let mut job =
        CronJob::with_trigger(Trigger::Interval { every_secs: 5 }, slash("/fast")).unwrap();
    job.created_at = t0();
    store.insert(job).await.unwrap();

    for s in 1..=59 {
        tick_once_at(
            &store,
            &arc,
            None,
            &LeaseHandle::unleased(),
            t0() + Duration::seconds(s),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        handler.count().await,
        0,
        "a job asking for 5s must be held to its 60s bound"
    );

    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(61),
    )
    .await
    .unwrap();
    assert_eq!(handler.count().await, 1);
}

#[tokio::test]
async fn a_cron_trigger_behaves_exactly_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job = CronJob::new("0 9 * * *", slash("/daily")).unwrap();
    job.created_at = t0() - Duration::days(2);
    store.insert(job).await.unwrap();

    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), t0())
        .await
        .unwrap();
    assert_eq!(handler.count().await, 1);
    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), t0())
        .await
        .unwrap();
    assert_eq!(handler.count().await, 1, "one fire per due window");
}

#[tokio::test]
async fn externally_driven_triggers_never_fire_from_the_clock_alone() {
    // Event and webhook are driven from outside. The tick must not invent a
    // fire for them, because a schedule that guesses when a remote will call
    // is a schedule that fires work nobody asked for.
    for trigger in [
        Trigger::Event {
            topic: "build.finished".into(),
        },
        Trigger::Webhook {
            path: "/hooks/build".into(),
            require_auth: true,
        },
        // 24-C2: `poll` joined this set on measured evidence. It used to be
        // clock-driven and fired its target on a timer having never contacted
        // the URL — six fires in six ticks against a remote that was never
        // asked.
        Trigger::Poll {
            url: "https://status.test/health".into(),
            every_secs: 300,
        },
    ] {
        let kind = trigger.kind();
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let handler = Counting::default();
        let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

        let mut job = CronJob::with_trigger(trigger, slash("/external")).unwrap();
        job.created_at = t0();
        store.insert(job).await.unwrap();

        tick_once_at(
            &store,
            &arc,
            None,
            &LeaseHandle::unleased(),
            t0() + Duration::days(30),
        )
        .await
        .unwrap();
        assert_eq!(
            handler.count().await,
            0,
            "{kind} must not be fired by the clock"
        );
    }
}

#[tokio::test]
async fn a_webhook_records_whether_it_is_open() {
    // The auth requirement is stored ON THE JOB so `cron list` can show it.
    // A deliberately open endpoint has to say so in its own record.
    let closed = CronJob::with_trigger(
        Trigger::Webhook {
            path: "/hooks/x".into(),
            require_auth: true,
        },
        slash("/x"),
    )
    .unwrap();
    assert!(closed.expression.contains("auth"));
    let open = CronJob::with_trigger(
        Trigger::Webhook {
            path: "/hooks/x".into(),
            require_auth: false,
        },
        slash("/x"),
    )
    .unwrap();
    assert!(
        open.expression.contains("OPEN"),
        "an unauthenticated endpoint must be visible in the operator-facing descriptor, got {:?}",
        open.expression
    );
}

/// 24-C2 re-targeted from `poll` to `interval`.
///
/// This test proved that a variant's rate FLOOR is applied to the computed
/// next-fire and not only to the stored parameter. It used to drive that
/// through `Trigger::Poll` — but `poll` has no producer, nothing has ever
/// performed its HTTP request, and it is no longer fired by the clock at all
/// (see `a_poll_job_never_fires_because_nothing_performs_the_poll`). Left on
/// `poll`, both halves of this test would now read zero and it would pass
/// whatever the flooring code did: a self-passing gate.
///
/// `interval` is genuinely clock-driven, so the property is still proven and
/// the test can still fail. Reddens on: removing the `.max(60)` from
/// `Interval`'s `default_bound`, or the `earliest` floor in `next_after`.
#[tokio::test]
async fn an_interval_is_floored_at_a_minute_however_fast_it_asks() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job =
        CronJob::with_trigger(Trigger::Interval { every_secs: 1 }, slash("/fast")).unwrap();
    job.created_at = t0();
    store.insert(job).await.unwrap();

    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(59),
    )
    .await
    .unwrap();
    assert_eq!(
        handler.count().await,
        0,
        "a job must not run faster than its variant's floor"
    );

    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(61),
    )
    .await
    .unwrap();
    assert_eq!(
        handler.count().await,
        1,
        "and it must still fire once the floor has passed — without this half \
         the test passes against a runner that fires nothing at all"
    );
}

#[tokio::test]
async fn a_commitment_beats_then_misses_then_expires() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let deadline = t0() + Duration::hours(2);
    let mut job = CronJob::with_trigger(
        Trigger::Commitment {
            deadline,
            heartbeat_secs: 600,
        },
        slash("/commit"),
    )
    .unwrap();
    job.created_at = t0();
    store.insert(job.clone()).await.unwrap();

    // Never beat yet: missed, not alive. A commitment that has said nothing is
    // not healthy by default.
    let listed = store.list().await.unwrap();
    assert_eq!(
        listed[0].heartbeat_state(t0()),
        Some(HeartbeatState::Missed)
    );

    // A fire is a beat.
    let beat_at = t0() + Duration::minutes(11);
    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), beat_at)
        .await
        .unwrap();
    assert_eq!(handler.count().await, 1);
    let listed = store.list().await.unwrap();
    assert_eq!(
        listed[0].heartbeat_state(beat_at),
        Some(HeartbeatState::Alive),
        "a fresh beat must read alive"
    );
    assert_eq!(
        listed[0].heartbeat_state(beat_at + Duration::minutes(30)),
        Some(HeartbeatState::Missed),
        "a beat older than the interval must read missed, not alive"
    );

    // Past the deadline the commitment is terminal and stops firing.
    let count_before = handler.count().await;
    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        deadline + Duration::hours(1),
    )
    .await
    .unwrap();
    assert_eq!(
        handler.count().await,
        count_before,
        "a commitment past its deadline must reach a terminal state, not retry forever"
    );
    let listed = store.list().await.unwrap();
    assert_eq!(
        listed[0].heartbeat_state(deadline + Duration::hours(1)),
        Some(HeartbeatState::Expired)
    );
}

// ---------------------------------------------------------------------------
// Bounds cannot be widened from disk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_hand_edited_bound_cannot_make_a_job_fire_faster() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    // 24-C2: re-targeted from `poll` to `interval` for the same reason as
    // `an_interval_is_floored_at_a_minute_however_fast_it_asks` — a `poll` job
    // no longer fires from the clock under any bound, so this test would read
    // zero regardless of whether the clamp worked, and would prove nothing.
    let mut job =
        CronJob::with_trigger(Trigger::Interval { every_secs: 3600 }, slash("/hourly")).unwrap();
    job.created_at = t0();
    // The shape a hand-edited jobs.json takes: a one-second rate and a large
    // in-flight allowance.
    job.bound = Some(TriggerBound::new(1, 1000));
    store.insert(job).await.unwrap();

    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(60),
    )
    .await
    .unwrap();
    assert_eq!(
        handler.count().await,
        0,
        "a stored bound must not be able to widen the variant's default"
    );

    // The other half, which is what keeps this from passing against a runner
    // that has simply stopped firing: past its OWN period it must still fire.
    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(3601),
    )
    .await
    .unwrap();
    assert_eq!(
        handler.count().await,
        1,
        "the clamp narrows the rate, it does not disable the job"
    );
}

// ---------------------------------------------------------------------------
// Bounded retry reaches a recorded terminal give-up
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_failing_target_gives_up_inside_its_cap_and_the_give_up_is_in_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let history = dir.path().join("history.jsonl");
    let arc: Arc<dyn JobHandler> = Arc::new(AlwaysFails);

    let mut job = CronJob::new("* * * * *", slash("/doomed")).unwrap();
    job.created_at = t0() - Duration::hours(1);
    job.retry = Some(RetryPolicy {
        max_attempts: 3,
        base_backoff_secs: 60,
        max_backoff_secs: 60,
    });
    store.insert(job.clone()).await.unwrap();

    // Three attempts, each after its backoff window has passed.
    let mut now = t0();
    for _ in 0..3 {
        tick_once_at(&store, &arc, Some(&history), &LeaseHandle::unleased(), now)
            .await
            .unwrap();
        now += Duration::minutes(5);
    }

    let listed = store.list().await.unwrap();
    let after = listed.iter().find(|j| j.id == job.id).unwrap();
    match &after.last_result {
        Some(CronFireOutcome::GaveUp { attempts, .. }) => assert_eq!(*attempts, 3),
        other => panic!("expected a terminal GaveUp after the cap, got {other:?}"),
    }

    let (recs, skipped) = read_recent(&history, 100).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(
        recs.len(),
        3,
        "each attempt is recorded, and the give-up is one of them"
    );
    assert!(
        matches!(recs.last().unwrap().outcome, CronFireOutcome::GaveUp { .. }),
        "the terminal outcome must be visible in history, not merely in a log"
    );

    // And it genuinely stops: a hundred further ticks add nothing.
    let before = read_recent(&history, 500).unwrap().0.len();
    for i in 0..100 {
        tick_once_at(
            &store,
            &arc,
            Some(&history),
            &LeaseHandle::unleased(),
            now + Duration::minutes(i),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        read_recent(&history, 500).unwrap().0.len(),
        before,
        "a job that gave up must stop consuming attempts, not merely slow down"
    );
}

#[tokio::test]
async fn the_backoff_actually_holds_a_failing_job_off_between_ticks() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let history = dir.path().join("history.jsonl");
    let arc: Arc<dyn JobHandler> = Arc::new(AlwaysFails);

    let mut job = CronJob::new("* * * * *", slash("/doomed")).unwrap();
    job.created_at = t0() - Duration::hours(1);
    job.retry = Some(RetryPolicy {
        max_attempts: 5,
        base_backoff_secs: 600,
        max_backoff_secs: 600,
    });
    store.insert(job).await.unwrap();

    // First attempt, then nineteen ticks STRICTLY INSIDE the ten-minute
    // backoff. The nineteenth lands at t0+570s; t0+600s is the boundary and
    // belongs to the second half of this test, because a window that also
    // refused the attempt at its own expiry would be a backoff that never
    // ends.
    tick_once_at(&store, &arc, Some(&history), &LeaseHandle::unleased(), t0())
        .await
        .unwrap();
    for i in 1..=19 {
        tick_once_at(
            &store,
            &arc,
            Some(&history),
            &LeaseHandle::unleased(),
            t0() + Duration::seconds(i * 30),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        read_recent(&history, 100).unwrap().0.len(),
        1,
        "before the backoff was bounded this fired on every single tick"
    );

    // At the boundary it DOES retry. Without this half the assertion above
    // would also pass against a job that had simply stopped forever, which is
    // a different bug wearing the same green.
    tick_once_at(
        &store,
        &arc,
        Some(&history),
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(600),
    )
    .await
    .unwrap();
    assert_eq!(
        read_recent(&history, 100).unwrap().0.len(),
        2,
        "the backoff must expire and admit the next attempt, not silence the job"
    );
}

// ---------------------------------------------------------------------------
// The historical on-disk shape still loads and still fires
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_job_written_in_the_historical_shape_still_loads_and_fires() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.json");

    // `schedule` not `expression`; `type` not `kind`; no trigger, no bound, no
    // retry policy, no retry state, no heartbeat. Exactly what is already on
    // operators' disks.
    let raw = serde_json::json!({
        "jobs": [{
            "id": "legacy-0000-0000-0000-000000000001",
            "schedule": "0 9 * * *",
            "target": { "type": "slash", "command": "/legacy" },
            "enabled": true,
            "created_at": "2026-07-25T00:00:00Z",
            "last_fired": null
        }]
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    let store = FileCronStore::new(path.clone());
    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 1, "the historical shape must still parse");
    let legacy = &listed[0];
    assert_eq!(legacy.expression, "0 9 * * *");
    assert!(
        legacy.trigger.is_none(),
        "no trigger is stored on an old job"
    );
    assert!(matches!(legacy.effective_trigger(), Trigger::Cron { .. }));

    // It fires exactly as it did before the vocabulary existed. Re-inserting
    // through the engine stamps the integrity tag `list_for_run` requires;
    // that gate is M-19's and is unchanged by this plan.
    store.insert(legacy.clone()).await.unwrap();
    let store: Arc<dyn CronStore> = Arc::new(store);
    let handler = Counting::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());
    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), t0())
        .await
        .unwrap();
    assert_eq!(handler.count().await, 1);
}

#[tokio::test]
async fn a_new_job_round_trips_through_disk_with_its_trigger_intact() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileCronStore::new(dir.path().join("jobs.json"));
    let job = CronJob::with_trigger(
        Trigger::Commitment {
            deadline: t0() + Duration::days(1),
            heartbeat_secs: 900,
        },
        slash("/c"),
    )
    .unwrap();
    store.insert(job.clone()).await.unwrap();
    let back = store.list().await.unwrap();
    assert_eq!(back[0], job, "a trigger must survive a disk round trip");
}
