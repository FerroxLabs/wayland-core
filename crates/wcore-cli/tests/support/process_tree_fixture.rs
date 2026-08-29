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
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::time::{Duration, Instant};

use wcore_types::process_liveness::process_is_alive;

use super::owned_tree::OwnedTree;

/// How long the fixture processes stay alive if nothing kills them. Long
/// enough that a leak is still observable at the end of a slow test, short
/// enough that a leaked one is not a permanent squatter on a build host.
const FIXTURE_LIFETIME_SECS: u32 = 300;

#[cfg(unix)]
fn fixture_command() -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(format!(
        "read _ignored; sleep {FIXTURE_LIFETIME_SECS} & echo $!; \
         while read _probe; do echo ack; done; wait"
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
             while ($null -ne [Console]::In.ReadLine()) {{ \
               [Console]::Out.WriteLine('ack'); \
               [Console]::Out.Flush() \
             }}; \
             Start-Sleep -Seconds {FIXTURE_LIFETIME_SECS}"
        ),
    ]);
    cmd
}

/// How long to wait for the fixture to answer a probe before calling it wedged.
///
/// This is NOT the window the assertions depend on — see [`RunningProof`]. It
/// only bounds a fixture that has stopped talking for a reason nothing here
/// models, so a broken fixture reports as a broken fixture instead of hanging
/// until nextest's `slow-timeout` kills the whole binary.
pub const PROBE_BUDGET: Duration = Duration::from_secs(10);

/// What a probe of the fixture parent established.
///
/// # Why a round trip and not a liveness sample
///
/// `process_is_alive(pid)` answers "is there a live process at this pid *right
/// now*". After something has called `kill(pid, SIGKILL)` there is a window in
/// which the answer is still `true`: the signal is pending, the task has not
/// been scheduled to die, and it is neither a zombie yet nor gone. A test that
/// samples liveness immediately after a kill therefore measures the scheduler,
/// not the guard — measured on hetzner-dsm with the guard mutated to over-kill,
/// `--retries 0`: 1 of 20 sequential runs and 9 of 80 runs under 8-way load
/// missed the over-kill entirely (FerroxLabs/wayland-core#358 c4).
///
/// A round trip does not have that window. The fixture parent answers `ack`
/// only by executing user-space code, and a task cannot return to user space
/// with a pending `SIGKILL`. So if an `ack` arrives in response to a probe
/// written at time `T`, the parent had no kill pending at some instant after
/// `T` — and every kill this test cares about was issued *before* `T`, inside
/// the `drop` that returned already. [`RunningProof::Ran`] is therefore an
/// observation, not a sample, and it does not become wrong under load.
#[derive(Debug, PartialEq, Eq)]
pub enum RunningProof {
    /// The parent executed user-space code after the probe was written.
    Ran,
    /// The parent was observed gone while waiting for its answer.
    Gone,
    /// Still alive and still silent after the budget: a wedged fixture, which
    /// is neither verdict.
    NoAnswer,
}

/// A fixture parent, its detached grandchild, and the pipes that let the test
/// ask the parent whether it is still running.
///
/// Field order is load-bearing: `guard` is declared first, so `Drop` reaps the
/// tree before the pipes close. Dropping the pipes first would send the parent
/// to EOF on `stdin` and let it fall out of its read loop on its own, which
/// would make "the tree died" ambiguous.
pub struct DetachingParent {
    guard: OwnedTree<Child>,
    grandchild: u32,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl DetachingParent {
    /// The direct child's pid.
    pub fn id(&self) -> u32 {
        self.guard.id()
    }

    /// The detached grandchild's pid.
    pub fn grandchild(&self) -> u32 {
        self.grandchild
    }

    /// Give up the pipes and keep the guard, for tests that only need the two
    /// pids and the `Drop`.
    pub fn into_parts(self) -> (OwnedTree<Child>, u32) {
        // The parent falls through to its terminal wait when `stdin` closes,
        // so it stays a live parent of the grandchild either way.
        let DetachingParent {
            guard, grandchild, ..
        } = self;
        (guard, grandchild)
    }

    /// Ask the parent to prove it is still executing user-space code.
    ///
    /// See [`RunningProof`] for why the answer is a proof rather than a
    /// sample. The channel is drained first so the `ack` that ends this call
    /// cannot be a leftover from an earlier one: the fixture writes exactly one
    /// `ack` per probe line, so a drained channel plus one probe means the
    /// `ack` observed here was written after this probe.
    pub fn prove_running(&mut self, budget: Duration) -> RunningProof {
        loop {
            match self.lines.try_recv() {
                Ok(_) => continue,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return RunningProof::Gone,
            }
        }
        let pid = self.id();
        if self
            .stdin
            .write_all(b"probe\n")
            .and_then(|()| self.stdin.flush())
            .is_err()
        {
            return RunningProof::Gone;
        }

        let deadline = Instant::now() + budget;
        loop {
            match self.lines.recv_timeout(Duration::from_millis(20)) {
                Ok(line) if line.trim() == "ack" => return RunningProof::Ran,
                Ok(_) => continue,
                Err(RecvTimeoutError::Disconnected) => return RunningProof::Gone,
                Err(RecvTimeoutError::Timeout) => {
                    // A dead parent is the answer, and it is a terminal one: a
                    // process that has been killed never speaks again, so this
                    // does not race the budget.
                    if !process_is_alive(pid) {
                        return RunningProof::Gone;
                    }
                    if Instant::now() >= deadline {
                        return RunningProof::NoAnswer;
                    }
                }
            }
        }
    }
}

/// Spawn the fixture under a guard, with its grandchild's pid and its pipes.
///
/// The grandchild is created AFTER the guard exists — see the module docs.
/// Killing the direct child does not reach it on either platform: on Unix it
/// reparents to init, on Windows a child simply does not die with its parent.
///
/// # Panics
/// If the fixture cannot be spawned or does not print a pid, which is a broken
/// fixture rather than a failed assertion and must not read as either verdict.
pub fn spawn_detaching_parent() -> DetachingParent {
    let mut cmd = fixture_command();
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut guard = OwnedTree::new(cmd.spawn().expect("spawn the process-tree fixture"));

    // Read stdout on a thread from the start. The grandchild inherits the
    // write end of this pipe and outlives its parent by design, so a blocking
    // read on the test thread would not see EOF when the parent dies — the one
    // case `prove_running` has to answer promptly.
    let stdout = guard.child_mut().stdout.take().expect("stdout piped");
    let (tx, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    // Release the parent only now: the guard — and on Windows its job — is in
    // place, so everything the parent creates from here is owned.
    let mut stdin = guard.child_mut().stdin.take().expect("stdin piped");
    stdin
        .write_all(b"go\n")
        .expect("release the fixture parent");
    stdin.flush().expect("flush the go-ahead");

    let line = lines
        .recv_timeout(PROBE_BUDGET)
        .expect("read the grandchild pid the fixture printed");
    let grandchild: u32 = line.trim().parse().unwrap_or_else(|e| {
        panic!("the fixture printed {line:?}, which is not a pid ({e})");
    });
    DetachingParent {
        guard,
        grandchild,
        stdin,
        lines,
    }
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
