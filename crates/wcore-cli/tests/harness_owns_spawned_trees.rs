//! FerroxLabs/wayland#1156 — grades the harness-ownership guard itself.
//!
//! `profile_router_live::a_panicking_test_body_still_reaps_the_supervisor_tree`
//! exercises the guard against the real `acp serve` supervisor, but the profile
//! child there also dies on its own through the product's parent-death channel
//! (`wcore_cli::parent_channel`), so that test cannot distinguish a guard that
//! kills the TREE from one that only kills the leaf.
//!
//! This file builds a process tree the product has nothing to do with: a direct
//! child with a BACKGROUNDED grandchild, which a `SIGKILL` to the direct child
//! provably does not reach — the grandchild reparents to init and keeps
//! running. That is the exact shape the ticket reported ("a whole process tree
//! survived, not just a leaf"), reduced to two `sleep`s so it costs no build
//! and no server.
//!
//! Each of the guard's three mechanisms fails a DIFFERENT assertion here:
//!
//! | mechanism        | what breaks without it                              |
//! |------------------|-----------------------------------------------------|
//! | `impl Drop`      | both processes survive the unwind                   |
//! | descendant kill  | the grandchild survives                             |
//! | `wait()` (reap)  | the direct child stays a zombie, which `kill(pid,0)` |
//! |                  | still reports as alive                              |
//!
//! `/bin/sh` is used deliberately and only here: this is a Unix-gated harness
//! test that needs a process to fork and detach one, which is a shell's job.
//! `wcore-types/tests/real_zombie.rs` builds its trees the same way. Nothing in
//! this file is LLM-supplied, so the argv-mode rule in AGENTS.md does not bite.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::{OwnedTree, child_pids};

/// Is `pid` still a process? A ZOMBIE counts as one — that is deliberate, it is
/// what makes this predicate able to catch a kill that never reaped.
fn is_running(pid: u32) -> bool {
    // SAFETY: signal 0 performs the permission + existence check only; it takes
    // no pointers and delivers nothing.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Poll until `pid` is gone or the budget expires.
fn await_gone(pid: u32, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if !is_running(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !is_running(pid)
}

#[test]
fn dropping_the_guard_kills_a_detached_grandchild_and_reaps_the_direct_child() {
    // `wait` is a shell BUILTIN, so the shell cannot tail-`exec` into it the
    // way it would into a final `sleep`. The direct child therefore stays a
    // real parent with a real child for the whole test, instead of quietly
    // becoming the grandchild.
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg("sleep 300 & echo $!; wait")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut guard = OwnedTree::new(cmd.spawn().expect("spawn /bin/sh"));

    let direct = guard.id();
    let stdout = guard.child_mut().stdout.take().expect("stdout piped");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read the grandchild pid the shell printed");
    let grandchild: u32 = line
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("shell printed {line:?}, not a pid ({e})"));

    // Not vacuous: BOTH processes are alive, and the grandchild really is a
    // descendant the walk can see, before anything is killed.
    assert!(is_running(direct), "the direct child must be running");
    assert!(is_running(grandchild), "the grandchild must be running");
    assert!(
        child_pids(direct).contains(&grandchild),
        "the grandchild {grandchild} must be visible as a descendant of {direct}; \
         saw {:?}",
        child_pids(direct)
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

    // Clean up before reporting, so a failing assertion cannot itself leave the
    // orphan it is complaining about.
    // SAFETY: plain SIGKILLs to pids this test is responsible for.
    unsafe {
        libc::kill(grandchild as libc::pid_t, libc::SIGKILL);
        libc::kill(direct as libc::pid_t, libc::SIGKILL);
    }

    assert!(
        direct_gone,
        "the direct child {direct} is still a process after the guard was \
         dropped — either it was never killed, or it was killed but never \
         `wait`ed and is now a zombie (FerroxLabs/wayland#1156)"
    );
    assert!(
        grandchild_gone,
        "the grandchild {grandchild} outlived the guard — killing the direct \
         child does not reach a backgrounded descendant, which is exactly the \
         surviving process TREE the ticket reported (FerroxLabs/wayland#1156)"
    );
}
