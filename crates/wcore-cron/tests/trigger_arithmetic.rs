//! Grading tests for the `trigger.rs` mutants that survived the 2026-09-04
//! `mutants-nightly` run on `wcore-cron` (wayland-core#449).
//!
//! Eight of the 55 live survivors are in `trigger.rs`, and they are all
//! boundary arithmetic: an off-by-one in the one-shot's "has it passed yet"
//! test, a sign flip in the two arms that project the next fire, a spentness
//! guard that stops applying, a validator's minimum-length test, and the
//! deadline comparison in `heartbeat_state`. Every one of them changes WHEN a
//! background job fires, which is the property this vocabulary exists to bound.
//!
//! The existing suite misses them for one shared reason: it exercises each
//! trigger against its own `default_bound()`, and those defaults floor the
//! interval at 60s, so the floor — not the arithmetic under test — decides the
//! answer. `TriggerBound::new(1, 1)` is used throughout here so the projected
//! instant is the observable, not the floor.
//!
//! Mutants targeted (line numbers as reported by the run, in
//! `crates/wcore-cron/src/`):
//!
//! | mutant | killed by |
//! |---|---|
//! | `trigger.rs:285:57` replace `<` with `==` / `<=` | `a_webhook_path_must_have_something_after_the_slash` |
//! | `trigger.rs:332:24` replace `>` with `>=` | `a_one_shot_due_exactly_now_does_not_re_arm` |
//! | `trigger.rs:339:28` replace `+` with `-` | `an_interval_projects_forward_by_its_period` |
//! | `trigger.rs:359:28` replace `+` with `-` | `a_commitment_projects_forward_by_its_heartbeat` |
//! | `trigger.rs:370:24` replace match guard `bound.is_spent(t)` with `false` | `a_fire_that_lands_past_the_deadline_is_not_scheduled` |
//! | `trigger.rs:378:9` replace `is_clock_driven` with `false` | `the_clock_driven_variants_say_so` |
//! | `trigger.rs:453:12` replace `>` with `>=` | `a_commitment_is_not_expired_at_its_deadline` |

use chrono::{DateTime, Duration, TimeZone, Utc};
use wcore_cron::trigger::{HeartbeatState, Trigger, TriggerBound, heartbeat_state};

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap()
}

/// A bound that imposes the smallest legal floor, so the projected instant —
/// not the floor — is what the assertions read. Every variant's own
/// `default_bound()` floors at 60s or more, which is why the existing suite
/// cannot see the arithmetic these tests grade.
fn unfloored() -> TriggerBound {
    TriggerBound::new(1, 1)
}

/// Kills `trigger.rs:332:24` (`>` -> `>=`).
///
/// A one-shot whose instant is exactly now has already come due; re-arming it
/// is a duplicate fire of an action that was supposed to happen once.
#[test]
fn a_one_shot_due_exactly_now_does_not_re_arm() {
    let t = Trigger::Once { at: t0() };
    assert_eq!(
        t.next_after(t0(), &unfloored()).unwrap(),
        None,
        "a one-shot due at exactly the query instant is spent, not pending"
    );

    // Control: one second earlier it is genuinely still pending, so the `None`
    // above is the boundary and not a trigger that never schedules anything.
    assert_eq!(
        t.next_after(t0() - Duration::seconds(1), &unfloored())
            .unwrap(),
        Some(t0()),
        "control: before its instant the one-shot is still pending"
    );
}

/// Kills `trigger.rs:339:28` (`+` -> `-`).
///
/// With the sign flipped the projection lands in the past and the result
/// floor silently rewrites it to `after + 1s`, so the job fires on every tick
/// instead of on its period. Reading the exact instant is what makes that
/// visible; asserting only `next > after` would not.
#[test]
fn an_interval_projects_forward_by_its_period() {
    let t = Trigger::Interval { every_secs: 600 };
    assert_eq!(
        t.next_after(t0(), &unfloored()).unwrap(),
        Some(t0() + Duration::seconds(600)),
        "a 600s interval must next fire 600s out, not at the rate floor"
    );
}

/// Kills `trigger.rs:359:28` (`+` -> `-`).
///
/// Same failure as the interval arm, on the variant whose whole point is that
/// a stalled commitment is observable: a heartbeat projected into the past
/// collapses to the floor and beats every second.
#[test]
fn a_commitment_projects_forward_by_its_heartbeat() {
    let t = Trigger::Commitment {
        deadline: t0() + Duration::days(1),
        heartbeat_secs: 300,
    };
    assert_eq!(
        t.next_after(t0(), &unfloored()).unwrap(),
        Some(t0() + Duration::seconds(300)),
        "a 300s heartbeat must next beat 300s out, not at the rate floor"
    );
}

/// Kills `trigger.rs:370:24` (match guard `bound.is_spent(t)` -> `false`).
///
/// The bound is applied to the RESULT, not only to the parameters: a trigger
/// whose next fire lands after its terminal deadline must not be scheduled at
/// all. Dropping that guard is how a bounded background job keeps firing past
/// the instant it was supposed to stop.
#[test]
fn a_fire_that_lands_past_the_deadline_is_not_scheduled() {
    let t = Trigger::Interval { every_secs: 600 };

    // Control: the same trigger with room left before the deadline schedules
    // normally, so the `None` below is the guard and not a dead trigger.
    let roomy = unfloored().with_deadline(t0() + Duration::hours(1));
    assert_eq!(
        t.next_after(t0(), &roomy).unwrap(),
        Some(t0() + Duration::seconds(600)),
        "control: a fire inside the deadline is scheduled"
    );

    // The deadline falls between now and the projected fire. `is_spent(after)`
    // is false — the trigger is still live right now — so only the guard on
    // the RESULT can refuse it.
    let tight = unfloored().with_deadline(t0() + Duration::seconds(60));
    assert!(
        !tight.is_spent(t0()),
        "control: the bound is not already spent at the query instant"
    );
    assert_eq!(
        t.next_after(t0(), &tight).unwrap(),
        None,
        "a fire projected past the terminal deadline must not be scheduled"
    );
}

/// Kills `trigger.rs:378:9` (`is_clock_driven` -> `false`).
///
/// The existing suite only asserts the `false` side, on the externally driven
/// variants, so a constant `false` satisfies it. The property is a partition
/// and has to be read from both sides.
#[test]
fn the_clock_driven_variants_say_so() {
    for t in [
        Trigger::Once { at: t0() },
        Trigger::Interval { every_secs: 600 },
        Trigger::Cron {
            expression: "0 9 * * *".into(),
        },
        Trigger::Commitment {
            deadline: t0() + Duration::days(1),
            heartbeat_secs: 300,
        },
    ] {
        assert!(
            t.is_clock_driven(),
            "{} is projected from the clock and must say so",
            t.kind()
        );
    }

    // The other side of the partition, for the same reason.
    for t in [
        Trigger::Event { topic: "x".into() },
        Trigger::Webhook {
            path: "/x".into(),
            require_auth: true,
        },
        Trigger::Poll {
            url: "https://example.invalid/health".into(),
            every_secs: 600,
        },
    ] {
        assert!(
            !t.is_clock_driven(),
            "{} is driven from outside and must not claim a predictable fire",
            t.kind()
        );
    }
}

/// Kills both `trigger.rs:285:57` survivors (`<` -> `==` and `<` -> `<=`).
///
/// `path.len() < 2` is what refuses a bare `/`. `== 2` accepts the bare slash
/// and refuses every two-character path; `<= 2` refuses the shortest legal
/// path. Both boundaries have to be read to separate them.
#[test]
fn a_webhook_path_must_have_something_after_the_slash() {
    let webhook = |p: &str| Trigger::Webhook {
        path: p.into(),
        require_auth: true,
    };

    assert!(
        webhook("/").validate().is_err(),
        "a bare slash names no endpoint and must be refused"
    );
    assert!(
        webhook("/a").validate().is_ok(),
        "two characters is the shortest legal absolute path and must be accepted"
    );
    assert!(
        webhook("/hook").validate().is_ok(),
        "control: an ordinary path is accepted"
    );
    assert!(
        webhook("relative").validate().is_err(),
        "control: the path must still be absolute"
    );
}

/// Kills `trigger.rs:453:12` (`>` -> `>=`).
///
/// At exactly the deadline the commitment has not yet run out of time, and a
/// beat at that instant is alive. Reporting `Expired` there retires a
/// commitment one tick early and hides the beat that arrived on time.
#[test]
fn a_commitment_is_not_expired_at_its_deadline() {
    let t = Trigger::Commitment {
        deadline: t0(),
        heartbeat_secs: 60,
    };

    assert_eq!(
        heartbeat_state(&t, Some(t0()), t0()),
        Some(HeartbeatState::Alive),
        "a beat at exactly the deadline is on time, not expired"
    );

    // Control: one second later it really is expired, so the assertion above
    // is the boundary and not a state this function can never report.
    assert_eq!(
        heartbeat_state(&t, Some(t0()), t0() + Duration::seconds(1)),
        Some(HeartbeatState::Expired),
        "control: past the deadline the commitment is expired"
    );
}
