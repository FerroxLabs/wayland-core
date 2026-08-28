//! #693 — the learned-policy file is a cross-process read-modify-write.
//!
//! `~/.wayland/permissions.toml` is USER-global: every wayland session the
//! user has open resolves the same file, and each "always allow <tool>" grant
//! is a load / mutate / save against it. Without mutual exclusion spanning the
//! READ as well as the write, two sessions that grant a tool at the same
//! moment both read the same "before" contents and the later save overwrites
//! the earlier one's rule. The user pressed a key, the TUI said it saved, and
//! the grant is gone.
//!
//! Both arms below are REAL races — concurrent writers against one file on a
//! real filesystem, released into the critical section together by a
//! rendezvous file. Neither mocks the contention.
//!
//! * [`concurrent_threads_do_not_lose_grants`] races 8 OS threads. `flock`
//!   belongs to the open file description, and each writer opens the lock file
//!   itself, so two threads of one process contend exactly as two processes do.
//! * [`concurrent_processes_do_not_lose_grants`] races 6 separate OS
//!   processes, which is the shape the defect actually takes: independent
//!   `wayland` invocations, no shared address space, nothing but the
//!   filesystem between them.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use wcore_permissions::learning::{LearnedDecision, LearnedPolicy, LearningError};

const WORKSPACE: &str = "/workspace/race";
/// Rounds per writer. Enough that the writers stay overlapped for the whole
/// run rather than colliding once at the start.
const ROUNDS: usize = 12;
const THREAD_WRITERS: usize = 8;
const PROCESS_WRITERS: usize = 6;

/// Env var naming the policy file a re-executed child should write.
const CHILD_PATH_VAR: &str = "WL693_RACE_PATH";
/// Env var naming the child's writer index.
const CHILD_INDEX_VAR: &str = "WL693_RACE_INDEX";
/// Env var naming the rendezvous file a child waits on.
const GO_VAR: &str = "WL693_RACE_GO";

fn tool_name(writer: usize, round: usize) -> String {
    format!("Tool_w{writer}_r{round}")
}

/// Block until `go` exists. The parent creates it only once every writer is
/// running, so the writers enter the critical section together instead of
/// arriving spread out over process-spawn latency — without this the red arm
/// can accidentally pass by never actually overlapping.
fn await_rendezvous(go: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !go.exists() {
        assert!(Instant::now() < deadline, "rendezvous file never appeared");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Wall-clock budget for one writer's whole contribution.
///
/// `update_at` gives up after its own 2 s and REPORTS that. The TUI turns a
/// `LockTimeout` into a visible "could not save the always-allow decision"
/// notice (`wcore_cli::tui::engine_bridge::persist_always_allow_grant`), so a
/// timeout is a grant the user is TOLD about — not one silently lost, which is
/// the property this file exists to guard.
///
/// The poll inside `update_at` is unfair by construction (a fixed 10 ms
/// `try_write` retry, no queue), and a 10 ms sleep stretches on a saturated
/// host, so at the rendezvous an unlucky writer can lose every poll inside the
/// 2 s and be told to give up. Measured on this host: 10/10 clean at load 47,
/// and BOTH arms failing inside the full 15,978-test parallel run at load 120+
/// with `writer N round 0 failed: gave up after 2s`.
///
/// So a REPORTED timeout is retried and only this budget fails. Nothing else is
/// retried, and [`assert_no_grant_was_lost`] is untouched: drop the lock and
/// the grants vanish and this file still fails, which is what it is for.
const WRITER_BUDGET: Duration = Duration::from_secs(60);

/// One writer's whole contribution: `ROUNDS` distinct always-allow grants.
fn write_grants(path: &Path, writer: usize) {
    let deadline = Instant::now() + WRITER_BUDGET;
    for round in 0..ROUNDS {
        loop {
            match LearnedPolicy::update_at(path, |policy| {
                policy.record_in(
                    tool_name(writer, round),
                    None,
                    LearnedDecision::AllowAlways,
                    WORKSPACE,
                )
            }) {
                Ok(()) => break,
                // Reported, not lost — try again until the budget is spent.
                Err(LearningError::LockTimeout { .. }) if Instant::now() < deadline => {}
                Err(error) => panic!("writer {writer} round {round} failed: {error}"),
            }
        }
    }
}

/// Assert every grant every writer made is present. A lost update shows up as
/// a missing tool, and the message names which.
fn assert_no_grant_was_lost(path: &Path, writers: usize, what: &str) {
    let policy = LearnedPolicy::load_from(path).expect("the policy file must still parse");
    let restored = policy.snapshot_in(WORKSPACE);

    let missing: Vec<String> = (0..writers)
        .flat_map(|w| (0..ROUNDS).map(move |r| tool_name(w, r)))
        .filter(|tool| !restored.contains_key(tool))
        .collect();

    assert!(
        missing.is_empty(),
        "{} of {} always-allow grants were LOST to a concurrent {what}: the \
         user made each of these decisions and the file does not hold them. \
         Missing: {:?}",
        missing.len(),
        writers * ROUNDS,
        missing,
    );
    assert_eq!(
        policy.len(),
        writers * ROUNDS,
        "every grant must survive exactly once"
    );
}

#[test]
fn concurrent_threads_do_not_lose_grants() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    let go = tmp.path().join("go");

    let handles: Vec<_> = (0..THREAD_WRITERS)
        .map(|writer| {
            let path = path.clone();
            let go = go.clone();
            std::thread::spawn(move || {
                await_rendezvous(&go);
                write_grants(&path, writer);
            })
        })
        .collect();

    std::fs::write(&go, b"go").expect("release the writers");
    for handle in handles {
        handle.join().expect("a writer thread panicked");
    }

    assert_no_grant_was_lost(&path, THREAD_WRITERS, "thread");
}

#[test]
fn concurrent_processes_do_not_lose_grants() {
    // Guard against a child re-entering this test: a child runs only the
    // helper below, but an explicit check costs nothing and a fork bomb is
    // expensive.
    if std::env::var_os(CHILD_PATH_VAR).is_some() {
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    let go = tmp.path().join("go");
    let exe = std::env::current_exe().expect("the test binary's own path");

    let children: Vec<std::process::Child> = (0..PROCESS_WRITERS)
        .map(|writer| {
            std::process::Command::new(&exe)
                .args(["--exact", "child_writer_helper", "--test-threads=1"])
                .env(CHILD_PATH_VAR, &path)
                .env(CHILD_INDEX_VAR, writer.to_string())
                .env(GO_VAR, &go)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn a writer process")
        })
        .collect();

    // Give every child time to reach the rendezvous, then release them all.
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(&go, b"go").expect("release the writers");

    for (writer, child) in children.into_iter().enumerate() {
        let out = child.wait_with_output().expect("wait for a writer process");
        assert!(
            out.status.success(),
            "writer process {writer} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    assert_no_grant_was_lost(&path, PROCESS_WRITERS, "process");
}

/// The child half of [`concurrent_processes_do_not_lose_grants`]. Re-executing
/// this binary is how the test gets genuinely separate processes; run without
/// the env vars (i.e. in a normal test run) it does nothing and passes.
#[test]
fn child_writer_helper() {
    let (Some(path), Some(index), Some(go)) = (
        std::env::var_os(CHILD_PATH_VAR),
        std::env::var_os(CHILD_INDEX_VAR),
        std::env::var_os(GO_VAR),
    ) else {
        return;
    };
    let writer: usize = index
        .to_string_lossy()
        .parse()
        .expect("writer index must be a number");
    await_rendezvous(&PathBuf::from(go));
    write_grants(&PathBuf::from(path), writer);
}

/// How long the wedged holder sits inside the critical section. It stands in
/// for a peer session SIGSTOPped (Ctrl-Z), paused in a debugger, or blocked on
/// a network home directory — measured at 17.9 s in the audit probe — and for
/// a lock orphaned by a forked child of a killed parent, which never releases
/// at all.
const WEDGE_HOLD: Duration = Duration::from_secs(6);
/// The caller must give up WELL inside the hold. `update_at`'s own budget is
/// 2 s; this leaves room for a loaded CI box without ever passing by accident
/// against a blocking `flock`, which could only return at `WEDGE_HOLD`.
const WEDGE_BUDGET: Duration = Duration::from_secs(4);

/// #693 — `update_at` is called INLINE on the TUI's synchronous event thread
/// (`TuiEngine::approve` ← the surface router), so a blocking `flock` there is
/// a frozen UI for as long as some unrelated process holds the lock, with no
/// message and no escape.
///
/// The holder here is a real `update_at` on another thread: `flock` belongs to
/// the open file description and each call opens the lock file itself, so two
/// threads of one process contend exactly as two processes do — the same
/// property `concurrent_threads_do_not_lose_grants` rests on.
///
/// This is NOT a licence to drop the lock. The race it closes is real and
/// reproduced (removing the guard loses 83 of 96 grants). The requirement is
/// that the wait is BOUNDED and the failure is HONEST: `LockTimeout` reaches
/// the same "this grant applies to this session only" notice a write failure
/// already does.
#[test]
fn a_wedged_lock_holder_does_not_hang_the_caller() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("permissions.toml");
    LearnedPolicy::new().save_to(&path).expect("prime the file");

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let holder_path = path.clone();
    let holder = std::thread::spawn(move || {
        LearnedPolicy::update_at(&holder_path, |policy| {
            entered_tx.send(()).expect("signal entry");
            std::thread::sleep(WEDGE_HOLD);
            policy.record_in("Holder", None, LearnedDecision::AllowAlways, WORKSPACE);
        })
    });
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the holder never entered the critical section");

    let started = Instant::now();
    let result = LearnedPolicy::update_at(&path, |policy| {
        policy.record_in("Wedged", None, LearnedDecision::AllowAlways, WORKSPACE);
    });
    let waited = started.elapsed();

    assert!(
        waited < WEDGE_BUDGET,
        "a keypress on the TUI event thread waited {waited:?} behind a wedged \
         lock holder — the UI is frozen for exactly that long, with no message"
    );
    assert!(
        matches!(result, Err(LearningError::LockTimeout { .. })),
        "giving up must be a LockTimeout the caller can report, got {result:?}"
    );

    // The holder is unharmed: bounding the WAITER does not weaken the lock.
    holder
        .join()
        .expect("holder thread panicked")
        .expect("the holder's own write must still succeed");
    let stored = LearnedPolicy::load_from(&path).expect("parse");
    assert_eq!(
        stored.len(),
        1,
        "the holder's grant must be the one on disk — the timed-out caller \
         must not have published a policy derived from a stale read"
    );
}
