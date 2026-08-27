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
//! assignment. A caller that skips step 1 hands `attach` a process that is
//! already running: the assignment still succeeds, but `attach` then fails
//! with `NotFound` from the resume pass (there is no suspended thread to
//! resume), which is deliberate — a silently-lost race is exactly the bug this
//! type exists to remove.

#![cfg(windows)]

use windows_sys::Win32::Foundation::HANDLE;

/// A kill-on-close Job Object owning a process and every descendant it goes on
/// to create. Dropping it terminates the whole tree.
pub struct WindowsJobObject(HANDLE);

// SAFETY: a Job Object handle is a process-wide kernel reference and this
// wrapper has unique ownership, so moving it across threads cannot duplicate a
// close or invalidate the handle. `TerminateJobObject` is itself thread-safe
// and idempotent, which is what makes `&self` sharing (`Sync`) sound: the only
// operation reachable through a shared reference is that terminate.
unsafe impl Send for WindowsJobObject {}
unsafe impl Sync for WindowsJobObject {}

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
    /// On error the process is left suspended and unowned; the caller is
    /// responsible for killing it rather than leaking a frozen child.
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

    fn resume_process_threads(pid: u32) -> std::io::Result<()> {
        use std::mem;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        };
        use windows_sys::Win32::System::Threading::{
            OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
        };

        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::from_raw_os_error(GetLastError() as i32));
            }

            let result = (|| {
                let mut entry: THREADENTRY32 = mem::zeroed();
                entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
                if Thread32First(snapshot, &mut entry) == 0 {
                    return Err(std::io::Error::from_raw_os_error(GetLastError() as i32));
                }

                let mut resumed = false;
                loop {
                    if entry.th32OwnerProcessID == pid {
                        let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if thread.is_null() {
                            return Err(std::io::Error::from_raw_os_error(GetLastError() as i32));
                        }
                        let resume_result = ResumeThread(thread);
                        let resume_error = if resume_result == u32::MAX {
                            Some(std::io::Error::from_raw_os_error(GetLastError() as i32))
                        } else {
                            None
                        };
                        CloseHandle(thread);
                        if let Some(error) = resume_error {
                            return Err(error);
                        }
                        resumed = true;
                    }
                    if Thread32Next(snapshot, &mut entry) == 0 {
                        break;
                    }
                }

                if resumed {
                    Ok(())
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "suspended child thread was not found",
                    ))
                }
            })();
            CloseHandle(snapshot);
            result
        }
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
