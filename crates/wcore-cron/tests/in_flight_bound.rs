//! What `TriggerBound::max_in_flight` actually does. Measured, not read.
//!
//! Phase 24 Criterion 2, lane `24-c4-support`. `24-PHASE-VERDICT.md` §3 records
//! the field as *"stored and clamped but not enforced at dispatch"*. Two
//! things are true and neither is quite that, so both are asserted here.
//!
//! # 1. The value is unreachable above 2 — and above 1 on six of seven variants
//!
//! `CronJob::effective_bound()` clamps any persisted bound to the trigger
//! variant's `default_bound()`, one-way. Every variant's default is
//! `max_in_flight = 1` **except `Event`, which is 2**. A job carrying a
//! persisted `max_in_flight` of `CEILING_IN_FLIGHT` (16) therefore has an
//! EFFECTIVE bound of 1 on `once`/`interval`/`cron`/`webhook`/`poll`/
//! `commitment`, and 2 on `event`.
//!
//! **`CEILING_IN_FLIGHT = 16` is decorative**: it bounds a value that every
//! variant default already bounds at 2 or below, so no input reaches it.
//!
//! This lane's first version of this file did not know that. It set a persisted
//! bound of 8 on an `interval` job, never looked at the effective bound, and
//! reported the resulting "peak concurrency 1" as a measurement of what a bound
//! of 8 buys. It was a measurement of a bound of 1. The mistake was caught by
//! driving the real `cron status` verb, which printed `max_in_flight=1` for the
//! job the test believed carried 8 — **a live drive correcting a green test**.
//! `the_effective_bound_can_never_exceed_two` is the replacement, and it
//! asserts the clamp rather than trusting the value it just wrote.
//!
//! # 2. The runner never reads the field
//!
//! Asserted as a source census over `runner.rs` rather than as a concurrency
//! measurement, and deliberately so. `dispatch_and_record` is `.await`ed inline
//! in the selection loop, so a probe driving that API would observe peak
//! concurrency 1 no matter what the code said — a tautology wearing a
//! measurement's clothes. What is actually claimed is narrower and checkable:
//! the runner enforces `deadline` and `min_interval_secs` and does not
//! reference `max_in_flight` at all. The census carries a known-positive
//! control on the sibling field that IS enforced, so a census that matched
//! nothing cannot pass.
//!
//! # Why this is a finding and not a shrug
//!
//! `cron status` renders `max_in_flight=2` on every event job. An operator
//! reading it is told two fires of that job may be outstanding. One may. Same
//! shape as the `poll:` trigger this phase already retired: a surface stating
//! behaviour the runtime does not implement. `crates/wcore-cli/src/cron.rs`
//! now annotates the line instead of silently echoing it.

use chrono::{DateTime, Duration, TimeZone, Utc};
use wcore_cron::CronJob;
use wcore_cron::job::Target;
use wcore_cron::trigger::{CEILING_IN_FLIGHT, Trigger, TriggerBound};

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap()
}

fn slash(cmd: &str) -> Target {
    Target::Slash {
        command: cmd.to_string(),
    }
}

/// Every variant, constructed. Kept exhaustive against `Trigger::KINDS` by
/// `every_kind_is_covered` below, so a new variant cannot slip past this file.
fn all_triggers() -> Vec<Trigger> {
    vec![
        Trigger::Once { at: t0() },
        Trigger::Interval { every_secs: 900 },
        Trigger::Cron {
            expression: "0 9 * * *".to_string(),
        },
        Trigger::Event {
            topic: "build.finished".to_string(),
        },
        Trigger::Webhook {
            path: "/hook".to_string(),
            require_auth: true,
        },
        Trigger::Poll {
            url: "https://example.invalid/x".to_string(),
            every_secs: 600,
        },
        Trigger::Commitment {
            deadline: t0() + Duration::hours(1),
            heartbeat_secs: 300,
        },
    ]
}

#[test]
fn every_kind_is_covered() {
    let covered: Vec<&str> = all_triggers().iter().map(|t| t.kind()).collect();
    for k in Trigger::KINDS {
        assert!(
            covered.contains(k),
            "trigger kind {k:?} is not covered here"
        );
    }
    assert_eq!(covered.len(), Trigger::KINDS.len());
}

// ---------------------------------------------------------------------------
// 1. Reachability of the value
// ---------------------------------------------------------------------------

#[test]
fn the_effective_bound_can_never_exceed_two() {
    for trigger in all_triggers() {
        let kind = trigger.kind();
        let mut job = CronJob::with_trigger(trigger, slash("/probe")).unwrap();

        // POSITIVE CONTROL: the persisted value really is the ceiling, so a
        // clamped result below is the clamp acting rather than a value never
        // written. This is the exact control whose absence produced this
        // file's first wrong result.
        job.bound = Some(TriggerBound::new(1, CEILING_IN_FLIGHT));
        assert_eq!(
            job.bound.as_ref().unwrap().max_in_flight,
            CEILING_IN_FLIGHT,
            "{kind}: the persisted bound must really carry the ceiling"
        );

        let effective = job.effective_bound().max_in_flight;
        assert!(
            effective <= 2,
            "{kind}: a persisted {CEILING_IN_FLIGHT} produced an EFFECTIVE \
             bound of {effective}; no variant default permits more than 2"
        );
    }
}

#[test]
fn only_event_permits_more_than_one_and_only_two() {
    let mut above_one: Vec<(&str, u32)> = Vec::new();
    for trigger in all_triggers() {
        let n = trigger.default_bound().max_in_flight;
        if n > 1 {
            above_one.push((trigger.kind(), n));
        }
    }
    assert_eq!(
        above_one,
        vec![("event", 2)],
        "exactly one variant is expected to permit concurrency, and only two: \
         got {above_one:?}"
    );
}

#[test]
fn the_ceiling_constant_is_unreachable_by_any_input() {
    let widest = all_triggers()
        .iter()
        .map(|t| t.default_bound().max_in_flight)
        .max()
        .unwrap();
    assert_eq!(widest, 2, "the widest variant default; got {widest}");
    assert!(
        widest < CEILING_IN_FLIGHT,
        "CEILING_IN_FLIGHT ({CEILING_IN_FLIGHT}) bounds nothing while the \
         widest default is {widest}. If a default ever reaches the ceiling the \
         constant becomes live and this file's premise changes."
    );
}

// ---------------------------------------------------------------------------
// 2. The runner does not read the field
// ---------------------------------------------------------------------------

/// The runner's own source, compiled into this test. A census over text rather
/// than over behaviour, because the behaviour is serial by construction and a
/// behavioural probe would pass on any implementation.
const RUNNER_SRC: &str = include_str!("../src/runner.rs");

#[test]
fn the_runner_enforces_the_other_two_bound_fields_and_not_this_one() {
    // KNOWN-POSITIVE CONTROL FIRST. The sibling fields ARE enforced, and if
    // this census cannot find them then it cannot find anything, and the zero
    // below would be free.
    let interval_hits = RUNNER_SRC.matches("min_interval_secs").count();
    let deadline_hits = RUNNER_SRC.matches("is_spent").count();
    assert!(
        interval_hits > 0,
        "census is dead: `min_interval_secs` IS enforced in runner.rs and was \
         not found"
    );
    assert!(
        deadline_hits > 0,
        "census is dead: `is_spent` IS enforced in runner.rs and was not found"
    );

    // The claim.
    let in_flight_hits = RUNNER_SRC.matches("max_in_flight").count();
    assert_eq!(
        in_flight_hits, 0,
        "`max_in_flight` now appears {in_flight_hits} time(s) in runner.rs \
         while `min_interval_secs` appears {interval_hits} and `is_spent` \
         {deadline_hits}. If the field became load-bearing, delete this test \
         and the NOTE in crates/wcore-cli/src/cron.rs."
    );
}
