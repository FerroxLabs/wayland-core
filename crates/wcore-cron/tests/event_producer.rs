//! 24-C2 regression guards for the three trigger kinds that could never fire.
//!
//! Every guard here asserts an OBSERVABLE EFFECT, never a return code. The
//! handler under test recorded `Ok(())` for a dispatch that reached nothing at
//! all for most of this crate's life, so a test asserting `Ok` proves nothing
//! about whether a job ran. What is asserted is the set of targets the handler
//! actually received.
//!
//! Each guard states, in its own body, the single edit that reddens it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use wcore_cron::job::Target;
use wcore_cron::lease::LeaseHandle;
use wcore_cron::runner::{JobHandler, tick_once_at};
use wcore_cron::store::{CronStore, FileCronStore};
use wcore_cron::{CronJob, Trigger};

/// Records the targets it was actually asked to run.
#[derive(Default, Clone)]
struct Recording {
    seen: Arc<Mutex<Vec<Target>>>,
}

#[async_trait]
impl JobHandler for Recording {
    async fn dispatch(&self, target: &Target) -> wcore_cron::Result<()> {
        self.seen.lock().unwrap().push(target.clone());
        Ok(())
    }
}

impl Recording {
    fn fired(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|t| match t {
                Target::Slash { command } => command.clone(),
                Target::Skill { name, .. } => name.clone(),
                Target::Channel { channel_name, .. } => channel_name.clone(),
            })
            .collect()
    }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 28, 9, 0, 0).unwrap()
}

fn cron_dir(root: &Path) -> std::path::PathBuf {
    root.join("cron")
}

fn store_in(root: &Path) -> Arc<dyn CronStore> {
    Arc::new(FileCronStore::new(cron_dir(root).join("jobs.json")))
}

fn slash(cmd: &str) -> Target {
    Target::Slash {
        command: cmd.to_string(),
    }
}

/// Insert a job with the given trigger, anchored before `t0` so it is not
/// filtered by the "created after the event" rule.
async fn insert(store: &Arc<dyn CronStore>, trigger: Trigger, cmd: &str) -> CronJob {
    let mut job = CronJob::with_trigger(trigger, slash(cmd)).unwrap();
    job.created_at = t0() - Duration::hours(1);
    store.insert(job.clone()).await.unwrap();
    job
}

// ---------------------------------------------------------------------------
// event — the kind that now has a producer
// ---------------------------------------------------------------------------

/// THE guard for 24-C2's event leg. It drives the real tick and asserts the
/// job's TARGET REACHED THE HANDLER — not that a config parsed, not that a row
/// persisted, not that the tick returned `Ok`.
///
/// Reddens on: deleting the `drain_published_events` call from `tick_once_at`.
/// That is precisely the state this crate shipped in, and the whole suite was
/// green in it.
#[tokio::test]
async fn a_published_event_actually_fires_its_subscribed_job() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    insert(
        &store,
        Trigger::Event {
            topic: "build.finished".into(),
        },
        "/notify",
    )
    .await;

    // Nothing published yet: the tick must fire nothing. Without this half the
    // test could pass against a runner that fires event jobs unconditionally.
    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), t0())
        .await
        .unwrap();
    assert!(
        handler.fired().is_empty(),
        "an event job must not fire before its topic is published, got {:?}",
        handler.fired()
    );

    wcore_cron::publish_event(cron_dir(dir.path()), "build.finished", t0()).unwrap();

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
        vec!["/notify".to_string()],
        "publishing the topic must actually run the job"
    );
}

/// A published event is CONSUMED. Without this the queue would re-fire the same
/// event on every subsequent tick forever, which is the unbounded-firing threat
/// the trigger bounds exist to prevent.
#[tokio::test]
async fn a_drained_event_does_not_fire_again_on_the_next_tick() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    insert(&store, Trigger::Event { topic: "t".into() }, "/once").await;
    wcore_cron::publish_event(cron_dir(dir.path()), "t", t0()).unwrap();

    for minute in 1..=5 {
        tick_once_at(
            &store,
            &arc,
            None,
            &LeaseHandle::unleased(),
            t0() + Duration::minutes(minute),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        handler.fired().len(),
        1,
        "one publish is one fire across five ticks, got {:?}",
        handler.fired()
    );
}

/// Fan-out: one event fires EVERY subscriber, not the first one matched.
///
/// Reddens on: `break`ing out of the per-job loop after the first match, or
/// consuming the event inside it.
#[tokio::test]
async fn one_event_fires_every_subscribed_job_not_just_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    insert(&store, Trigger::Event { topic: "t".into() }, "/a").await;
    insert(&store, Trigger::Event { topic: "t".into() }, "/b").await;
    // A different topic must NOT be swept up.
    insert(&store, Trigger::Event { topic: "u".into() }, "/c").await;

    wcore_cron::publish_event(cron_dir(dir.path()), "t", t0()).unwrap();
    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::unleased(),
        t0() + Duration::seconds(30),
    )
    .await
    .unwrap();

    let mut fired = handler.fired();
    fired.sort();
    assert_eq!(
        fired,
        vec!["/a".to_string(), "/b".to_string()],
        "both subscribers to the published topic must fire, and only those"
    );
}

/// Topics match EXACTLY. A prefix rule would fire `build` on `build.finished`
/// and become a compatibility constraint the day it shipped.
#[tokio::test]
async fn a_topic_matches_exactly_and_not_by_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    insert(
        &store,
        Trigger::Event {
            topic: "build".into(),
        },
        "/prefix",
    )
    .await;
    wcore_cron::publish_event(cron_dir(dir.path()), "build.finished", t0()).unwrap();
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
        "\"build\" must not match \"build.finished\", got {:?}",
        handler.fired()
    );
}

/// A job created AFTER an event was published does not consume it. Otherwise
/// creating a subscriber immediately fires it against a backlog it was never
/// meant to see.
#[tokio::test]
async fn a_job_does_not_consume_an_event_published_before_it_existed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    wcore_cron::publish_event(cron_dir(dir.path()), "t", t0() - Duration::hours(2)).unwrap();

    let mut job =
        CronJob::with_trigger(Trigger::Event { topic: "t".into() }, slash("/late")).unwrap();
    job.created_at = t0(); // created after the publish
    store.insert(job).await.unwrap();

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
        "a subscriber must not inherit a backlog published before it existed, got {:?}",
        handler.fired()
    );
}

/// The event trigger's rate bound is ENFORCED at the drain, not merely stored.
/// A runaway publisher is the exact case the bound was written for.
///
/// Reddens on: deleting the `min_interval_secs` check in
/// `drain_published_events`.
#[tokio::test]
async fn a_burst_of_publishes_is_held_to_the_triggers_minimum_interval() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job =
        CronJob::with_trigger(Trigger::Event { topic: "t".into() }, slash("/hot")).unwrap();
    job.created_at = t0() - Duration::hours(1);
    // Narrower than the variant default is allowed; this states the floor the
    // job is held to explicitly rather than relying on the default staying 1s.
    job.bound = Some(wcore_cron::TriggerBound::new(60, 1));
    store.insert(job).await.unwrap();

    for _ in 0..5 {
        wcore_cron::publish_event(cron_dir(dir.path()), "t", t0()).unwrap();
    }
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
        handler.fired().len(),
        1,
        "five publishes inside a 60s floor must produce one fire, got {:?}",
        handler.fired()
    );

    // And the four it did NOT fire are still queued. A rate-held event that got
    // consumed anyway would be a published event silently never delivered —
    // the same defect as the one this whole file exists to close, moved one
    // layer down. Backpressure belongs at the publisher (`MAX_PENDING`), not in
    // a quiet drop here.
    assert_eq!(
        wcore_cron::events::pending(cron_dir(dir.path())).len(),
        4,
        "rate-held events must stay queued, not be discarded"
    );
}

/// The other half of the rate hold: a held event is DELIVERED once the floor
/// has passed. Without this, `a_burst_of_publishes...` would pass against a
/// runner that simply dropped everything.
#[tokio::test]
async fn a_rate_held_event_is_delivered_once_the_floor_has_passed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    let mut job =
        CronJob::with_trigger(Trigger::Event { topic: "t".into() }, slash("/held")).unwrap();
    job.created_at = t0() - Duration::hours(1);
    job.bound = Some(wcore_cron::TriggerBound::new(60, 1));
    store.insert(job).await.unwrap();

    wcore_cron::publish_event(cron_dir(dir.path()), "t", t0()).unwrap();
    wcore_cron::publish_event(cron_dir(dir.path()), "t", t0()).unwrap();

    tick_once_at(&store, &arc, None, &LeaseHandle::unleased(), t0())
        .await
        .unwrap();
    assert_eq!(handler.fired().len(), 1, "the floor holds the second");

    // Past the floor.
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
        handler.fired().len(),
        2,
        "the held event must eventually be delivered, not dropped"
    );
    assert!(
        wcore_cron::events::pending(cron_dir(dir.path())).is_empty(),
        "and the queue must then be empty"
    );
}

/// An observer — a process that does not own the schedule lease — drains
/// nothing. Two processes both draining one queue would double-fire every
/// event, which is the duplicate the lease exists to prevent.
#[tokio::test]
async fn an_observer_does_not_drain_the_event_queue() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    insert(&store, Trigger::Event { topic: "t".into() }, "/obs").await;
    wcore_cron::publish_event(cron_dir(dir.path()), "t", t0()).unwrap();

    tick_once_at(
        &store,
        &arc,
        None,
        &LeaseHandle::observer(),
        t0() + Duration::seconds(30),
    )
    .await
    .unwrap();

    assert!(
        handler.fired().is_empty(),
        "an observer must fire nothing, got {:?}",
        handler.fired()
    );
    // And the event is still queued for the real owner — an observer must not
    // silently consume work it refused to do.
    assert_eq!(
        wcore_cron::events::pending(cron_dir(dir.path())).len(),
        1,
        "the observer must leave the event for the owner"
    );
}

/// An event fire and a clock fire of the same job carry DIFFERENT delivery
/// identities, and two publishes in the same millisecond do too. Collapsing
/// them would make the delivery ledger drop the second as a duplicate — a
/// silent loss of a published event.
#[test]
fn two_events_in_one_millisecond_are_two_deliveries() {
    use wcore_cron::runner::FireContext;
    let at = t0();
    let clock = FireContext::scheduled("job", at);
    let a = FireContext::external("job", at, "event-a");
    let b = FireContext::external("job", at, "event-b");
    assert_ne!(a.delivery_id(), b.delivery_id());
    assert_ne!(a.delivery_id(), clock.delivery_id());
    // The clock key is byte-identical to the one already written in persisted
    // ledgers; changing it would make every pending delivery unrecognisable
    // across the upgrade.
    assert_eq!(
        clock.delivery_id(),
        format!("cron:job:{}", at.timestamp_millis())
    );
}

// ---------------------------------------------------------------------------
// webhook and poll — the kinds that have no producer and now say so
// ---------------------------------------------------------------------------

/// The measured 24-C2 defect for `poll`, closed.
///
/// `poll` used to be clock-driven, so the tick fired the job's action on a
/// timer having NEVER contacted the URL — measured at six fires in six ticks.
/// A trigger documented as "fires when the response says work is due" must not
/// fire when no response was ever obtained.
///
/// Reddens on: restoring `Self::Poll` to the clock-driven arm of `next_after`.
#[tokio::test]
async fn a_poll_job_never_fires_because_nothing_performs_the_poll() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let handler = Recording::default();
    let arc: Arc<dyn JobHandler> = Arc::new(handler.clone());

    insert(
        &store,
        Trigger::Poll {
            url: "https://status.test/health".into(),
            every_secs: 300,
        },
        "/poll",
    )
    .await;

    for hour in 1..=6 {
        tick_once_at(
            &store,
            &arc,
            None,
            &LeaseHandle::unleased(),
            t0() + Duration::hours(hour),
        )
        .await
        .unwrap();
    }
    assert!(
        handler.fired().is_empty(),
        "a poll job must not run its action on a timer without ever contacting the \
         remote — that is `every:` wearing a remote's name. Got {:?}",
        handler.fired()
    );
}

/// Every trigger kind is either driven by a producer or names itself
/// unreachable. Walks `Trigger::KINDS` so a kind cannot be added without
/// answering the question.
#[test]
fn every_kind_either_has_a_producer_or_says_why_not() {
    let all = [
        Trigger::Once { at: t0() },
        Trigger::Interval { every_secs: 900 },
        Trigger::Cron {
            expression: "0 9 * * *".into(),
        },
        Trigger::Event { topic: "t".into() },
        Trigger::Webhook {
            path: "/hooks/x".into(),
            require_auth: true,
        },
        Trigger::Poll {
            url: "https://x.test".into(),
            every_secs: 300,
        },
        Trigger::Commitment {
            deadline: t0() + Duration::hours(1),
            heartbeat_secs: 600,
        },
    ];
    assert_eq!(
        all.len(),
        Trigger::KINDS.len(),
        "a trigger kind exists with no producer answer"
    );
    for t in all {
        assert_eq!(
            t.has_producer(),
            t.no_producer_reason().is_none(),
            "{} must either have a producer or state why it has none",
            t.kind()
        );
        if let Some(reason) = t.no_producer_reason() {
            assert!(
                reason.contains("never fire"),
                "{}'s reason must say plainly that the job will never fire, got {reason:?}",
                t.kind()
            );
        }
    }
    // The specific two, named. If a producer lands for either, this line is
    // what forces the reason string to be removed with it.
    assert!(
        !Trigger::Webhook {
            path: "/x".into(),
            require_auth: true
        }
        .has_producer()
    );
    assert!(
        !Trigger::Poll {
            url: "https://x.test".into(),
            every_secs: 300
        }
        .has_producer()
    );
    assert!(Trigger::Event { topic: "t".into() }.has_producer());
}
