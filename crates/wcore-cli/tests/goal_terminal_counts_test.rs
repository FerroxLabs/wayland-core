//! A Goal's terminal transition must count the GOAL, not the last run.
//!
//! ## The defect this is built to reach
//!
//! `goal run --terminate` built its `ShardSummary` list from the waves THIS
//! process dispatched, and `StrategyTermination::from_fleet` summed those into
//! `PartiallyCompleted { completed, failed }`. Every completion the Goal
//! already carried — everything an earlier, crashed process finished — is in the
//! chain and in none of this process's waves, so the terminal recorded a
//! finished job as a partial one.
//!
//! Measured on the shipped binary: a 6-task Goal whose parent was `SIGKILL`ed
//! mid-wave, resumed to 6-of-6 with `total=6 distinct=6` effects on disk, wrote
//! `partially_completed { completed: 2, failed: 0 }` to the durable journal —
//! and that record is what the host protocol serves to Desktop, permanently.
//!
//! ## Why this drives the binary rather than the adapter
//!
//! The wrong arithmetic was not in the adapter; the adapter faithfully summed
//! what it was handed. It was in WHAT the product handed it. A test that called
//! the adapter directly would have to rebuild the caller's mapping in order to
//! test it, which tests the test. So this runs `wayland-core goal` for real and
//! reads the terminal back out of the durable record.
//!
//! ## Why two runs and no kill
//!
//! The bug is arithmetic over runs, and a kill is only one way to get more than
//! one run. Declaring the second half of the work between two runs reaches the
//! same defect deterministically, in seconds, with no signal, no orphan reaping
//! and no waiting out a claim lease — so this can run in CI on every commit.
//! The crash-shaped path is proven live against the same binary.

#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_wayland-core");
const GOAL: &str = "g-terminal-counts";

/// `true` rather than a script: the worker only has to exit 0, and the effect
/// under test is the one `goal exec-task` writes around it. Unix-only for the
/// same reason the file is.
const WORKER: &str = "true";

fn goal(args: &[&str]) -> Output {
    let output = Command::new(BIN)
        .arg("goal")
        .args(args)
        .output()
        .expect("wayland-core runs");
    assert!(
        output.status.success(),
        "wayland-core goal {args:?} failed ({}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn declare(journal: &Path, tasks: &[&str]) {
    for task in tasks {
        goal(&[
            "task",
            "--journal",
            journal.to_str().unwrap(),
            "--goal",
            GOAL,
            "--task",
            task,
        ]);
    }
}

fn drive(journal: &Path, effects: &Path, terminate: bool) -> String {
    let journal = journal.to_str().unwrap().to_owned();
    let effects = effects.to_str().unwrap().to_owned();
    let mut args = vec![
        "run",
        "--journal",
        &journal,
        "--goal",
        GOAL,
        "--effects-dir",
        &effects,
        "--worker-command",
        WORKER,
        "--width",
        "4",
        "--shard-size",
        "2",
    ];
    if terminate {
        args.push("--terminate");
    }
    String::from_utf8(goal(&args).stdout).expect("stdout is utf-8")
}

fn status(journal: &Path) -> serde_json::Value {
    let out = goal(&[
        "status",
        "--journal",
        journal.to_str().unwrap(),
        "--goal",
        GOAL,
    ]);
    serde_json::from_slice(&out.stdout).expect("status prints JSON")
}

#[test]
fn a_goals_terminal_counts_the_goal_not_the_final_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = dir.path().join("session.journal");
    let effects = dir.path().join("effects");

    goal(&[
        "open",
        "--journal",
        journal.to_str().unwrap(),
        "--goal",
        GOAL,
        "--objective",
        "count the goal",
    ]);

    // Run one finishes half the work and leaves the Goal live, exactly as a
    // pre-crash parent would have.
    declare(&journal, &["t00", "t01", "t02"]);
    let first = drive(&journal, &effects, false);
    assert!(
        first.contains("goal_complete=true"),
        "run one did not finish its half:\n{first}"
    );

    // Run two is the successor: it does the rest and terminates the Goal.
    declare(&journal, &["t03", "t04", "t05"]);
    let second = drive(&journal, &effects, true);
    assert!(
        second.contains("goal_complete=true"),
        "run two did not finish the Goal:\n{second}"
    );

    // Ground truth from the chain: every declared task carries a delivered
    // completion. Asserted before the terminal so a broken run fails as a broken
    // run rather than as a wrong count.
    let state = status(&journal);
    let tasks = state["tasks"].as_object().expect("tasks");
    assert_eq!(tasks.len(), 6, "{state:#}");
    for (id, task) in tasks {
        assert_eq!(
            task["completion"]["delivered"],
            serde_json::json!(true),
            "task {id} has no delivered completion: {state:#}"
        );
    }

    // The durable terminal — the record the host protocol serves to Desktop —
    // must describe those six tasks and not the two-or-three this process ran.
    let terminal = &state["lifecycle"]["terminal"];
    assert_eq!(
        terminal["state"],
        serde_json::json!("partially_completed"),
        "{state:#}"
    );
    assert_eq!(
        terminal["completed"],
        serde_json::json!(6),
        "6 of 6 tasks carry a durable completion but the terminal records \
         completed={}: {state:#}",
        terminal["completed"]
    );
    assert_eq!(terminal["failed"], serde_json::json!(0), "{state:#}");
}
