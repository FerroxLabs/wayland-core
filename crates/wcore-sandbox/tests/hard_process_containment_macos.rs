//! Live descendant process-tree containment (macOS sandbox-exec backend).
//!
//! This is the macOS counterpart to the Linux descendant-reaping proof in
//! `hard_process_containment.rs` (`contained_detached_child_exit`), adapted to
//! the sandbox-exec backend. Linux reaps the tree through the bubblewrap PID
//! namespace (`--die-with-parent`); macOS has no PID namespace, so the
//! sandbox-exec backend instead attaches the sandboxed process group to a
//! `ProcessTreeGuard` and SIGKILLs the whole group the instant the direct child
//! exits (see `backends/process_tree.rs` — the macOS `disarm`/`Drop` path).
//! Either way, a detached descendant is contained and reaped with its parent.
//!
//! Whole-file `#![cfg(target_os = "macos")]` gating mirrors `live_integrity.rs`
//! (`#![cfg(windows)]`): on other platforms the file compiles to zero tests.
//! The `WAYLAND_SANDBOX_LIVE_MACOS` env opt-in mirrors the
//! `WAYLAND_SANDBOX_LIVE_WINDOWS` gate in `live_integrity.rs`.

#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::sandbox_exec::SandboxExecBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// Run `/bin/sh -c '/bin/sleep 45 & exit <code>'` under the macOS sandbox and
/// return `(exit_code, wall_clock)`.
///
/// The detached `sleep` inherits the sandboxed child's stdout pipe. If the
/// backend did NOT own and reap the descendant process tree, the direct child
/// exiting would leave the grandchild `sleep` alive holding that pipe, so
/// `execute` would block draining stdout until the 45s sleep exits or the 30s
/// manifest timeout fires (returning `Err(Timeout)`). Because the backend
/// SIGKILLs the sandboxed process group when the direct child exits, `execute`
/// returns promptly with the declared exit code. The wall-clock bound is
/// therefore a falsifiable descendant-reaping assertion.
async fn contained_detached_child_exit(
    backend: &SandboxExecBackend,
    exit_code: u8,
) -> (i32, Duration) {
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(30)),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };
    let cmd = SandboxCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("/bin/sleep 45 & exit {exit_code}"),
        ],
        cwd: None,
    };
    let started = Instant::now();
    let out = backend
        .execute(&manifest, cmd)
        .await
        .expect("contained execution must return an exit status, not block or error");
    (out.exit_code, started.elapsed())
}

/// Descendant containment on every terminal path. A generous 20s bound sits
/// well below both the 45s detached sleep and the 30s manifest timeout, so a
/// non-reaping backend (grandchild holding the pipe) cannot pass.
#[tokio::test]
#[ignore = "live macOS process-tree acceptance; run via `--run-ignored all` with WAYLAND_SANDBOX_LIVE_MACOS=1"]
async fn required_live_macos_process_tree_contains_descendants() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_MACOS").is_err() {
        eprintln!(
            "skip: WAYLAND_SANDBOX_LIVE_MACOS not set \
             (host has not opted into live macOS execution)"
        );
        return;
    }
    let backend = SandboxExecBackend::new();
    if !backend.is_available() {
        eprintln!("skip: sandbox-exec probe failed on this host");
        return;
    }

    let bound = Duration::from_secs(20);

    let (zero_code, zero_elapsed) = contained_detached_child_exit(&backend, 0).await;
    assert_eq!(zero_code, 0, "zero-exit terminal path must report exit 0");
    assert!(
        zero_elapsed < bound,
        "zero-exit path leaked a descendant: execute took {zero_elapsed:?} (>= {bound:?})"
    );

    let (nonzero_code, nonzero_elapsed) = contained_detached_child_exit(&backend, 7).await;
    assert_eq!(
        nonzero_code, 7,
        "non-zero-exit terminal path must report the exact exit code"
    );
    assert!(
        nonzero_elapsed < bound,
        "non-zero-exit path leaked a descendant: execute took {nonzero_elapsed:?} (>= {bound:?})"
    );
}

// ---------------------------------------------------------------------------
// SIGKILL of the owner.
//
// The case above proves the tree is reaped when the owner gets to run its
// teardown. SIGKILL is the case where it does NOT: the kernel destroys the
// owner with no signal handler, no unwinding and no `Drop`, so every cleanup
// path written in Rust is unreachable by construction. Linux and Windows are
// both covered there by a kernel mechanism that needs no cooperation from the
// dying parent (bubblewrap's PID namespace / `--die-with-parent`, and a
// kill-on-close Job Object). macOS has neither, which is why it needs its own
// case.
//
// Everything here is graded from `ps`, from a process that is not in the tree
// and never was. Nothing the product prints about itself is evidence.
// ---------------------------------------------------------------------------

/// One row of the system process table.
#[derive(Debug, Clone)]
struct ProcessRow {
    pid: i32,
    process_group: i32,
    state: String,
    command: String,
}

impl ProcessRow {
    /// A zombie is not a survivor: it holds no resources and its parent (here,
    /// `launchd`, after reparenting) is about to reap it.
    fn is_running(&self) -> bool {
        !self.state.starts_with('Z')
    }
}

/// Read the process table, and refuse to return a count we could not take.
///
/// Every failure below panics rather than yielding an empty table. A scan that
/// could not look must never render as "zero survivors" — that reads as proof
/// of correctness and everything downstream banks it.
fn read_process_table() -> Vec<ProcessRow> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,pgid=,stat=,command="])
        .output()
        .expect("ps must be runnable: an unavailable instrument is not a measurement of zero");
    assert!(
        output.status.success(),
        "ps exited {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(group), Some(state)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(process_group)) = (pid.parse::<i32>(), group.parse::<i32>()) else {
            continue;
        };
        rows.push(ProcessRow {
            pid,
            process_group,
            state: state.to_owned(),
            command: fields.collect::<Vec<_>>().join(" "),
        });
    }
    // Instrument self-test. If the scanner cannot see its own process, with its
    // own non-empty command line, it cannot see an orphan either — and a blind
    // scanner agrees enthusiastically with every expectation of zero.
    let own = std::process::id() as i32;
    assert!(
        rows.iter()
            .any(|row| row.pid == own && !row.command.is_empty()),
        "the process scanner cannot see itself (pid {own}) in its own {} rows, \
         so it could not have seen a survivor",
        rows.len()
    );
    rows
}

/// One contained tree the owner probe published.
#[derive(Debug)]
struct OwnedTree {
    root: i32,
    group: i32,
    descendant: i32,
}

/// Everything the owner probe published.
#[derive(Debug)]
struct OwnedWorld {
    trees: Vec<OwnedTree>,
    /// An ordinary uncontained child, spawned after every guard existed. Not
    /// expected to die; killed during cleanup so the runner is left clean.
    stray: i32,
}

fn wait_for_published_world(
    status: &std::path::Path,
    owner: &mut std::process::Child,
    expected_trees: usize,
) -> OwnedWorld {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Some(status) = owner.try_wait().expect("poll owner probe") {
            panic!("the owner probe exited before publishing its trees: {status}");
        }
        if let Ok(text) = std::fs::read_to_string(status) {
            let trees: Vec<OwnedTree> = text
                .lines()
                .filter_map(|line| {
                    let fields: Vec<i32> = line
                        .strip_prefix("TREE=")?
                        .trim()
                        .split(',')
                        .filter_map(|value| value.parse::<i32>().ok())
                        .collect();
                    match fields[..] {
                        [root, group, descendant] => Some(OwnedTree {
                            root,
                            group,
                            descendant,
                        }),
                        _ => None,
                    }
                })
                .collect();
            let stray = text
                .lines()
                .find_map(|line| line.strip_prefix("STRAY=")?.trim().parse::<i32>().ok());
            if let Some(stray) = stray
                && trees.len() == expected_trees
            {
                return OwnedWorld { trees, stray };
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the owner probe never published {expected_trees} tree(s) within 60s");
}

/// Rows that belong to an owned tree: the two known pids, plus anything still
/// sitting in the owned process group (which catches the group's sentinel and
/// any descendant the probe never named).
fn survivors_of(trees: &[OwnedTree]) -> Vec<ProcessRow> {
    read_process_table()
        .into_iter()
        .filter(|row| {
            row.is_running()
                && trees.iter().any(|tree| {
                    row.pid == tree.root
                        || row.pid == tree.descendant
                        || row.process_group == tree.group
                })
        })
        .collect()
}

fn describe(rows: &[ProcessRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "[pid {} pgid {} {} {}]",
                row.pid, row.process_group, row.state, row.command
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn kill_now(pid: i32) {
    let _ = std::process::Command::new("/bin/kill")
        .args(["-9", &pid.to_string()])
        .status();
}

/// SIGKILL the owner of `tree_count` contained trees and grade the process
/// table from outside at 0.4s, 6s and 20s.
///
/// The 0.4s checkpoint is not decoration: a mechanism that only reaps on some
/// later timer or on the next agent start leaves a live tree in the window a
/// user would actually observe. The 20s checkpoint rejects the opposite error,
/// a mechanism that appears to work only because nothing had started yet.
fn assert_sigkilled_owner_leaves_nothing(tree_count: usize) {
    let workspace = tempfile::tempdir().expect("workspace");
    let status = workspace.path().join("trees.status");

    let mut owner = std::process::Command::new(env!("CARGO_BIN_EXE_macos_orphan_probe"))
        .arg(&status)
        .arg(workspace.path())
        .arg(tree_count.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn the owner probe");

    let world = wait_for_published_world(&status, &mut owner, tree_count);
    for tree in &world.trees {
        assert_eq!(
            tree.group, tree.root,
            "each contained tree must lead its own process group"
        );
    }

    // Take the agreement check while the two sides can still DISAGREE. If the
    // trees are already gone here, every later "zero survivors" is vacuous.
    let before = survivors_of(&world.trees);
    for tree in &world.trees {
        assert!(
            before.iter().any(|row| row.pid == tree.root),
            "baseline: contained root {} is not running; observed {}",
            tree.root,
            describe(&before)
        );
        assert!(
            before.iter().any(|row| row.pid == tree.descendant),
            "baseline: contained descendant {} is not running; observed {}",
            tree.descendant,
            describe(&before)
        );
    }
    eprintln!(
        "baseline over {} owned tree(s): {} process(es) {}",
        tree_count,
        before.len(),
        describe(&before)
    );

    // SIGKILL, from outside, with no warning of any kind to the owner.
    kill_now(owner.id() as i32);
    owner.wait().expect("reap the owner probe");

    let killed_at = Instant::now();
    let mut observations = Vec::new();
    let mut leaked = Vec::new();
    for after in [
        Duration::from_millis(400),
        Duration::from_secs(6),
        Duration::from_secs(20),
    ] {
        let until = killed_at + after;
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let survivors = survivors_of(&world.trees);
        observations.push(format!(
            "t+{after:?}: {} survivor(s) {}",
            survivors.len(),
            describe(&survivors)
        ));
        if !survivors.is_empty() {
            leaked.push(after);
        }
    }

    // Clean up before asserting, so a red run does not also litter the host.
    for row in survivors_of(&world.trees) {
        kill_now(row.pid);
    }
    kill_now(world.stray);

    for line in &observations {
        eprintln!("{line}");
    }
    assert!(
        leaked.is_empty(),
        "a SIGKILLed owner left {tree_count} owned tree(s) alive at {leaked:?}\n{}",
        observations.join("\n")
    );
}

/// A SIGKILLed owner must not leave its owned process tree running.
///
/// The owner here is `src/bin/macos_orphan_probe.rs`: it takes a real
/// `ProcessTreeGuard` over a real two-deep tree and then blocks.
#[test]
#[ignore = "live macOS SIGKILL orphan acceptance; run via `--ignored` with WAYLAND_SANDBOX_LIVE_MACOS=1"]
fn required_live_macos_sigkilled_owner_leaves_no_process_tree() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_MACOS").is_err() {
        eprintln!(
            "skip: WAYLAND_SANDBOX_LIVE_MACOS not set \
             (host has not opted into live macOS execution)"
        );
        return;
    }
    assert_sigkilled_owner_leaves_nothing(1);
}

/// The same, with THREE guards live at once — the shape a real agent session
/// has, and the one where the mechanism can quietly stop working.
///
/// Each guard's sentinel learns its owner died from EOF on a socketpair, and
/// `fork` copies descriptors regardless of close-on-exec. A sentinel created
/// for a later guard therefore inherits every earlier guard's owner-side
/// descriptor and pins it open. Nothing in the single-guard case can detect
/// that. Here, if the wake-ups did not cascade, the FIRST tree would survive
/// while the last was reaped — which is precisely a fix that passes its own
/// test and does nothing for a user.
#[test]
#[ignore = "live macOS SIGKILL orphan acceptance; run via `--ignored` with WAYLAND_SANDBOX_LIVE_MACOS=1"]
fn required_live_macos_sigkilled_owner_leaves_no_overlapping_trees() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_MACOS").is_err() {
        eprintln!(
            "skip: WAYLAND_SANDBOX_LIVE_MACOS not set \
             (host has not opted into live macOS execution)"
        );
        return;
    }
    assert_sigkilled_owner_leaves_nothing(3);
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test hard_process_containment_macos`
/// executes 0 of 3 and still exits 0 printing `test result: ok`. This guard is
/// deliberately NOT `#[ignore]`d: three suites in this repo carried a guard that
/// was itself ignored, which made each inert against precisely the scenario it
/// existed for — it could only fire under `--ignored`, by which point the real
/// case were running anyway.
///
/// It always runs, so this binary can never report success on zero executed
/// tests, and it FAILS when a caller sets `WAYLAND_REQUIRE_IGNORED=1 (or WAYLAND_SANDBOX_LIVE_MACOS=1)` to declare a run of the
/// ignored cases while passing an invocation that cannot execute any of them.
/// Skipped under nextest, whose `no-tests = "fail"` policy covers the same
/// ground at the invocation site.
/// Also honours `WAYLAND_SANDBOX_LIVE_MACOS`, this suite's own pre-existing
/// live-intent variable, so the CI job that sets it cannot silently run zero
/// cases.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    let declared = std::env::var("WAYLAND_REQUIRE_IGNORED").as_deref() == Ok("1")
        || std::env::var("WAYLAND_SANDBOX_LIVE_MACOS").as_deref() == Ok("1");
    if !declared {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "declared intent to run this suite's 3 #[ignore]d cases, but neither \
         --ignored nor --include-ignored was passed, so zero of them can execute. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-sandbox --test hard_process_containment_macos -- --ignored --test-threads=1"
    );
}
