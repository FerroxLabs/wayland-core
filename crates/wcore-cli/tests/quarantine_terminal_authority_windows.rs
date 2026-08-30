//! Issue #338, the Windows half — an untrusted plugin install must not be able
//! to prompt the user for credentials on the console it was launched from.
//!
//! The Unix arm of this property lives in `quarantine_terminal_authority.rs`
//! and is `#![cfg(unix)]`. Until this file existed the Windows arm was
//! UNEXERCISED: `harden_against_credential_prompt`'s `#[cfg(windows)]` branch
//! had no test anywhere, and the criterion it is graded against ("any prompt
//! raised inside a quarantine operation") is not platform-scoped.
//!
//! WHAT IS ACTUALLY DELIVERED ON WINDOWS, AND WHAT IS NOT
//!
//! On Unix the guarantee is strong and structural: `setsid(2)` puts the child
//! in a fresh session with no controlling terminal, `open("/dev/tty")` fails
//! with `ENXIO`, every descendant inherits that session, and the child cannot
//! reacquire the parent's terminal — `TIOCSCTTY` refuses a tty that is already
//! another session's ctty.
//!
//! `DETACHED_PROCESS` is NOT that. It withholds the parent's console AT
//! CREATION, which is what this test grades. It does not make the child
//! unable to obtain one afterwards: Win32 documents `AllocConsole()` for a
//! console-less process and `AttachConsole(ATTACH_PARENT_PROCESS)` for
//! attaching to the parent's console. So the Windows guarantee is
//! "does not inherit", not "cannot acquire", and the two platforms are not
//! equivalent. That asymmetry is measured here as a REPORTED probe rather than
//! asserted, and recorded on the criterion, instead of being left implied by
//! the word "analogue" in the source doc.
//!
//! ARMS
//!
//! * `PLAIN=OPEN` — an UNHARDENED child reaches `CONOUT$`. Negative control.
//!   If it reports `DENIED` the host has no console, this environment cannot
//!   exhibit the defect, and the test says UNEXERCISED instead of passing
//!   vacuously — the same vacuity trap the Unix arm re-executes into a PTY to
//!   avoid.
//! * `HARDENED=DENIED` — the same child through
//!   `harden_against_credential_prompt` cannot.
//! * `PRODUCTION_GIT` — a real `git` built by `build_git_command`, the builder
//!   every quarantine spawn goes through. This grades the WIRING, not just the
//!   function. It is NOT graded with the `CONOUT$` probe: measured on Windows
//!   11 26200, a hardened `git` alias reports `OPEN`, and `OPEN` is ambiguous —
//!   Git for Windows runs a `!`-alias through its MSYS2 `sh`, whose runtime
//!   allocates a console of its own. "Has a console" and "has THE USER'S
//!   console" are different claims and only the second is this issue. So the
//!   production arm asks `GetConsoleProcessList` instead: a console that
//!   contains THIS TEST'S pid is the user's, one that does not is a fresh
//!   allocation the child made for itself.
//! * `GIT_STILL_RUNS=true` — liveness control: a guard that refuses everything
//!   is not a fix.

#![cfg(windows)]

use std::process::{Command, Stdio};

/// Writes one byte to the console device and reports whether it could.
///
/// `CONOUT$` is the console the process is attached to; opening it needs a
/// console, exactly as `/dev/tty` needs a controlling terminal. The write is
/// wrapped so a failing redirection sets the errorlevel instead of leaking the
/// error text into the answer, and the answer itself goes to the PIPE we hand
/// the child, which exists whether or not a console does.
const CONSOLE_PROBE: &str = "(echo probe 1>CONOUT$) >nul 2>nul && echo OPEN|| echo DENIED";

/// The decisive probe: which processes share the console this child is
/// attached to. `GetConsoleProcessList` returns 0 when the caller has no
/// console at all, so `NOCONSOLE` and `CONSOLE:<pids>` are distinguishable and
/// neither is inferred.
const PIDLIST_PS1: &str = r#"
$src = @"
using System;
using System.Runtime.InteropServices;
public class WcoreConsoleProbe {
  [DllImport("kernel32.dll", SetLastError=true)]
  public static extern uint GetConsoleProcessList(uint[] lpdwProcessList, uint dwProcessCount);
}
"@
Add-Type -TypeDefinition $src | Out-Null
$buf = New-Object uint32[] 64
$n = [WcoreConsoleProbe]::GetConsoleProcessList($buf, 64)
if ($n -eq 0) { "NOCONSOLE" } else { "CONSOLE:" + (($buf[0..($n-1)]) -join ",") }
"#;

fn probe(harden: bool) -> String {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", CONSOLE_PROBE])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if harden {
        wcore_cli::plugin::quarantine::harden_against_credential_prompt(&mut cmd);
    }
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(e) => format!("SPAWN_FAILED({e})"),
    }
}

/// Write `PIDLIST_PS1` somewhere both we and a child can read it.
fn pidlist_script(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("wcore338_pidlist.ps1");
    std::fs::write(&p, PIDLIST_PS1).expect("write probe script");
    p
}

/// The pid-list probe, spawned directly, hardened or not.
fn pidlist(script: &std::path::Path, harden: bool) -> String {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(script)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    if harden {
        wcore_cli::plugin::quarantine::harden_against_credential_prompt(&mut cmd);
    }
    answer(cmd.output())
}

/// Empty output is not an answer. A probe that failed to start reads exactly
/// like `NOCONSOLE` unless the two are spelled differently, and this file
/// exists because ambiguous evidence had already been mistaken for a verdict
/// once: the first Windows run reported `PRODUCTION_GIT=OPEN`, which turned out
/// to mean "has a console", not "has the user's console".
fn answer(out: std::io::Result<std::process::Output>) -> String {
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                let err = String::from_utf8_lossy(&o.stderr).trim().replace('\n', " ");
                let err: String = err.chars().take(160).collect();
                format!("NO_OUTPUT(status={:?}, stderr={err:?})", o.status.code())
            } else {
                s
            }
        }
        Err(e) => format!("SPAWN_FAILED({e})"),
    }
}

/// The same pid-list probe through the PRODUCTION command builder, so the
/// wiring is graded: an assertion against the hardening function alone still
/// passes when `build_git_command` stops calling it.
fn pidlist_through_production_git(script: &std::path::Path) -> String {
    let alias = format!(
        "alias.consoleprobe=!powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File '{}'",
        script.display().to_string().replace('\\', "/")
    );
    let mut cmd = wcore_cli::plugin::quarantine::build_git_command(
        &["-c", alias.as_str(), "consoleprobe"],
        None,
    );
    answer(cmd.output())
}

fn git_still_runs() -> bool {
    let mut cmd = wcore_cli::plugin::quarantine::build_git_command(&["--version"], None);
    matches!(cmd.output(), Ok(out) if String::from_utf8_lossy(&out.stdout).starts_with("git version"))
}

#[test]
fn a_quarantine_child_does_not_inherit_the_users_console() {
    let plain = probe(false);
    if plain != "OPEN" {
        // Not a pass. This host cannot exhibit the defect at all, so the
        // hardened arm below would be true of completely unhardened code.
        println!(
            "UNEXERCISED — this process has no console, so a child could not \
             inherit one either (unhardened control said {plain:?}). Run this \
             from a console session; a green here would prove nothing."
        );
        return;
    }

    let hardened = probe(true);
    let live = git_still_runs();

    let tmp = std::env::temp_dir();
    let script = pidlist_script(&tmp);
    let me = std::process::id();
    let plain_pids = pidlist(&script, false);
    let hardened_pids = pidlist(&script, true);
    let production_pids = pidlist_through_production_git(&script);
    let _ = std::fs::remove_file(&script);

    println!(
        "SELF_PID={me}\nPLAIN={plain} HARDENED={hardened} GIT_STILL_RUNS={live}\n\
         PLAIN_PIDS={plain_pids}\nHARDENED_PIDS={hardened_pids}\nPRODUCTION_GIT_PIDS={production_pids}"
    );

    assert_eq!(
        hardened, "DENIED",
        "a hardened child reached a console; DETACHED_PROCESS is not being \
         applied. PLAIN={plain}"
    );
    assert!(
        live,
        "git could not run at all through the quarantine builder — a guard that \
         refuses everything is not a fix"
    );

    // The control for the pid-list instrument: an UNHARDENED child must land in
    // OUR console, or the instrument cannot tell the two cases apart and the
    // production assertion below would be unfalsifiable.
    let shares_ours = |s: &str| {
        s.strip_prefix("CONSOLE:")
            .map(|pids| pids.split(',').any(|p| p.trim() == me.to_string()))
            .unwrap_or(false)
    };
    assert!(
        shares_ours(&plain_pids),
        "instrument control failed: an UNHARDENED child did not report this \
         process ({me}) in its console list ({plain_pids}), so 'shares the \
         user's console' cannot be distinguished from 'has some console'"
    );

    // THE #338 property on Windows, stated as what DETACHED_PROCESS actually
    // buys: the quarantine child does not end up attached to the console the
    // user launched us from. A console it allocated for itself is a separate,
    // weaker problem and is recorded as a residual on the criterion, not
    // silently folded into a pass here.
    assert!(
        !shares_ours(&production_pids),
        "a git spawned through the PRODUCTION quarantine builder is attached to \
         THIS process's console ({production_pids} contains {me}) — a credential \
         helper it launches can prompt on the user's terminal (#338). \
         HARDENED_PIDS={hardened_pids} PLAIN_PIDS={plain_pids}"
    );
}
