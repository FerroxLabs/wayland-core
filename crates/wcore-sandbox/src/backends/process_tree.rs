//! Centralized child-process lifecycle ownership.

/// Prepare a Tokio command for platform process containment.
///
/// Call [`ProcessTreeGuard::new`] immediately after spawning the configured
/// command. On Windows the child is deliberately suspended until the guard
/// attaches it to a kill-on-close Job Object. Unix process groups reliably
/// collect ordinary background children, but an adversarial child can leave
/// its group with `setsid`/`setpgid`; hard Smart/Managed containment comes from
/// the sandbox backend (for example Bubblewrap's PID namespace), not this
/// Dangerous-mode reliability backstop.
pub fn isolate(command: &mut tokio::process::Command) {
    isolate_std(command.as_std_mut());
}

/// Prepare a synchronous command for the same platform containment primitive.
pub fn isolate_std(_command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        _command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        // The child cannot create an unowned descendant before `new` assigns
        // it to a Job Object. `WindowsJob::attach` resumes it only after the
        // kernel accepts that assignment.
        _command.creation_flags(CREATE_SUSPENDED);
    }
}

/// Armed while a direct child is alive. Dropping it kills the dedicated Unix
/// process group or the Windows Job. A Windows Job is a hard descendant
/// boundary; see [`isolate`] for the documented Unix limitation.
pub struct ProcessTreeGuard {
    #[cfg(unix)]
    process_group: Option<libc::pid_t>,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}

impl ProcessTreeGuard {
    pub fn new(_pid: Option<u32>) -> std::io::Result<Self> {
        let pid = _pid.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "spawned child has no PID")
        })?;
        Ok(Self {
            #[cfg(unix)]
            process_group: Some(libc::pid_t::try_from(pid).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "child PID exceeds pid_t")
            })?),
            #[cfg(windows)]
            job: Some(WindowsJob::attach(pid)?),
        })
    }

    /// Ask a Unix child group to unwind cooperatively before the guard's hard
    /// kill. This lets a supervised process drop guards for nested process
    /// groups of its own. Callers must apply a bounded wait and then drop this
    /// guard; cooperation is not assumed.
    #[cfg(unix)]
    pub fn request_graceful_shutdown(&self) -> std::io::Result<()> {
        let Some(process_group) = self.process_group else {
            return Ok(());
        };
        // SAFETY: `isolate_std` created a dedicated group whose ID is the child
        // PID. A negative PID targets that group only.
        let result = unsafe { libc::kill(-process_group, libc::SIGTERM) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    pub(crate) fn disarm(&mut self) {
        #[cfg(unix)]
        if let Some(process_group) = self.process_group.take() {
            terminate_process_group(process_group);
        }
        #[cfg(windows)]
        {
            // Closing the last KILL_ON_JOB_CLOSE handle also reaps any
            // background descendants that outlived the direct child.
            self.job = None;
        }
    }
}

impl Drop for ProcessTreeGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(process_group) = self.process_group.take() {
            terminate_process_group(process_group);
        }
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group: libc::pid_t) {
    // SAFETY: `isolate` created a dedicated group whose ID is the child PID. A
    // negative PID targets only that group. Reaping the group on both future
    // drop and normal direct-child completion prevents background descendants
    // from outliving the bounded command.
    unsafe {
        libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: Job Object handles are process-wide kernel references and this
// wrapper has unique ownership, so moving it with the execution future cannot
// duplicate a close or invalidate the handle.
#[cfg(windows)]
unsafe impl Send for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    fn attach(pid: u32) -> std::io::Result<Self> {
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

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        unsafe {
            // Termination is idempotent for an already-empty job and closes the
            // cancellation race before the last job handle is released.
            TerminateJobObject(self.0, 1);
            CloseHandle(self.0);
        }
    }
}
