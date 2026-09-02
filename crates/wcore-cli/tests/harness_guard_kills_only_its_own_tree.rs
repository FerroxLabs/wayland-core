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
//!
//! ## Why the bystander is PROBED and not sampled
//!
//! The first version of this control read `process_is_alive(bystander)` in the
//! statement after `drop(victim)`. That is a sample of the scheduler, not an
//! observation of the guard: `kill(pid, SIGKILL)` returns as soon as the
//! signal is pending, and the target stays live-looking until it is scheduled
//! to die. MEASURED on hetzner-dsm against the guard mutated to walk from the
//! child's PPid (an over-broad kill that reaps the sibling tree), `--retries 0`:
//!
//! | arm | n | detected | missed |
//! |-----|---|----------|--------|
//! | sampled, sequential | 20 | 19 | 1 |
//! | sampled, 8 concurrent | 80 | 71 | 9 |
//!
//! An 11 % per-attempt miss under load is a 30 % chance of a laundered green
//! at `[profile.ci] retries = 2`, which is why this binary also carries a
//! `retries = 0` override in `.config/nextest.toml`.
//!
//! The replacement is a round trip: the fixture parent answers `ack` only by
//! executing user-space code, and a task cannot return to user space with a
//! pending `SIGKILL`. Every kill this control cares about was issued inside a
//! `drop` that has already returned, so an `ack` received afterwards proves no
//! such kill was ever aimed at the bystander. See
//! `support::process_tree_fixture::RunningProof`.
//!
//! The grandchild assertion cannot be a round trip — the grandchild is a
//! `sleep`, it has no way to answer — so it stays a liveness check, but it is
//! taken AFTER the round trip has completed and it must hold across a settle
//! window rather than at one instant. It is the second net here, not the
//! primary one: every over-kill shape named above reaps the bystander PARENT,
//! which is what the round trip grades exactly.

use std::time::{Duration, Instant};

use wcore_types::process_liveness::process_is_alive;

#[path = "support/mod.rs"]
mod support;
use support::process_tree_fixture::{
    PROBE_BUDGET, RunningProof, force_kill, spawn_detaching_parent,
};

/// How long the bystander's grandchild must stay CONTINUOUSLY live after the
/// round trip has already proved its parent was never killed.
///
/// Not a guess at how long a `SIGKILL` takes: by the time this window opens,
/// `drop(victim)` has returned (so every kill it issues has been issued) and a
/// full probe round trip through the bystander parent has completed (so the
/// killed processes have had a scheduling round trip to die in). The window is
/// margin on top of that, and it costs the green arm exactly this much wall
/// clock and nothing else — a healthy grandchild is a `sleep` that never dies.
const GRANDCHILD_SETTLE: Duration = Duration::from_millis(500);

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

/// Poll `pid` across `window`, returning `false` at the FIRST observation that
/// it is not live rather than at one arbitrary instant.
fn stayed_alive(pid: u32, window: Duration) -> bool {
    let deadline = Instant::now() + window;
    loop {
        if !process_is_alive(pid) {
            return false;
        }
        if Instant::now() >= deadline {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn dropping_one_guard_leaves_a_sibling_guards_tree_and_the_runner_alive() {
    let mut bystander = spawn_detaching_parent();
    let bystander_direct = bystander.id();
    let bystander_grandchild = bystander.grandchild();
    let victim = spawn_detaching_parent();
    let victim_direct = victim.id();
    let victim_grandchild = victim.grandchild();
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

    // Anti-vacuity for the instrument itself: `Ran` has to be a state this
    // fixture can reach before its ABSENCE is read as "something killed the
    // bystander". Without this, a fixture that never answers would look
    // exactly like an over-broad kill.
    assert_eq!(
        bystander.prove_running(PROBE_BUDGET),
        RunningProof::Ran,
        "the bystander parent {bystander_direct} did not answer a probe BEFORE \
         anything was dropped, so this control's instrument is broken and its \
         verdict below would mean nothing"
    );

    drop(victim);

    // THE measurement. The victim's own tree is expected to die, but this
    // control does NOT assert that — that is what the positive tests are for,
    // and asserting it here would make the control fail in the pre-fix arm on
    // Windows and stop being a control at all.
    let bystander_proof = bystander.prove_running(PROBE_BUDGET);
    let bystander_grandchild_alive = stayed_alive(bystander_grandchild, GRANDCHILD_SETTLE);
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

    assert_eq!(
        bystander_proof,
        RunningProof::Ran,
        "after one guard was dropped, a DIFFERENT guard's direct child \
         ({bystander_direct}) could no longer prove it was executing — \
         `Gone` means the guard reaped outside its own tree, `NoAnswer` means \
         the fixture wedged (FerroxLabs/wayland-core#358 c4)"
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
