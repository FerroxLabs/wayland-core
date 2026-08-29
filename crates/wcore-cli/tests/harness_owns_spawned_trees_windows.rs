//! FerroxLabs/wayland-core#358 — the WINDOWS half of the harness-ownership
//! guard, graded on Windows.
//!
//! `harness_owns_spawned_trees.rs` is the Unix twin and the shape this
//! mirrors. It is `#![cfg(unix)]` because its mechanism is: a `/proc` walk and
//! `SIGKILL`. Windows had no mechanism at all — `child_pids` returned an empty
//! vector there, so the guard snapshotted an empty descendant set, killed the
//! direct child, and left the grandchild running on every one of the swept
//! sites at once. The guard looked present and owned nothing.
//!
//! It now owns the tree through a kill-on-close Job Object
//! (`wcore_types::job_object::WindowsJobObject`), the same primitive the
//! Windows sandbox and the MCP stdio transport already use. This file is what
//! grades that claim.
//!
//! ## Why this cannot be graded from the Linux build host
//!
//! Every load-bearing statement here is about what the Windows kernel does:
//! that a child does not die with its parent, that `TerminateProcess` reaches
//! exactly one process, and that a job reaches all of them. Nothing outside
//! Windows can answer any of the three.
//!
//! ## What each assertion is for
//!
//! | assertion | what breaks without the job |
//! |---|---|
//! | the job CONTAINS the grandchild | nothing — this is the anti-vacuity check, and it fails on a fixture whose grandchild escaped rather than on the guard |
//! | the grandchild is gone | it survives: killing the direct child does not reach it |
//! | the direct child is gone | unchanged; the leaf was always killed |

#![cfg(windows)]

use std::time::{Duration, Instant};

use wcore_types::process_liveness::process_is_alive;

#[path = "support/mod.rs"]
mod support;
use support::process_tree_fixture::{force_kill, spawn_detaching_parent};

/// Poll until `pid` is gone or the budget expires.
///
/// `process_is_alive` is the zombie-aware probe, which matters here for the
/// same reason it matters on Unix: a Windows pid stays reserved for as long as
/// anything holds a handle to the exited process, so a bare `OpenProcess`
/// would report a corpse as alive and this test would fail for the wrong
/// reason.
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
fn dropping_the_guard_kills_a_detached_grandchild_on_windows() {
    let (guard, grandchild) = spawn_detaching_parent();
    let direct = guard.id();

    // Not vacuous. Both processes are alive, and the grandchild really is
    // INSIDE the guard's job — asked of the kernel, before anything is killed.
    // Without this, the only evidence that the job holds the grandchild would
    // be the grandchild dying, which is the claim under test.
    assert!(process_is_alive(direct), "the direct child must be running");
    assert!(
        process_is_alive(grandchild),
        "the grandchild must be running"
    );
    let job = guard.job().expect("the guard must hold a job on Windows");
    assert!(
        job.contains(grandchild)
            .expect("ask the kernel whether the grandchild is in the job"),
        "the grandchild {grandchild} is not in the guard's job, so this test \
         would prove nothing about the guard — the fixture created it outside \
         the job (FerroxLabs/wayland-core#358)"
    );
    assert!(
        !job.contains(std::process::id())
            .expect("ask the kernel whether the test runner is in the job"),
        "the TEST PROCESS is inside the guard's job; dropping the guard would \
         terminate the runner itself"
    );

    // The exit path the leak actually took. Moving the guard in means its
    // `Drop` — and only its `Drop` — runs, while unwinding.
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owned = guard;
        panic!("deliberate panic with the tree still running");
    }));
    assert!(unwound.is_err(), "the deliberate panic must have unwound");

    let direct_gone = await_gone(direct, Duration::from_secs(10));
    let grandchild_gone = await_gone(grandchild, Duration::from_secs(10));

    // Clean up before reporting, so a failing assertion cannot itself leave
    // the orphan it is complaining about.
    force_kill(grandchild);
    force_kill(direct);

    assert!(
        direct_gone,
        "the direct child {direct} is still a process after the guard was \
         dropped (FerroxLabs/wayland#1156)"
    );
    assert!(
        grandchild_gone,
        "the grandchild {grandchild} outlived the guard — on Windows killing \
         the direct child does not reach a descendant, so without a Job Object \
         the guard owns the leaf and leaks the TREE \
         (FerroxLabs/wayland-core#358)"
    );
}
