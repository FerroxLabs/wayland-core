//! A process with a DETACHED grandchild — the shape FerroxLabs/wayland#1156
//! reported — buildable on either platform.
//!
//! `harness_owns_spawned_trees.rs` grades the guard against this shape on
//! Unix. FerroxLabs/wayland-core#358 needs the same shape on Windows and needs
//! a NEGATIVE control that runs on both, so the fixture moved here rather than
//! being written a third time.
//!
//! ## The handshake, and why it is the whole point
//!
//! The parent blocks on a line of stdin before it creates the grandchild. That
//! is not politeness — it is what makes the fixture grade the guard instead of
//! grading a race.
//!
//! On Windows [`OwnedTree::new`] assigns an ALREADY-RUNNING child to its Job
//! Object, and the kernel puts only a process's FUTURE descendants in a job.
//! A fixture whose parent forks immediately could therefore produce a
//! grandchild on either side of the assignment depending on scheduling, and a
//! test over it would be flaky in the direction that hides the bug. Releasing
//! the parent only after the caller holds the guard makes "the grandchild was
//! created after the assignment" true by construction, so a surviving
//! grandchild can only mean the guard does not own the tree.
//!
//! ## The two commands
//!
//! Both are the same program in two dialects: read a line, start a
//! long-running process that will NOT die with us, print its pid, block.
//!
//! * Unix — `/bin/sh`. `wait` is a shell BUILTIN, so the shell cannot tail-
//!   `exec` into it the way it would into a final `sleep`; the direct child
//!   therefore stays a real parent for the whole test instead of quietly
//!   becoming the grandchild.
//! * Windows — `powershell.exe` (System32 on every supported SKU) driving
//!   `System.Diagnostics.Process` directly rather than `Start-Process`.
//!   `Start-Process` defaults to `UseShellExecute = $true`, which can route
//!   the launch through the shell and hand the new process a different parent
//!   — which would take it out of the job and make this fixture prove nothing.
//!   `UseShellExecute = $false` forces a plain `CreateProcess`, so the new
//!   process is a real child and inherits the job. `ping.exe` is the
//!   long-running body for the same reason `wcore_types::job_object`'s own
//!   tests use it: it ships in System32, where `sleep` would smuggle in a
//!   "Git for Windows is installed" requirement.
//!
//! Nothing in this file is LLM-supplied, so the argv-mode rule in AGENTS.md
//! does not bite; the shell here is the subject of the test, not a way to run
//! something else.

#![allow(dead_code)] // Shared module: not every test binary uses every helper.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use super::owned_tree::OwnedTree;

/// How long the fixture processes stay alive if nothing kills them. Long
/// enough that a leak is still observable at the end of a slow test, short
/// enough that a leaked one is not a permanent squatter on a build host.
const FIXTURE_LIFETIME_SECS: u32 = 300;

#[cfg(unix)]
fn fixture_command() -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(format!(
        "read _ignored; sleep {FIXTURE_LIFETIME_SECS} & echo $!; wait"
    ));
    cmd
}

#[cfg(windows)]
fn fixture_command() -> Command {
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!(
            "[Console]::In.ReadLine() | Out-Null; \
             $si = New-Object System.Diagnostics.ProcessStartInfo; \
             $si.FileName = 'ping.exe'; \
             $si.Arguments = '-n {FIXTURE_LIFETIME_SECS} 127.0.0.1'; \
             $si.UseShellExecute = $false; \
             $si.RedirectStandardOutput = $true; \
             $p = [System.Diagnostics.Process]::Start($si); \
             [Console]::Out.WriteLine($p.Id); \
             [Console]::Out.Flush(); \
             Start-Sleep -Seconds {FIXTURE_LIFETIME_SECS}"
        ),
    ]);
    cmd
}

/// Spawn the fixture under a guard and return it with its grandchild's pid.
///
/// The grandchild is created AFTER the guard exists — see the module docs.
/// Killing the direct child does not reach it on either platform: on Unix it
/// reparents to init, on Windows a child simply does not die with its parent.
///
/// # Panics
/// If the fixture cannot be spawned or does not print a pid, which is a broken
/// fixture rather than a failed assertion and must not read as either verdict.
pub fn spawn_detaching_parent() -> (OwnedTree<Child>, u32) {
    let mut cmd = fixture_command();
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut guard = OwnedTree::new(cmd.spawn().expect("spawn the process-tree fixture"));

    // Release the parent only now: the guard — and on Windows its job — is in
    // place, so everything the parent creates from here is owned.
    let mut stdin = guard.child_mut().stdin.take().expect("stdin piped");
    stdin
        .write_all(b"go\n")
        .expect("release the fixture parent");
    stdin.flush().expect("flush the go-ahead");
    drop(stdin);

    let stdout = guard.child_mut().stdout.take().expect("stdout piped");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("read the grandchild pid the fixture printed");
    let grandchild: u32 = line.trim().parse().unwrap_or_else(|e| {
        panic!("the fixture printed {line:?}, which is not a pid ({e})");
    });
    (guard, grandchild)
}

/// Kill `pid` outright, ignoring "already gone".
///
/// Used only to clean up before reporting, so a failing assertion cannot
/// itself leave behind the orphan it is complaining about.
#[cfg(unix)]
pub fn force_kill(pid: u32) {
    // SAFETY: `kill` takes no pointers and delivers nothing but the signal
    // number; the pid is one this harness spawned or descends from one.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

#[cfg(windows)]
pub fn force_kill(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    // SAFETY: a null handle is checked before use and closed straight after
    // the single call that consumes it. An already-exited pid fails
    // `OpenProcess` or `TerminateProcess`, which is the ignored case.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return;
        }
        TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
}
