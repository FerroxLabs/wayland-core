//! Issue #338 on WINDOWS — the platform arm of
//! `quarantine_terminal_authority.rs`, which is `#![cfg(unix)]` and therefore
//! graded nothing here.
//!
//! On unix the fix is `setsid(2)`: the quarantine child becomes a session
//! leader with NO controlling terminal, `open("/dev/tty")` returns `ENXIO`,
//! and it cannot reacquire the parent's terminal because `TIOCSCTTY` refuses a
//! tty that is already another session's ctty. That is ELIMINATION, and #338's
//! c2 ("any prompt raised inside a quarantine operation is distinguishable
//! from a prompt raised by Wayland itself") is satisfied by it: there is no
//! prompt to distinguish.
//!
//! Windows has `DETACHED_PROCESS`, and it is NOT the same primitive. It
//! withholds the parent's console AT CREATION; it does not make the console
//! unreachable afterwards. This file measures both halves rather than
//! reasoning about them, because the doc comment on
//! `harden_against_credential_prompt` used to assert the analogy outright.
//!
//! MEASURED, Windows 11 build 10.0.26200.9168, 2026-08-30:
//!
//! ```text
//! [plain]    SHARES_USER_CONSOLE_BEFORE=true   CONOUT_BEFORE=OPEN
//! [hardened] SHARES_USER_CONSOLE_BEFORE=false  CONOUT_BEFORE=DENIED(6)
//! [hardened] ATTACH_PARENT_PROCESS=SUCCEEDED   SHARES_USER_CONSOLE_AFTER=true
//! [hardened] ATTACH_BY_EXPLICIT_PID=SUCCEEDED  CONOUT_AFTER_EXPLICIT=OPEN
//! ```
//!
//! So one documented call — `AttachConsole(ATTACH_PARENT_PROCESS)` — puts a
//! `DETACHED_PROCESS` child back on the user's own console, and attaching by
//! EXPLICIT pid works too, which forecloses the obvious remedy: reparenting
//! the child (`PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`) onto a console-less
//! process cannot help, because the child never needed
//! `ATTACH_PARENT_PROCESS` in the first place. Giving the child a console of
//! its own (`CREATE_NO_WINDOW`) does not help either: the by-pid arm below
//! calls `FreeConsole` first and still gets back on the driver's console.
//!
//! What this file therefore asserts:
//!
//! * the negative control — an UNHARDENED child DOES inherit the console, so
//!   the environment can exhibit the defect;
//! * the property `DETACHED_PROCESS` genuinely delivers — no console at
//!   creation, for a hand-built child AND for a real `git` built by
//!   `build_git_command`, which is what grades the WIRING;
//! * liveness — hardening does not simply break `git`;
//! * and the RESIDUAL, pinned as an assertion rather than as prose, so that
//!   the day it stops holding this test says so instead of quietly agreeing.
//!
//! The residual is tracked as FerroxLabs/wayland-core#380; see the ledger
//! entry for #338 c2.

#![cfg(windows)]

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, FreeConsole, GetConsoleProcessList,
    GetConsoleWindow,
};

/// Set on the re-executed copy of this binary; its presence switches this
/// binary from driver to probe.
const PROBE_ENV: &str = "WCORE_QUARANTINE_CONSOLE_PROBE";
/// The driver's own pid, so the probe can attach by EXPLICIT pid as well as by
/// `ATTACH_PARENT_PROCESS`.
const PARENT_PID_ENV: &str = "WCORE_QUARANTINE_CONSOLE_PARENT_PID";

const TEST_NAME: &str = "quarantine_child_has_no_console_at_creation_on_windows";

/// Is this process on the SAME console as `driver_pid`?
///
/// The oracle that matters, and the reason `GetConsoleWindow()` is not it.
/// MEASURED here: the probe reached through git's own bundled shell reports a
/// NULL console WINDOW and still opens `CONOUT$` — Git for Windows runs its
/// `!`-aliases under MSYS, which gives the descendant a pseudoconsole, and a
/// ConPTY has no window handle. So "no window" does not mean "no console", and
/// "has a console" does not mean "has the USER'S console". `GetConsoleProcessList`
/// answers the actual question: it enumerates the pids attached to THIS
/// process's console, so the driver's presence in that list is identity, not
/// inference.
fn shares_console_with_driver() -> bool {
    let Ok(driver) = std::env::var(PARENT_PID_ENV) else {
        return false;
    };
    let Ok(driver): Result<u32, _> = driver.parse() else {
        return false;
    };
    let mut pids = [0u32; 64];
    // SAFETY: `pids` is a live, correctly sized buffer and its length is passed
    // as the count. A process with no console returns 0 and writes nothing.
    let n = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) } as usize;
    n > 0 && n <= pids.len() && pids[..n].contains(&driver)
}

fn console_window() -> &'static str {
    // SAFETY: no arguments, no state; returns NULL when this process has no
    // console WINDOW — which, per `shares_console_with_driver`, is not the same
    // question as having no console.
    if unsafe { GetConsoleWindow() }.is_null() {
        "NONE"
    } else {
        "PRESENT"
    }
}

/// Can this process write to the console device? `CONOUT$` resolves to the
/// ATTACHED console's active screen buffer, which is the surface a credential
/// prompt would land on.
fn conout() -> String {
    match std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        Ok(_) => "OPEN".to_string(),
        Err(e) => format!("DENIED({})", e.raw_os_error().unwrap_or(-1)),
    }
}

/// The half that runs in the spawned child.
fn run_as_probe() {
    println!("CONSOLE_WINDOW_AT_CREATION={}", console_window());
    println!("CONOUT_BEFORE={}", conout());
    println!(
        "SHARES_USER_CONSOLE_BEFORE={}",
        shares_console_with_driver()
    );

    // SAFETY: both are argument-free kernel32 calls with no invariants beyond
    // "this process is a console client or is not"; both report failure
    // through their return value, which is what is printed.
    let by_parent = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    println!(
        "ATTACH_PARENT_PROCESS={}",
        if by_parent != 0 {
            "SUCCEEDED"
        } else {
            "FAILED"
        }
    );
    println!("CONOUT_AFTER={}", conout());
    println!("SHARES_USER_CONSOLE_AFTER={}", shares_console_with_driver());

    // Detach and come back by EXPLICIT pid, so a reader can see that
    // reparenting the child is not a remedy.
    // SAFETY: argument-free; detaching a process with no console is a no-op
    // that reports failure through its return value, which is not used.
    unsafe { FreeConsole() };
    let pid: u32 = std::env::var(PARENT_PID_ENV)
        .expect("driver pid")
        .parse()
        .expect("driver pid is numeric");
    // SAFETY: as above; `pid` is a plain process id, and the call reports
    // failure through its return value.
    let by_pid = unsafe { AttachConsole(pid) };
    println!(
        "ATTACH_BY_EXPLICIT_PID={}",
        if by_pid != 0 { "SUCCEEDED" } else { "FAILED" }
    );
    println!("CONOUT_AFTER_EXPLICIT={}", conout());
    println!(
        "SHARES_USER_CONSOLE_AFTER_EXPLICIT={}",
        shares_console_with_driver()
    );
    let _ = std::io::stdout().flush();
}

/// Spawn this binary as a probe, optionally through the production hardening,
/// and return its report as `key=value` lines.
fn probe(harden: bool) -> String {
    let exe = std::env::current_exe().expect("current test binary");
    let mut cmd = Command::new(exe);
    cmd.arg(TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(PROBE_ENV, "1")
        .env(PARENT_PID_ENV, std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if harden {
        wcore_cli::plugin::quarantine::harden_against_credential_prompt(&mut cmd);
    } else {
        // The control must differ from the subject ONLY in the hardening, so
        // it is spawned with no creation flags at all — which is what a child
        // inherits by default.
        cmd.creation_flags(0);
    }
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(e) => format!("SPAWN_FAILED={e}\n"),
    }
}

/// The same probe reached through the PRODUCTION command builder, so the test
/// grades the wiring and not only the hardening function. An assertion against
/// `harden_against_credential_prompt` alone still passes when `run_git` stops
/// calling it.
fn probe_through_production_git() -> String {
    let exe = std::env::current_exe().expect("current test binary");
    // A `!`-prefixed git alias runs the command through git's shell. Forward
    // slashes and quoting keep the path safe for that shell.
    let alias = format!(
        "alias.consoleprobe=!\"{}\" {TEST_NAME} --exact --nocapture --test-threads=1",
        exe.display().to_string().replace('\\', "/")
    );
    let mut cmd = wcore_cli::plugin::quarantine::build_git_command(
        &["-c", alias.as_str(), "consoleprobe"],
        None,
    );
    cmd.env(PROBE_ENV, "1")
        .env(PARENT_PID_ENV, std::process::id().to_string());
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(e) => format!("SPAWN_FAILED={e}\n"),
    }
}

/// Pull one `KEY=value` field out of a probe report.
///
/// Searched ANYWHERE in the line, not anchored at its start: libtest writes
/// `test <name> ... ` before the first `--nocapture` line of the child, so an
/// anchored parse silently returns `None` for `CONSOLE_WINDOW_AT_CREATION` and
/// the negative control fails against a probe that answered perfectly.
/// Measured — that is exactly how this file first ran. The `=` in the needle is
/// what keeps `CONOUT_AFTER` from matching `CONOUT_AFTER_EXPLICIT=OPEN`.
fn field<'a>(report: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    report.lines().find_map(|l| {
        let idx = l.find(&needle)?;
        Some(l[idx + needle.len()..].trim())
    })
}

#[test]
fn quarantine_child_has_no_console_at_creation_on_windows() {
    if std::env::var_os(PROBE_ENV).is_some() {
        run_as_probe();
        return;
    }

    // The property is only observable from a driver that HAS a console. A test
    // binary run as a service or over a pipe may have none, so allocate one —
    // the console the children then contend for is a real one, exactly as the
    // unix sibling opens a real PTY.
    // SAFETY: argument-free; fails harmlessly when a console already exists,
    // which the branch guard has already excluded.
    if unsafe { GetConsoleWindow() }.is_null() {
        unsafe { AllocConsole() };
    }
    assert!(
        // SAFETY: as above.
        !unsafe { GetConsoleWindow() }.is_null(),
        "this driver has no console and could not allocate one, so #338's \
         Windows property is unobservable here — the run proves nothing and \
         must not be read as a pass"
    );

    let plain = probe(false);
    let hardened = probe(true);
    let production = probe_through_production_git();

    // Emit the measurements, not only the verdict. #338's Windows arm is a
    // claim about what a Win32 flag delivers, and a bare `ok` from a CI job on
    // a host nobody will re-run is not evidence of it.
    println!("--- #338 windows console authority, measured ---");
    for (arm, report) in [
        ("plain", &plain),
        ("hardened", &hardened),
        ("production_git", &production),
    ] {
        for line in report.lines().filter(|l| l.contains('=')) {
            println!("[{arm}] {}", line.trim());
        }
    }

    // ---- negative control -------------------------------------------------
    // Without this, every DENIED below could be an environment that has no
    // console for anyone.
    assert_eq!(
        field(&plain, "SHARES_USER_CONSOLE_BEFORE"),
        Some("true"),
        "negative control: an UNHARDENED child must land on the DRIVER'S OWN \
         console, or the environment cannot exhibit #338 at all and nothing \
         below means anything. plain report:\n{plain}"
    );
    assert_eq!(
        field(&plain, "CONOUT_BEFORE"),
        Some("OPEN"),
        "negative control: an UNHARDENED child must be able to write to the \
         user's console. plain report:\n{plain}"
    );

    // ---- what DETACHED_PROCESS does deliver -------------------------------
    assert_eq!(
        field(&hardened, "SHARES_USER_CONSOLE_BEFORE"),
        Some("false"),
        "#338: a quarantine child must not be CREATED on the user's console. \
         hardened report:\n{hardened}"
    );
    assert!(
        field(&hardened, "CONOUT_BEFORE").is_some_and(|v| v.starts_with("DENIED")),
        "#338: a quarantine child must not be able to write to the user's \
         console as created. hardened report:\n{hardened}"
    );

    // ---- the same, through the builder every quarantine spawn uses --------
    // NOTE the property asserted here, and why it is not `CONOUT_BEFORE`.
    // MEASURED: this arm reports `CONOUT_BEFORE=OPEN` — Git for Windows runs a
    // `!`-alias under its bundled MSYS shell, which hands the descendant a
    // pseudoconsole of its OWN. That is a console, but it is not the user's,
    // and a prompt written to it does not reach the terminal the operator is
    // looking at. Sharing is the property #338 is about.
    assert_eq!(
        field(&production, "SHARES_USER_CONSOLE_BEFORE"),
        Some("false"),
        "#338: `build_git_command` must apply the hardening — an assertion on \
         `harden_against_credential_prompt` alone stays green when `run_git` \
         stops calling it. production report:\n{production}"
    );

    // ---- liveness ---------------------------------------------------------
    // A guard that refuses everything is not a fix, and would make every
    // assertion above meaningless.
    let version = wcore_cli::plugin::quarantine::build_git_command(&["--version"], None)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|e| format!("SPAWN_FAILED({e})"));
    assert!(
        version.starts_with("git version"),
        "liveness: hardened `git` must still run, got {version:?}"
    );

    // ---- the residual, pinned --------------------------------------------
    // This is NOT an assertion that the product is correct. It records, as a
    // measurement the tree carries, that `DETACHED_PROCESS` is weaker than
    // `setsid` and that #338 c2 is therefore satisfied by elimination on unix
    // ONLY. Tracked as FerroxLabs/wayland-core#380. If either of these ever
    // stops holding, the elimination argument has become true on Windows too —
    // at which point delete this block, invert it, and say so in the ledger.
    assert_eq!(
        field(&hardened, "ATTACH_PARENT_PROCESS"),
        Some("SUCCEEDED"),
        "the Windows residual behind #338 c2 (core#380) appears to be CLOSED: \
         a DETACHED_PROCESS child could no longer AttachConsole to its parent. \
         That is good news, not a regression — re-grade c2 on Windows and \
         replace this pin. hardened report:\n{hardened}"
    );
    assert_eq!(
        field(&hardened, "SHARES_USER_CONSOLE_AFTER"),
        Some("true"),
        "the Windows residual behind #338 c2 (core#380) appears to have \
         changed: the reattached child is no longer on the USER'S console. \
         Re-grade c2 on Windows and replace this pin. hardened \
         report:\n{hardened}"
    );
    assert_eq!(
        field(&hardened, "CONOUT_AFTER"),
        Some("OPEN"),
        "the Windows residual behind #338 c2 (core#380) appears to have \
         changed: the reattached child could no longer write to the console. \
         Re-grade c2 on Windows and replace this pin. hardened \
         report:\n{hardened}"
    );
    assert_eq!(
        field(&hardened, "ATTACH_BY_EXPLICIT_PID"),
        Some("SUCCEEDED"),
        "the by-pid arm changed. It exists to foreclose the obvious remedy: \
         while it holds, reparenting the child onto a console-less process \
         cannot fix #338 on Windows, because the child never needed \
         ATTACH_PARENT_PROCESS. hardened report:\n{hardened}"
    );
}
