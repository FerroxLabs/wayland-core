//! FerroxLabs/wayland-core#358 c4 — the NEGATIVE control for tree ownership.
//!
//! Every other test of `OwnedTree` asks whether it kills enough. This one asks
//! whether it kills too much, and it is the assertion that a wrong repair
//! fails. The two ways to get the Windows arm wrong in that direction are both
//! real:
//!
//! * a snapshot walk that enumerates every process on the box rather than the
//!   descendants of one pid — it would reap the build agent, the other test
//!   binaries running beside it, and the runner service itself;
//! * a Job Object attached to something above the child — the test process is
//!   already inside a job on a GitHub Actions runner, so a mechanism that
//!   reached for "the job this pid is in" instead of creating a fresh one
//!   would terminate the whole runner on the first `Drop`.
//!
//! Neither is hypothetical enough to leave ungraded, and neither is visible in
//! a test that only checks that the guarded tree died.
//!
//! ## Why it must pass in BOTH arms
//!
//! A control that only passes after the fix is not a control — it cannot
//! distinguish "the fix works" from "the fix broke something else". This one
//! is written so the leaf-only Windows guard that #358 was filed about passes
//! it too: it asserts nothing about what the guard reaches, only about what it
//! must not. It is deliberately cross-platform for the same reason — the Unix
//! arm has had a descendant walk all along, so the control has an arm on the
//! platform where an over-broad walk is actually reachable today.

use std::time::{Duration, Instant};

use wcore_types::process_liveness::process_is_alive;

#[path = "support/mod.rs"]
mod support;
use support::process_tree_fixture::{force_kill, spawn_detaching_parent};

/// Poll until `pid` is gone, or report that it is still there.
fn await_gone(pid: u32, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if !process_is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !process_is_alive(pid)
}

#[test]
fn dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive() {
    let (bystander, bystander_grandchild) = spawn_detaching_parent();
    let bystander_direct = bystander.id();
    let (victim, victim_grandchild) = spawn_detaching_parent();
    let victim_direct = victim.id();
    let runner = std::process::id();

    // Anti-vacuity: a control over processes that were never alive passes for
    // the wrong reason. All four, and the two trees are distinct.
    for (label, pid) in [
        ("the bystander parent", bystander_direct),
        ("the bystander grandchild", bystander_grandchild),
        ("the victim parent", victim_direct),
        ("the victim grandchild", victim_grandchild),
    ] {
        assert!(process_is_alive(pid), "{label} ({pid}) must be running");
    }
    assert_ne!(
        bystander_direct, victim_direct,
        "the two fixtures must be different processes"
    );

    drop(victim);

    // The victim's own tree is expected to die, but this control does NOT
    // assert that — that is what the positive tests are for, and asserting it
    // here would make the control fail in the pre-fix arm on Windows and stop
    // being a control at all.
    let bystander_direct_alive = process_is_alive(bystander_direct);
    let bystander_grandchild_alive = process_is_alive(bystander_grandchild);
    let runner_alive = process_is_alive(runner);

    // The bystander tree really does go down once ITS OWN guard drops. This
    // is measured BEFORE the cleanup kills below, so the two assertions that
    // follow cannot be passing because nothing is ever killed at all.
    drop(bystander);
    let bystander_died_with_its_own_guard = await_gone(bystander_direct, Duration::from_secs(10));

    // Clean up before reporting, so a failing assertion cannot itself leave
    // behind the orphan it is complaining about.
    for pid in [
        victim_grandchild,
        victim_direct,
        bystander_grandchild,
        bystander_direct,
    ] {
        force_kill(pid);
    }

    assert!(
        bystander_direct_alive,
        "dropping one guard killed a DIFFERENT guard's direct child \
         ({bystander_direct}) — the guard is reaping outside its own tree \
         (FerroxLabs/wayland-core#358 c4)"
    );
    assert!(
        bystander_grandchild_alive,
        "dropping one guard killed a DIFFERENT guard's grandchild \
         ({bystander_grandchild}) — a descendant walk or a job that reaches \
         beyond the tree it owns (FerroxLabs/wayland-core#358 c4)"
    );
    assert!(
        runner_alive,
        "dropping a guard killed the test runner itself ({runner})"
    );

    assert!(
        bystander_died_with_its_own_guard,
        "the bystander parent {bystander_direct} outlived its OWN guard, so \
         the assertions above pass only because this harness never kills \
         anything — the control proves nothing"
    );
}
