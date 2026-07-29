//! Both directions of the liveness probe, proven against a **real** corpse.
//!
//! The failure mode this module guards is not "the probe is wrong", it is
//! "the probe is wrong in a way that manufactures a green". A liveness check
//! that answered `Dead` for everything would satisfy every descendant-
//! containment test in this workspace without containing anything, so the
//! positive direction is asserted just as hard as the negative one.
//!
//! Three assertions per corpse, and the third is the one that proves the
//! repair does anything at all:
//!
//! 1. **known-positive** — a genuinely running process reads as `Live`.
//! 2. **known-negative** — a real, unreaped corpse reads as `Dead`.
//! 3. **the old shape would have missed it** — at the same instant, the
//!    probes this workspace used before (`kill(pid, 0) == 0`,
//!    `/proc/<pid>` exists, `OpenProcess` succeeds) all report the corpse as
//!    ALIVE. Without this assertion the test passes on the broken probe too.
//!
//! The corpse is created, not simulated: a child process is spawned, allowed
//! to exit, and deliberately never reaped. On Unix that is a zombie in state
//! `Z`; on Windows it is an exited process whose pid is still reserved by the
//! handle its parent holds — the same observable hazard.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wcore_types::process_liveness::{ProcessLiveness, process_is_alive, process_liveness};

/// Spawn a process that exits immediately and return its pid, holding the
/// `Child` so the corpse is never reaped. Blocks until the child's stdout
/// reaches EOF, which happens inside the child's exit path — an ordering
/// guarantee that needs no external tool.
///
/// A corpse is constructed differently on each family because the two families
/// keep corpses differently, and pretending otherwise produces a racy test:
///
/// * **Unix** — spawn, wait for the child's stdout to reach EOF (which happens
///   inside its exit path), and **never** `wait()` it. The unreaped child is a
///   zombie in state `Z`. Caller then polls an independent oracle for `Z`,
///   because fd teardown in `do_exit` happens fractionally before the task
///   actually becomes a zombie.
/// * **Windows** — spawn and `wait()` it, so the process has *definitively*
///   exited, then keep the `Child` alive. Rust's `Child` holds the process
///   HANDLE until it drops, and an open handle keeps the pid **reserved**, so
///   `OpenProcess` keeps succeeding for a process that is already gone. That
///   is the Windows analogue of the zombie, and constructing it this way makes
///   the test deterministic rather than dependent on how far into its exit
///   path `cmd.exe` had got when the pipe closed.
#[cfg(unix)]
fn spawn_unreaped_corpse() -> (std::process::Child, u32) {
    let mut child = corpse_command()
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn a child that exits immediately");
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut sink = Vec::new();
    // Returns when the last write end of the pipe closes, i.e. when the child
    // exits. NOTE: `child` is NOT waited on anywhere in this file.
    stdout
        .read_to_end(&mut sink)
        .expect("read the corpse's stdout to EOF");
    (child, pid)
}

#[cfg(windows)]
fn spawn_unreaped_corpse() -> (std::process::Child, u32) {
    let mut child = corpse_command()
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn a child that exits immediately");
    let pid = child.id();
    // `wait()` on Windows is `WaitForSingleObject` + `GetExitCodeProcess`; it
    // does NOT close the handle, which `Child` keeps until drop. So after this
    // returns the process is certainly exited AND its pid is certainly still
    // reserved — both halves of the hazard, with no race.
    let status = child.wait().expect("wait for the child to exit");
    assert_eq!(
        status.code(),
        Some(7),
        "independent confirmation that the child really exited, and with the \
         status we asked for"
    );
    (child, pid)
}

#[cfg(unix)]
fn corpse_command() -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "exit 7"]);
    command
}

#[cfg(windows)]
fn corpse_command() -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "exit 7"]);
    command
}

#[cfg(unix)]
fn spawn_live_child() -> std::process::Child {
    Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .expect("spawn a long-lived child")
}

#[cfg(windows)]
fn spawn_live_child() -> std::process::Child {
    Command::new("cmd")
        .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
        .spawn()
        .expect("spawn a long-lived child")
}

/// The probe as this workspace wrote it before this module existed:
/// `kill(pid, 0) == 0` on Unix, a successful `OpenProcess` on Windows.
/// Returns `true` for "the old code would have called this ALIVE".
#[cfg(unix)]
fn old_shape_says_alive(pid: u32) -> bool {
    // SAFETY: signal 0 delivers nothing; existence/permission check only.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
fn old_shape_says_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: Win32 FFI; the handle is closed on the success path.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: obtained from a successful OpenProcess, not used afterwards.
    unsafe { CloseHandle(handle) };
    true
}

/// The second old shape, Linux-only: `/proc/<pid>` existence.
#[cfg(target_os = "linux")]
fn old_proc_shape_says_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// An oracle INDEPENDENT of the code under test: `ps`. Returns the raw state
/// column, or `None` when `ps` is unavailable. Used to corroborate that the
/// corpse really is in state `Z`, never to decide the assertion.
#[cfg(unix)]
fn ps_state(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if state.is_empty() { None } else { Some(state) }
}

/// Wait until an oracle that is NOT the code under test agrees the pid is a
/// corpse. On Linux that is the raw `/proc/<pid>/stat` text; elsewhere it is
/// `ps`. Bounded, and it reports what it saw rather than hanging.
#[cfg(unix)]
fn settle_into_corpse(pid: u32) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = String::from("<never observed>");
    while Instant::now() < deadline {
        #[cfg(target_os = "linux")]
        {
            if let Ok(raw) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                last = raw.trim().to_string();
                // Deliberately a crude, independent check on the raw text --
                // not a call into the parser under test.
                if let Some((_, after_comm)) = last.rsplit_once(')')
                    && after_comm.trim_start().starts_with('Z')
                {
                    return last;
                }
            }
        }
        if let Some(state) = ps_state(pid) {
            last = format!("ps state={state}");
            if state.starts_with('Z') {
                return last;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    last
}

#[test]
fn a_live_process_reads_as_live() {
    // DIRECTION 1 (the one a universal-denial probe would fail). Counted
    // explicitly so "every containment test passes" can never be achieved by
    // a probe that calls everything dead.
    let mut live = spawn_live_child();
    let pid = live.id();

    let mut observed = ProcessLiveness::Indeterminate;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        observed = process_liveness(pid);
        if observed == ProcessLiveness::Live {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(
        observed,
        ProcessLiveness::Live,
        "a running child (pid {pid}) must read as Live, not {observed:?}"
    );
    assert!(process_is_alive(pid), "process_is_alive must agree");
    assert!(
        old_shape_says_alive(pid),
        "control: the old probe must also call a genuinely live process alive \
         (pid {pid}) -- if it does not, this test is measuring something else"
    );

    let _ = live.kill();
    let _ = live.wait();
}

#[test]
fn the_running_test_process_reads_as_live() {
    // The cheapest possible positive control: the instrument must be able to
    // see itself. A probe that cannot see its own process cannot see anything.
    let me = std::process::id();
    assert_eq!(
        process_liveness(me),
        ProcessLiveness::Live,
        "the test process itself (pid {me}) must read as Live"
    );
}

#[test]
fn a_real_unreaped_corpse_reads_as_dead_and_the_old_shape_would_have_missed_it() {
    // DIRECTION 2 + the third assertion, on a corpse this test creates.
    let (_child, pid) = spawn_unreaped_corpse();

    #[cfg(unix)]
    let oracle = settle_into_corpse(pid);
    #[cfg(windows)]
    let oracle =
        String::from("wait() returned exit code 7; pid still reserved by the parent's handle");

    println!("independent oracle for pid {pid}: {oracle}");

    // -- assertion 3: the OLD probe reports this corpse as ALIVE ------------
    //
    // This runs FIRST and is the load-bearing one. If it fails, the corpse
    // was already fully reaped and assertion 2 below would pass for the
    // trivial reason -- which is exactly how a repair gets certified without
    // repairing anything.
    assert!(
        old_shape_says_alive(pid),
        "pid {pid} was expected to be an UNREAPED corpse, but the old probe \
         already reports it gone -- something reaped it, so this test is not \
         measuring the defect. Oracle said: {oracle}"
    );

    #[cfg(target_os = "linux")]
    assert!(
        old_proc_shape_says_alive(pid),
        "/proc/{pid} was expected to still exist for an unreaped corpse \
         (this is the second old shape, used by wcore-sandbox and wcore-swarm)"
    );

    #[cfg(unix)]
    assert!(
        oracle.contains(") Z ") || oracle.contains("ps state=Z"),
        "the independent oracle must confirm state Z for pid {pid}, saw: {oracle}"
    );

    // -- assertion 2: the repaired probe reports it DEAD --------------------
    let observed = process_liveness(pid);
    assert_eq!(
        observed,
        ProcessLiveness::Dead,
        "an unreaped corpse (pid {pid}) must read as Dead, not {observed:?}. \
         Oracle said: {oracle}"
    );
    assert!(
        !process_is_alive(pid),
        "process_is_alive must report false for a corpse (pid {pid})"
    );
}

#[test]
fn a_fully_reaped_process_reads_as_dead() {
    let mut child = corpse_command()
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn");
    let pid = child.id();
    child.wait().expect("reap it properly this time");

    // Small pid-reuse window is theoretically possible but the pid space is
    // large and nothing else spawns here; a failure would show as Live.
    assert_eq!(
        process_liveness(pid),
        ProcessLiveness::Dead,
        "a reaped process (pid {pid}) must read as Dead"
    );
}
