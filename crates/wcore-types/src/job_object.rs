//! Windows process-tree ownership: a kill-on-close Job Object.
//!
//! # Why this lives here
//!
//! Windows has no process group. `TerminateProcess` on a wrapper reaps that
//! one PID and nothing else, so a `cmd /C <server>` shim leaves `<server>`
//! running as an orphan — still holding the inherited stdout/stderr pipe write
//! handles, which is how a "killed" child goes on wedging its reader forever.
//! A Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is the only
//! kernel-backed equivalent of `kill(-pgid)`, and it is a HARD boundary: a
//! descendant cannot leave the job the way a Unix child can `setsid` out of its
//! group.
//!
//! Two crates need exactly this primitive — `wcore-sandbox`'s
//! `ProcessTreeGuard` and `wcore-mcp`'s stdio transport — and AGENTS.md
//! forbids a second copy. `wcore-sandbox` is deliberately dep-light
//! (`wcore-types` is its only internal dependency) and `wcore-mcp` sits far
//! above it in the crate graph, so the one crate both already depend on is the
//! bottom one. That is also where the workspace's other OS-facing process
//! primitive already lives (see [`crate::process_liveness`]), and for the same
//! reason: `windows-sys` is a target-gated leaf system crate with no
//! transitive Rust dependencies, so the platform that does not select it pays
//! nothing.
//!
//! # The suspend/attach contract (load-bearing)
//!
//! A process assigned to a job carries its FUTURE descendants into the job,
//! but the ones it already spawned stay outside it forever. So the child must
//! not be allowed to run before the assignment lands. Callers therefore:
//!
//! 1. call [`WindowsJobObject::create_suspended`] on the `Command` before
//!    spawning, and
//! 2. call [`WindowsJobObject::attach`] with the new PID immediately after.
//!
//! `attach` resumes the child's threads only once the kernel has accepted the
//! assignment, and it verifies the SUSPEND COUNT rather than merely the
//! absence of an error. `ResumeThread` returns the thread's PREVIOUS suspend
//! count and succeeds on a thread that was never suspended, so "no error" is
//! not evidence that step 1 happened. The counts and what each means here:
//!
//! | previous | meaning | verdict |
//! |----------|---------|---------|
//! | `u32::MAX` | the call failed; `GetLastError` is meaningful | error |
//! | `0` | the thread was NOT suspended and nothing changed | error — step 1 was skipped, and the child may already have spawned a descendant outside the job |
//! | `1` | count is now 0 and the thread is runnable | the only success |
//! | `n > 1` | count is now `n - 1`; the thread is STILL frozen | error — something else (debugger, EDR) suspended it too, and resuming once would hand back a job whose only process never runs |
//!
//! The honest limit of that check: it reads the suspend count at one instant.
//! It cannot distinguish "never suspended" from "was created suspended and
//! something already resumed it" — both report `0`. It is a detector for the
//! skipped-`create_suspended` bug, not a proof that no descendant escaped.
//!
//! # De-duplication state (do not overclaim this)
//!
//! Moving this type into `wcore-types` removed the `wcore-mcp` copy. It did
//! NOT leave the workspace with one implementation:
//! `crates/wcore-eval-scenarios/src/process_tree.rs` still carries a third,
//! independent `mod windows` (`WindowsJob(HANDLE)`, `CreateJobObjectW`,
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `CREATE_SUSPENDED`,
//! `resume_only_thread`, its own `unsafe impl Send` and `Drop`, and its own
//! `Win32_System_Diagnostics_ToolHelp` dependency). Folding it in is a
//! deliberate separate change — it also owns `IsProcessInJob` /
//! `QueryInformationJobObject` membership assertions this type does not have.
//!
//! Nor are there only two consumers. `attach` has two DIRECT callers
//! (`wcore-mcp`'s stdio transport and `wcore-sandbox`'s `ProcessTreeGuard`),
//! but `ProcessTreeGuard::new` is itself called from seven production sites:
//! `wcore-sandbox`'s `sandbox_exec`, `no_sandbox` (twice), `bwrap` and
//! `process_capture` backends, plus `wcore-browser`'s `supervisor` (twice).

#![cfg(windows)]

use windows_sys::Win32::Foundation::HANDLE;

/// A kill-on-close Job Object owning a process and every descendant it goes on
/// to create. Dropping it terminates the whole tree.
pub struct WindowsJobObject(HANDLE);

// SAFETY: a Job Object handle is a process-wide kernel reference and this
// wrapper has unique ownership, so moving it across threads cannot duplicate a
// close or invalidate the handle.
//
// Deliberately NOT `unsafe impl Sync`. Both consumers store this behind a
// `std::sync::Mutex`, and `Mutex<T>: Sync` needs only `T: Send`, so a `Sync`
// impl buys nothing — it only leaves a hand-audited invariant standing for
// whoever adds the next `&self` method. If a future caller genuinely needs to
// share a `&WindowsJobObject` across threads, add the impl back together with
// the concrete reason and re-audit every `&self` method at that point.
unsafe impl Send for WindowsJobObject {}

impl WindowsJobObject {
    /// Mark `command` `CREATE_SUSPENDED` so the child cannot create an
    /// unowned descendant before [`attach`](Self::attach) assigns it to a job.
    ///
    /// Every caller of `attach` must call this first — see the module docs.
    pub fn create_suspended(command: &mut std::process::Command) {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        command.creation_flags(CREATE_SUSPENDED);
    }

    /// Create a kill-on-close job, assign the (suspended) process `pid` to it,
    /// then resume the process.
    ///
    /// Errors split at the assignment, and the caller's obligation differs:
    ///
    /// * Failing BEFORE `AssignProcessToJobObject` succeeds (job creation,
    ///   limit configuration, `OpenProcess`, the assignment itself) leaves the
    ///   process suspended and unowned. The caller must kill it rather than
    ///   leak a frozen child.
    /// * Failing in the resume pass AFTER the assignment landed does not: the
    ///   process is already in the job, so dropping the job on the way out
    ///   fires `TerminateJobObject` and takes the tree with it. The caller's
    ///   kill is then a harmless no-op on an already-dead pid.
    ///
    /// Callers therefore kill unconditionally on `Err` — that is correct for
    /// both shapes — but must not read this as "the child is always still
    /// alive to be killed".
    pub fn attach(pid: u32) -> std::io::Result<Self> {
        use std::mem;
        use std::ptr;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };

        unsafe {
            let job = CreateJobObjectW(ptr::null(), ptr::null());
            if job.is_null() {
                return Err(std::io::Error::from_raw_os_error(GetLastError() as i32));
            }
            let job = Self(job);

            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(std::io::Error::from_raw_os_error(GetLastError() as i32));
            }

            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return Err(std::io::Error::from_raw_os_error(GetLastError() as i32));
            }
            let assigned = AssignProcessToJobObject(job.0, process);
            let assign_error = if assigned == 0 {
                Some(std::io::Error::from_raw_os_error(GetLastError() as i32))
            } else {
                None
            };
            CloseHandle(process);
            if let Some(error) = assign_error {
                return Err(error);
            }
            Self::resume_process_threads(pid)?;
            Ok(job)
        }
    }

    /// Kill the job's whole process tree now, without waiting for the handle
    /// to be dropped. Idempotent, and a no-op on an already-empty job.
    pub fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: `self.0` is a live job handle for the lifetime of `self`,
        // and `TerminateJobObject` is thread-safe and idempotent.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }

    /// Every thread of `pid`, from a system-wide thread snapshot.
    ///
    /// The snapshot is the only route to a thread id from a pid.
    /// `std::os::windows::process::ChildExt::main_thread_handle` would make
    /// this O(1) and let the `Win32_System_Diagnostics_ToolHelp` dependency go
    /// away, but it is unstable (feature
    /// `windows_process_extensions_main_thread_handle`, rust-lang/rust#96723 —
    /// verified rejected by the pinned stable toolchain), it lives on
    /// `std::process::Child` while every spawn site here uses
    /// `tokio::process::Command` (whose `Child` exposes `raw_handle()`, the
    /// PROCESS handle, and never the primary thread's), and both callers of
    /// [`WindowsJobObject::attach`] hand it a bare pid rather than a `Child`.
    fn process_thread_ids(pid: u32) -> std::io::Result<Vec<u32>> {
        use std::mem;
        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_NO_MORE_FILES, GetLastError, INVALID_HANDLE_VALUE, SetLastError,
        };
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        };

        // SAFETY: the snapshot handle is closed on every path below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }

        let result = (|| {
            // SAFETY: `entry` is a plain POD struct with its `dwSize` set, as
            // `Thread32First`/`Thread32Next` require.
            let mut entry: THREADENTRY32 = unsafe { mem::zeroed() };
            entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
            // SAFETY: snapshot and entry are valid.
            if unsafe { Thread32First(snapshot, &raw mut entry) } == 0 {
                return Err(std::io::Error::last_os_error());
            }

            let mut ids = Vec::new();
            loop {
                if entry.th32OwnerProcessID == pid {
                    ids.push(entry.th32ThreadID);
                }
                // SAFETY: clear stale state so end-of-snapshot is
                // distinguishable from a real enumeration failure. Without
                // this an aborted walk reads back as "this process has no
                // threads", which the caller would report as the wrong bug.
                unsafe { SetLastError(0) };
                // SAFETY: snapshot and entry are valid.
                if unsafe { Thread32Next(snapshot, &raw mut entry) } == 0 {
                    // SAFETY: reads the calling thread's last-error slot.
                    let error = unsafe { GetLastError() };
                    if error != ERROR_NO_MORE_FILES {
                        return Err(std::io::Error::from_raw_os_error(error as i32));
                    }
                    break;
                }
            }
            Ok(ids)
        })();
        // SAFETY: snapshot was returned by CreateToolhelp32Snapshot.
        unsafe { CloseHandle(snapshot) };
        result
    }

    /// Resume `pid`'s threads, refusing anything that is not exactly one
    /// pending suspension per thread.
    ///
    /// See the module docs for the suspend-count table. The rule applied here
    /// is "every thread of `pid` must report a previous suspend count of
    /// exactly 1, and there must be at least one". A `CREATE_SUSPENDED` child
    /// has exactly one thread by construction — its primary thread has not run,
    /// so it cannot have created another — so for a caller that honoured the
    /// contract the per-thread rule and a one-thread rule are the same rule.
    /// For a caller that did not, the per-thread rule is the stricter of the
    /// two, and it is deliberately strict: a thread this pass does not leave
    /// runnable is a job whose only process never starts, and a thread that
    /// was already runnable is a process that may have escaped a descendant
    /// before the assignment landed. Both are hard errors, not warnings.
    fn resume_process_threads(pid: u32) -> std::io::Result<()> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
        };

        let thread_ids = Self::process_thread_ids(pid)?;
        if thread_ids.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "suspended child thread was not found",
            ));
        }

        for thread_id in thread_ids {
            // SAFETY: the suspended process cannot exit before it is resumed,
            // and OpenThread returns a separately owned handle.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
            if thread.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: `thread` was opened with THREAD_SUSPEND_RESUME.
            let previous = unsafe { ResumeThread(thread) };
            // Captured BEFORE `CloseHandle`, which overwrites the last-error
            // slot on its own success path.
            let resume_error = if previous == u32::MAX {
                Some(std::io::Error::last_os_error())
            } else {
                None
            };
            // SAFETY: `thread` was returned by OpenThread and is not used after.
            unsafe { CloseHandle(thread) };
            if let Some(error) = resume_error {
                return Err(error);
            }
            if previous != 1 {
                return Err(std::io::Error::other(format!(
                    "thread {thread_id} of child pid {pid} had suspend count {previous}, \
                     expected exactly 1 — the child was not created suspended, or something \
                     else suspended it as well, so this job cannot be trusted to own the tree"
                )));
            }
        }
        Ok(())
    }
}

impl Drop for WindowsJobObject {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: `self.0` is a live job handle and this is its unique owner.
        // Termination is idempotent for an already-empty job and closes the
        // cancellation race before the last job handle is released.
        unsafe {
            TerminateJobObject(self.0, 1);
            CloseHandle(self.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests. Windows-only by construction: the whole module is `#![cfg(windows)]`.
// They drive real processes, because the claim under test is about what the
// Windows kernel does, and only the kernel can answer that.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::WindowsJobObject;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Long enough to survive a loaded runner, short enough that a frozen
    /// child fails rather than hangs.
    const RUN_BUDGET: Duration = Duration::from_secs(30);

    /// A child that runs for roughly `pings - 1` seconds once it is allowed
    /// to, and forever if it is not.
    ///
    /// `ping` ships in System32 on every Windows SKU, so this fixture does not
    /// smuggle in a "Git for Windows is installed" requirement the way `sleep`
    /// would.
    fn child_command(pings: u32) -> Command {
        let mut command = Command::new("cmd.exe");
        command
            .args(["/C", &format!("ping -n {pings} 127.0.0.1 >nul")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// The contract holds: a `CREATE_SUSPENDED` child is owned AND resumed.
    ///
    /// The child's own exit is the positive control. Nothing here kills it —
    /// `ping -n 2` terminates only if the resume pass genuinely made the
    /// thread runnable, so an `attach` that owned the child and left it frozen
    /// fails here instead of passing quietly.
    #[test]
    fn attaching_to_a_create_suspended_child_owns_it_and_lets_it_run() {
        let mut command = child_command(2);
        WindowsJobObject::create_suspended(&mut command);
        let mut child = command.spawn().expect("spawn a suspended child");
        let pid = child.id();

        let job = WindowsJobObject::attach(pid).expect("attach must own a CREATE_SUSPENDED child");

        let deadline = Instant::now() + RUN_BUDGET;
        loop {
            if child.try_wait().expect("poll the child").is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "attach reported success but pid {pid} never ran to completion — it \
                 was assigned to the job and left suspended"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(job);
    }

    /// F1 red arm. A process that was never suspended must be REFUSED.
    ///
    /// `ResumeThread` succeeds on a running thread and returns the previous
    /// suspend count 0, so an implementation whose only failure test is
    /// `== u32::MAX` takes the success branch and hands back a job for a
    /// process that has been free to spawn descendants outside it since it
    /// started. That is the silently-lost race this type exists to remove, and
    /// before the suspend-count check this test failed with `attach` returning
    /// `Ok`.
    #[test]
    fn attaching_to_a_process_that_was_never_suspended_is_an_error() {
        let mut child = child_command(61).spawn().expect("spawn a running child");
        let pid = child.id();

        let result = WindowsJobObject::attach(pid);
        let refused = result.is_err();
        // Dropping an `Ok` job terminates its tree, so this covers the accepted
        // case; the kill below covers the refused one. Either way the fixture
        // must not outlive the test.
        drop(result);
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            refused,
            "attach accepted pid {pid}, which was never created suspended — the \
             assignment cannot have preceded anything the process already spawned"
        );
    }

    /// The second half of the same defect: resuming ONCE is not the same as
    /// making the thread runnable.
    ///
    /// A debugger or EDR that also suspends the child leaves the count at 2.
    /// The old code resumed to 1, saw no error, and reported success for a job
    /// whose only process is permanently frozen — a wedged MCP server that
    /// looks like a healthy spawn.
    #[test]
    fn attaching_to_a_doubly_suspended_child_is_an_error() {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenThread, SuspendThread, THREAD_SUSPEND_RESUME,
        };

        let mut command = child_command(61);
        WindowsJobObject::create_suspended(&mut command);
        let mut child = command.spawn().expect("spawn a suspended child");
        let pid = child.id();

        let thread_ids = WindowsJobObject::process_thread_ids(pid).expect("enumerate threads");
        assert_eq!(
            thread_ids.len(),
            1,
            "a CREATE_SUSPENDED child must have exactly one thread; the rest of \
             this test is meaningless if it does not"
        );
        for thread_id in &thread_ids {
            // SAFETY: the child is suspended and cannot exit; the handle is
            // closed immediately after the single call that uses it.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, *thread_id) };
            assert!(!thread.is_null(), "open the suspended child's thread");
            // SAFETY: `thread` was opened with THREAD_SUSPEND_RESUME.
            let previous = unsafe { SuspendThread(thread) };
            // SAFETY: `thread` was returned by OpenThread and is not used after.
            unsafe { CloseHandle(thread) };
            assert_eq!(
                previous, 1,
                "the child must already be suspended exactly once"
            );
        }

        let result = WindowsJobObject::attach(pid);
        let refused = result.is_err();
        drop(result);
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            refused,
            "attach accepted a child suspended twice: one ResumeThread leaves it \
             frozen, so this job owns a process that will never run"
        );
    }
}
