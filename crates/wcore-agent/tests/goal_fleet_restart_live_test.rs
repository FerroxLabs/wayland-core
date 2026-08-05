//! The assertion side of the live kill/restart exercise (22-03 Task 4).
//!
//! This reads a journal and an effect directory a REALLY-killed, REALLY-restarted
//! process wrote, from paths given to it, and re-derives the four-part outcome.
//! It is a test rather than a one-time script so the same assertions re-run
//! against any future capture — including one taken on a platform this session
//! never touched.
//!
//! Point it at a capture with:
//!
//! ```text
//! F22_03_LIVE_JOURNAL=/path/session.journal \
//! F22_03_LIVE_EFFECTS=/path/effects        \
//! F22_03_LIVE_GOAL=g-live                  \
//! F22_03_LIVE_TASKS=10                     \
//!   cargo test -p wcore-agent --test goal_fleet_restart_live_test
//! ```
//!
//! ## Why this skips when unpointed, and why that is not a self-passing gate
//!
//! Stated plainly because "skips when the env var is absent" is exactly the
//! shape a vacuous gate takes. The skip is safe ONLY because it is not the gate:
//! the gate for the live exercise is the evidence-index check in the plan's own
//! `verify` block, which counts effect lines in a committed capture and fails
//! when the capture is missing. This file is the re-runnable ASSERTION, and when
//! it is pointed at a capture it makes real claims that can go red — which was
//! measured, not assumed: neutralising the instrument's idempotency key produced
//! 11 effect lines against 10 distinct and turned
//! `every_task_effect_appears_exactly_once` red.

use std::collections::BTreeMap;
use std::path::PathBuf;

use wcore_agent::session_journal::{GoalTaskState, SessionJournal};

const SESSION: &str = "p22-ledger-live";

struct Capture {
    tasks: BTreeMap<String, GoalTaskState>,
    effects: Vec<String>,
    expected: usize,
}

/// Load a capture, or `None` when this run was not pointed at one.
///
/// Loaded exactly ONCE per process and shared. The journal's writer lease is
/// exclusive by design, so four tests each opening it in parallel is a race the
/// lease correctly refuses — the first draft of this file did exactly that and
/// three of its four tests failed on the lease rather than on the capture.
fn capture() -> Option<&'static Capture> {
    static CAPTURE: std::sync::OnceLock<Option<Capture>> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(load_capture).as_ref()
}

fn load_capture() -> Option<Capture> {
    let journal = std::env::var("F22_03_LIVE_JOURNAL").ok()?;
    let effects_dir = PathBuf::from(
        std::env::var("F22_03_LIVE_EFFECTS")
            .expect("F22_03_LIVE_EFFECTS must accompany the journal"),
    );
    let goal = std::env::var("F22_03_LIVE_GOAL").unwrap_or_else(|_| "g-live".to_owned());
    let expected: usize = std::env::var("F22_03_LIVE_TASKS")
        .expect("F22_03_LIVE_TASKS must accompany the journal")
        .parse()
        .expect("task count parses");

    let state = SessionJournal::open(&journal, SESSION)
        .expect("the captured journal opens")
        .state()
        .expect("the captured journal replays");
    let tasks = state
        .goals
        .get(&goal)
        .unwrap_or_else(|| panic!("the capture has no goal {goal}"))
        .tasks
        .clone();
    let effects = std::fs::read_to_string(effects_dir.join("effects.txt"))
        .expect("the captured effect file is readable")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect();

    Some(Capture {
        tasks,
        effects,
        expected,
    })
}

/// A capture with fewer than eight tasks cannot have had a kill land with tasks
/// claimed, finished-undelivered and unstarted all at once, so its "exactly
/// once" would be measuring a much weaker run than the criterion asks for.
fn require_capture() -> &'static Capture {
    let capture = capture().expect("no capture supplied");
    assert!(
        capture.expected >= 8,
        "a capture of {} tasks is too small for a kill to land in all three interesting states",
        capture.expected
    );
    capture
}

#[test]
fn every_task_effect_appears_exactly_once_across_the_whole_run() {
    if capture().is_none() {
        return;
    }
    let capture = require_capture();

    let mut sorted = capture.effects.clone();
    sorted.sort();
    let mut distinct = sorted.clone();
    distinct.dedup();

    // Duplicate execution is a COUNT here, not an inference: a duplicated effect
    // is a duplicated line in a file a dead process wrote.
    assert_eq!(
        sorted.len(),
        distinct.len(),
        "duplicate execution: {} effect lines but only {} distinct — {sorted:?}",
        sorted.len(),
        distinct.len()
    );
    // And a lost completion is the same count from the other side.
    assert_eq!(
        sorted.len(),
        capture.expected,
        "lost or extra completion: {} effect lines against {} tasks",
        sorted.len(),
        capture.expected
    );
}

#[test]
fn every_completion_produced_before_the_kill_survives_into_the_parents_delivered_set() {
    if capture().is_none() {
        return;
    }
    let capture = require_capture();

    assert_eq!(capture.tasks.len(), capture.expected);
    for task in capture.tasks.values() {
        let completion = task
            .completion
            .as_ref()
            .unwrap_or_else(|| panic!("task {} carries no durable completion", task.task_id));
        assert!(
            completion.delivered,
            "task {} completed but the parent never observed it",
            task.task_id
        );
        // A completion must be attributable to the attempt that actually held
        // the claim when it was produced, or "who ran this" is unanswerable
        // after the fact.
        assert!(
            task.attempts
                .iter()
                .any(|attempt| attempt.epoch == completion.epoch),
            "task {} has a completion at an epoch no attempt ever held",
            task.task_id
        );
    }
}

#[test]
fn every_dependency_unblocked_exactly_once_in_the_real_journal() {
    if capture().is_none() {
        return;
    }
    let capture = require_capture();

    for task in capture.tasks.values() {
        assert_eq!(
            task.dependency_releases, 1,
            "task {} was released {} times, not once",
            task.task_id, task.dependency_releases
        );
        for dependency in &task.depends_on {
            let upstream = capture
                .tasks
                .get(dependency)
                .unwrap_or_else(|| panic!("task {} depends on absent {dependency}", task.task_id));
            assert!(
                upstream.completion.is_some(),
                "task {} ran while its dependency {dependency} had no durable completion",
                task.task_id
            );
        }
    }
}

#[test]
fn no_attempt_was_left_unresolved_and_reassignment_is_visible_in_the_history() {
    if capture().is_none() {
        return;
    }
    let capture = require_capture();

    for task in capture.tasks.values() {
        assert!(
            !task.requires_resolution(),
            "task {} finished the run needing explicit resolution",
            task.task_id
        );
        assert!(
            task.live_attempt().is_none(),
            "task {} still holds a live claim after the run finished",
            task.task_id
        );
    }

    // The restart must actually have reassigned something. Without this the
    // capture could be of a run where the kill landed before anything was
    // claimed, and every assertion above would pass while proving nothing about
    // recovery.
    let reassigned = capture
        .tasks
        .values()
        .filter(|task| task.attempts.len() > 1)
        .count();
    assert!(
        reassigned > 0,
        "no task was reassigned, so this capture does not exercise the restart path"
    );
}
