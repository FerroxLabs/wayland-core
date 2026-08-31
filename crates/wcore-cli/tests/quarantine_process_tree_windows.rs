//! `FerroxLabs/wayland-core#393` — a quarantine `git` abort must take the
//! child's DESCENDANTS, not just the leaf, on Windows.
//!
//! # What was wrong
//!
//! `run_hardened` aborts on two paths: the wall-clock timeout, and the
//! drain-grace exit where `git` has already gone and a helper's background
//! worker still holds the inherited pipe. Both call `terminate_hardened_tree`,
//! which on unix `SIGKILL`s the process group `setsid(2)` created. On Windows
//! that function is a no-op and `Child::kill` is `TerminateProcess` — one
//! process. Everything `git` spawned kept running, after the install had
//! already reported an error and stopped looking.
//!
//! Windows never REGRESSED here the way unix did under `#379`: it had no group
//! teardown to lose. It had no teardown at all, which is worse and is why this
//! is its own issue.
//!
//! # The shape of the fixture, and why it is a `!`-alias
//!
//! The production trigger is a credential / askpass / transport helper that
//! `git` starts with our stdio INHERITED and does not detach. A `!`-alias
//! reproduces exactly that shape with no network, no credentials and no
//! third-party helper installed: `git` runs it through its bundled shell, the
//! alias re-execs THIS test binary as a probe, and the probe spawns a second
//! copy of itself that records its own pid and then sleeps. That second copy
//! is a grandchild of `git` and a great-grandchild of us — precisely the thing
//! `TerminateProcess` on the leaf cannot reach.
//!
//! # Non-vacuity
//!
//! Three separate defences, because "the descendant is gone" is exactly the
//! assertion a broken fixture passes for free:
//!
//! * the pid file must EXIST and parse — a descendant that never started is
//!   trivially absent, and would make every arm below green;
//! * a LIVENESS CONTROL runs the identical alias outside the quarantine path
//!   and asserts the descendant is still alive after `git` has exited, so the
//!   death in the graded arms is caused by the teardown and not by a
//!   descendant that simply ends on its own;
//! * both abort paths are graded separately, because `#393` c1 says BOTH and
//!   a fix wired into one of them meets an easier adjacent property.
//!
//! The recorded RED arm is a code mutation: dropping the
//! `WindowsJobObject::attach` block in `run_hardened` leaves the leaf kill in
//! place and both graded arms fail while the control still passes.

#![cfg(windows)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wcore_types::process_liveness::{ProcessLiveness, process_liveness};

/// Set on a re-executed copy of this binary; its value picks the role.
const ROLE_ENV: &str = "WCORE_QUARANTINE_TREE_ROLE";
/// Where the descendant records its own pid.
const PIDFILE_ENV: &str = "WCORE_QUARANTINE_TREE_PIDFILE";
/// How long the alias process itself stays alive after starting the
/// descendant, in milliseconds. `0` reaches the drain-grace abort; a value
/// past the run's timeout reaches the wall-clock abort.
const HOLD_MS_ENV: &str = "WCORE_QUARANTINE_TREE_HOLD_MS";

const TEST_NAME: &str = "a_quarantine_abort_takes_the_whole_process_tree_on_windows";

/// Long enough that the descendant cannot plausibly have exited on its own
/// within any arm of this test.
const DESCENDANT_LIFETIME: Duration = Duration::from_secs(300);

/// The role that IS the descendant: record the pid, then stay alive.
fn run_as_descendant() {
    let pidfile = std::env::var(PIDFILE_ENV).expect("descendant needs a pid file");
    let mut file = std::fs::File::create(&pidfile).expect("create pid file");
    write!(file, "{}", std::process::id()).expect("write pid");
    file.sync_all().expect("flush pid file");
    drop(file);
    std::thread::sleep(DESCENDANT_LIFETIME);
}

/// The role `git` runs: start the descendant, then leave — either at once
/// (drain-grace abort) or after out-living the caller's timeout (wall-clock
/// abort).
///
/// The descendant is spawned with our stdio INHERITED, which is what makes the
/// drain-grace arm reachable: the pipe `git` handed us cannot see EOF while a
/// process holding a copy of its write end is alive. That is the production
/// shape, not a contrivance for this test.
fn run_as_alias() {
    let exe = std::env::current_exe().expect("current test binary");
    let pidfile = std::env::var(PIDFILE_ENV).expect("alias needs a pid file");
    let hold_ms: u64 = std::env::var(HOLD_MS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let descendant = Command::new(exe)
        .arg(TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(ROLE_ENV, "descendant")
        .env(PIDFILE_ENV, &pidfile)
        .spawn()
        .expect("spawn descendant");
    // Deliberately never waited on. The descendant must OUTLIVE this alias --
    // that is the whole fixture -- so there is nothing to reap here, and
    // dropping the handle would be the same thing without saying so.
    std::mem::forget(descendant);

    // Do not race the driver: it reads the pid file after the abort, and an
    // alias that exited before the descendant had written it would make the
    // "descendant is gone" assertion vacuous rather than false.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && read_pid(Path::new(&pidfile)).is_none() {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(hold_ms));
}

fn read_pid(pidfile: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile).ok()?.trim().parse().ok()
}

/// The `git` argv that runs the alias role, as `run_git` would build it.
fn alias_args(exe: &Path) -> String {
    // Forward slashes and quoting keep the path safe for git's bundled shell,
    // which is what runs a `!`-alias on Windows.
    format!(
        "alias.treeprobe=!\"{}\" {TEST_NAME} --exact --nocapture --test-threads=1",
        exe.display().to_string().replace('\\', "/")
    )
}

/// Wait up to `budget` for `pid` to stop being live, and report what it was
/// at the end.
fn settle(pid: u32, budget: Duration) -> ProcessLiveness {
    let deadline = Instant::now() + budget;
    loop {
        let state = process_liveness(pid);
        if state == ProcessLiveness::Dead || Instant::now() >= deadline {
            return state;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Drive one graded arm through the PRODUCTION spawn path and return the
/// descendant's pid plus the error the abort produced.
fn abort_through_production(hold_ms: u64, timeout: Duration) -> (u32, String) {
    let exe = std::env::current_exe().expect("current test binary");
    let dir = tempfile::tempdir().expect("tempdir");
    let pidfile = dir.path().join("descendant.pid");

    let mut cmd = wcore_cli::plugin::quarantine::build_git_command(
        &["-c", alias_args(&exe).as_str(), "treeprobe"],
        None,
    );
    cmd.env(ROLE_ENV, "alias")
        .env(PIDFILE_ENV, &pidfile)
        .env(HOLD_MS_ENV, hold_ms.to_string());

    let err = wcore_cli::plugin::quarantine::run_hardened(cmd, "git [treeprobe]", timeout)
        .expect_err("the run must abort; it is the abort paths that are under test");

    let pid = read_pid(&pidfile).unwrap_or_else(|| {
        panic!(
            "no descendant pid was recorded at {}, so nothing was spawned to survive and \
             every assertion about its death would be vacuous. abort said: {err}",
            pidfile.display()
        )
    });
    (pid, err.to_string())
}

#[test]
fn a_quarantine_abort_takes_the_whole_process_tree_on_windows() {
    match std::env::var(ROLE_ENV).as_deref() {
        Ok("descendant") => return run_as_descendant(),
        Ok("alias") => return run_as_alias(),
        _ => {}
    }

    // ---- liveness control, FIRST -----------------------------------------
    // Run the identical alias with nobody owning the tree, and prove the
    // descendant is still alive after `git` has exited. Without this, "the
    // descendant is gone" below is satisfied by a descendant that never runs
    // or that ends by itself, and the graded arms would pass against the
    // kill-the-leaf code this issue is about.
    let exe = std::env::current_exe().expect("current test binary");
    let control_dir = tempfile::tempdir().expect("tempdir");
    let control_pidfile = control_dir.path().join("descendant.pid");
    let control_status = Command::new("git")
        .args(["-c", alias_args(&exe).as_str(), "treeprobe"])
        .env(ROLE_ENV, "alias")
        .env(PIDFILE_ENV, &control_pidfile)
        .env(HOLD_MS_ENV, "0")
        .stdin(Stdio::null())
        // Null, not piped: the point of the control is that `git` exits and
        // nothing tears the tree down, and a piped stdout would make this
        // call block on the very descendant it is measuring.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run the control alias");
    assert!(
        control_status.success(),
        "the control alias did not run cleanly ({control_status}); the fixture, not the \
         product, is what failed"
    );
    let control_pid = read_pid(&control_pidfile).expect(
        "the control recorded no descendant pid, so this fixture spawns nothing and every \
         assertion below would be vacuous",
    );
    assert_eq!(
        process_liveness(control_pid),
        ProcessLiveness::Live,
        "CONTROL: with nobody owning the tree, the descendant must still be alive after \
         `git` exited. It is not, so the descendant does not outlive its parent here and \
         this fixture cannot demonstrate #393 at all"
    );
    kill_pid(control_pid);

    // ---- arm 1: the drain-grace abort ------------------------------------
    // `git` exits promptly and the descendant holds the inherited pipe, so
    // `join_drain` is what fails. The wall-clock budget is far above the
    // 5 s drain grace so it cannot be the guard that fires.
    let (drain_pid, drain_err) = abort_through_production(0, Duration::from_secs(120));
    assert!(
        drain_err.contains("pipe is still open"),
        "arm 1 must reach the DRAIN-GRACE abort, not some other exit, or it grades a path \
         #393 c1 does not name: {drain_err}"
    );
    let drain_state = settle(drain_pid, Duration::from_secs(20));
    assert_eq!(
        drain_state,
        ProcessLiveness::Dead,
        "#393 c1: after the drain-grace abort the descendant (pid {drain_pid}) is {drain_state:?}. \
         The leaf was reaped and its tree was left running. abort said: {drain_err}"
    );

    // ---- arm 2: the wall-clock abort -------------------------------------
    // The alias itself out-lives the timeout, so `git` is still running when
    // the wall-clock guard fires and the leaf kill happens on that branch.
    let (timeout_pid, timeout_err) = abort_through_production(30_000, Duration::from_millis(1_500));
    assert!(
        timeout_err.contains("timed out after"),
        "arm 2 must reach the WALL-CLOCK abort: {timeout_err}"
    );
    let timeout_state = settle(timeout_pid, Duration::from_secs(20));
    assert_eq!(
        timeout_state,
        ProcessLiveness::Dead,
        "#393 c1: after the wall-clock abort the descendant (pid {timeout_pid}) is \
         {timeout_state:?}. abort said: {timeout_err}"
    );
}

/// #393 c3's other half, and the reason the successful exit is not simply
/// "kill everything": a run that FINISHED must leave the tree standing.
///
/// `git-credential-cache--daemon` deliberately outlives the `git` that started
/// it and is shared with the operator's other `git` operations, so a teardown
/// that fired on success would be a product regression — the same distinction
/// the unix arm draws by not signalling the group on the disarm path. On
/// Windows that takes an explicit `WindowsJobObject::release`, because the job
/// kills on CLOSE: forgetting to release would take the tree down as the
/// guard dropped, and the successful path is exactly where nobody would look.
#[test]
fn a_successful_quarantine_run_leaves_its_tree_standing_on_windows() {
    if std::env::var_os(ROLE_ENV).is_some() {
        return;
    }
    let exe = std::env::current_exe().expect("current test binary");
    let dir = tempfile::tempdir().expect("tempdir");
    let pidfile = dir.path().join("descendant.pid");

    // A `git` that exits 0 and leaves a descendant behind, WITHOUT holding our
    // pipe — so the drains reach EOF, `run_hardened` disarms, and this is the
    // success path rather than an abort.
    let alias = format!(
        "alias.detachedprobe=!\"{}\" {TEST_NAME} --exact --nocapture --test-threads=1 > /dev/null 2>&1 &",
        exe.display().to_string().replace('\\', "/")
    );
    let mut cmd = wcore_cli::plugin::quarantine::build_git_command(
        &["-c", alias.as_str(), "detachedprobe"],
        None,
    );
    cmd.env(ROLE_ENV, "descendant").env(PIDFILE_ENV, &pidfile);

    let outcome = wcore_cli::plugin::quarantine::run_hardened(
        cmd,
        "git [detachedprobe]",
        Duration::from_secs(60),
    );
    assert!(
        outcome.is_ok(),
        "this arm must reach the SUCCESS path; it did not: {outcome:?}"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let pid = loop {
        if let Some(pid) = read_pid(&pidfile) {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "no descendant pid was recorded, so this arm proves nothing about what a \
             successful run leaves standing"
        );
        std::thread::sleep(Duration::from_millis(50));
    };

    let state = process_liveness(pid);
    kill_pid(pid);
    assert_eq!(
        state,
        ProcessLiveness::Live,
        "a SUCCESSFUL quarantine run killed the tree it started (pid {pid} is {state:?}). \
         On this path `git` ran to completion, so a descendant it left — \
         `git-credential-cache--daemon` in production — must survive. Check that \
         `HardenedTree::disarm` still calls `WindowsJobObject::release`: the job kills on \
         close, so dropping it without releasing takes the tree down silently."
    );
}
