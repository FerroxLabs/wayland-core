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
//! persisted `max_in_flight` of `u32::MAX` therefore has an EFFECTIVE bound of
//! 1 on `once`/`interval`/`cron`/`webhook`/`poll`/`commitment`, and 2 on
//! `event`.
//!
//! ## `CEILING_IN_FLIGHT` was deleted, and why the tripwire moved here
//!
//! `wcore_cron::trigger::CEILING_IN_FLIGHT = 16` used to sit inside that clamp
//! as `default.max_in_flight.min(CEILING_IN_FLIGHT)`. Since every variant
//! default is a hardcoded 1 or 2 — no input reaches those literals — the
//! ceiling could never be the binding operand, and `lane/small-defects`
//! removed it (`F24-C2-M1`), unanimously backed by a three-model cross-audit
//! against the project rule that forbids code kept for hypothetical future
//! authors.
//!
//! Deleting a constant deletes the tripwire it carried, so the tripwire is
//! restated here as behaviour instead:
//! [`the_deleted_ceiling_changed_no_answer_for_any_variant`] runs the OLD and
//! NEW clamp expressions side by side over every variant and a grid of
//! persisted values, and carries its own known-negative proving the
//! differential can detect a divergence. If a variant default ever exceeds 16
//! the two expressions part company and that test goes red — the same warning
//! the constant used to give, now given by something that can fail.
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
use wcore_cron::trigger::{Trigger, TriggerBound};

/// The value `CEILING_IN_FLIGHT` carried before it was deleted. Kept as a local
/// literal so [`the_deleted_ceiling_changed_no_answer_for_any_variant`] can
/// still replay the old expression; it is deliberately NOT re-exported from the
/// crate, so no product code can start depending on it again.
const DELETED_CEILING_IN_FLIGHT: u32 = 16;

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

        // POSITIVE CONTROL: the persisted value really is the widest a `u32`
        // can express, so a clamped result below is the clamp acting rather
        // than a value never written. This is the exact control whose absence
        // produced this file's first wrong result.
        //
        // `u32::MAX` rather than the old `CEILING_IN_FLIGHT`: with the ceiling
        // deleted, the claim is about `clamp_to(default)` alone, and the
        // strongest input is the largest one the persisted schema can hold.
        job.bound = Some(TriggerBound::new(1, u32::MAX));
        assert_eq!(
            job.bound.as_ref().unwrap().max_in_flight,
            u32::MAX,
            "{kind}: the persisted bound must really carry the widest value"
        );

        let effective = job.effective_bound().max_in_flight;
        assert!(
            effective <= 2,
            "{kind}: a persisted u32::MAX produced an EFFECTIVE bound of \
             {effective}; no variant default permits more than 2"
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
fn no_variant_default_permits_more_than_two_in_flight() {
    let widest = all_triggers()
        .iter()
        .map(|t| t.default_bound().max_in_flight)
        .max()
        .unwrap();
    assert_eq!(
        widest, 2,
        "the widest variant default is expected to be 2; got {widest}. Every \
         claim in this file rests on the variant defaults being narrower than \
         anything a persisted job can ask for."
    );
}

/// **The third assertion (lane brief §6b-ii): the change is provably a no-op,
/// and the probe that says so can fail.**
///
/// Deleting `CEILING_IN_FLIGHT` from `clamp_to` is only safe if the old and new
/// expressions agree on every input. Asserting "the tests still pass" would not
/// show that — the tests passed before the deletion too. So the two expressions
/// are evaluated side by side here, over every trigger variant and a grid of
/// persisted values spanning both sides of the deleted ceiling.
///
/// The known-negative is the load-bearing half: the same differential is run
/// against a synthetic default that DOES exceed 16, where the two expressions
/// must disagree. Without it, a differential that always returned "equal"
/// (a typo, a collapsed loop, an empty variant list) would pass for free.
#[test]
fn the_deleted_ceiling_changed_no_answer_for_any_variant() {
    /// The clamp as it was written before the deletion.
    fn old(persisted: u32, default: u32) -> u32 {
        persisted.min(default.min(DELETED_CEILING_IN_FLIGHT)).max(1)
    }
    /// The clamp as it is written now.
    fn new(persisted: u32, default: u32) -> u32 {
        persisted.min(default).max(1)
    }

    // Values on both sides of the deleted ceiling, including the ceiling
    // itself and the widest the schema can hold.
    const PERSISTED: [u32; 8] = [0, 1, 2, 3, 15, 16, 17, u32::MAX];

    // (1) KNOWN-POSITIVE / the claim: over every real variant, the two
    //     expressions agree everywhere.
    let mut compared = 0usize;
    for trigger in all_triggers() {
        let kind = trigger.kind();
        let default = trigger.default_bound().max_in_flight;
        for persisted in PERSISTED {
            assert_eq!(
                old(persisted, default),
                new(persisted, default),
                "{kind}: deleting CEILING_IN_FLIGHT changed the answer for a \
                 persisted {persisted} against a variant default of {default}. \
                 The deletion is NOT behaviour-preserving and must be reverted."
            );
            compared += 1;
        }
    }
    // The differential must actually have run. An empty variant list or an
    // empty value grid would satisfy every assertion above having compared
    // nothing — the vacuous-green shape this program keeps finding.
    assert_eq!(
        compared,
        Trigger::KINDS.len() * PERSISTED.len(),
        "the differential compared {compared} pairs; it must cover every \
         variant against every probe value or it proves nothing"
    );

    // (2) KNOWN-NEGATIVE: the probe CAN detect a divergence. Against a
    //     hypothetical variant default of 100, the deleted ceiling would have
    //     clamped to 16 and the current code clamps to 100 — so the two
    //     expressions must part company. If this assertion ever fails, the
    //     differential above is dead and its zero divergences were free.
    let wide_default = 100u32;
    assert_eq!(old(u32::MAX, wide_default), 16);
    assert_eq!(new(u32::MAX, wide_default), 100);
    assert_ne!(
        old(u32::MAX, wide_default),
        new(u32::MAX, wide_default),
        "the differential cannot distinguish the old expression from the new \
         one even where they provably differ, so its agreement above is \
         meaningless"
    );

    // (3) AND the reason (1) holds: no real variant is anywhere near the
    //     deleted ceiling. This is the tripwire the constant used to carry.
    //     A future variant with a wide default trips (1) and this together.
    for trigger in all_triggers() {
        let default = trigger.default_bound().max_in_flight;
        assert!(
            default < DELETED_CEILING_IN_FLIGHT,
            "{}: variant default {default} has reached the deleted ceiling of \
             {DELETED_CEILING_IN_FLIGHT}. A ceiling on in-flight fires may now \
             be genuinely load-bearing — reconsider the deletion rather than \
             relaxing this assertion.",
            trigger.kind()
        );
    }
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
