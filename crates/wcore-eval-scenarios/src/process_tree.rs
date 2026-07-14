//! Process-tree ownership for evaluator child processes.
//!
//! Linux uses a private cgroup v2 and moves the child into it from `pre_exec`,
//! before candidate code can fork. Windows launches suspended and assigns the
//! child to a kill-on-close Job Object before resuming its primary thread.
//! Other Unix hosts use an observed process-group fallback; it remains
//! non-authoritative because a hostile descendant can leave that group.

use std::io;
use std::process::ExitStatus;
use std::time::Duration;

use tokio::process::{Child, Command};

#[cfg(target_os = "linux")]
use linux::Cgroup;
#[cfg(windows)]
use windows::WindowsJob;

#[derive(Debug)]
pub(crate) struct ProcessTree {
    backend: Backend,
    root_pid: Option<u32>,
    cleanup_complete: bool,
}

#[derive(Debug)]
enum Backend {
    #[cfg(target_os = "linux")]
    Cgroup(Cgroup),
    #[cfg(unix)]
    ProcessGroup,
    #[cfg(windows)]
    WindowsJob(WindowsJob),
    #[cfg(not(any(unix, windows)))]
    DirectChild,
}

impl ProcessTree {
    pub(crate) fn backend_name(&self) -> &'static str {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(_) => "cgroup-v2",
            #[cfg(unix)]
            Backend::ProcessGroup => "process-group-observed-nonauthoritative",
            #[cfg(windows)]
            Backend::WindowsJob(_) => "windows-job-object",
            #[cfg(not(any(unix, windows)))]
            Backend::DirectChild => "direct-child-best-effort",
        }
    }

    pub(crate) fn root_pid(&self) -> Option<u32> {
        self.root_pid
    }

    pub(crate) fn is_authoritative(&self) -> bool {
        #[cfg(target_os = "linux")]
        if matches!(&self.backend, Backend::Cgroup(_)) {
            return true;
        }
        #[cfg(windows)]
        return matches!(&self.backend, Backend::WindowsJob(_));
        #[cfg(not(windows))]
        false
    }

    pub(crate) fn prepare() -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            match Cgroup::create() {
                Ok(cgroup) => {
                    return Ok(Self {
                        backend: Backend::Cgroup(cgroup),
                        root_pid: None,
                        cleanup_complete: false,
                    });
                }
                Err(error) if authoritative_required() => return Err(error),
                Err(_) => {}
            }
        }

        #[cfg(all(not(target_os = "linux"), not(windows)))]
        if authoritative_required() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "authoritative process-tree containment is unavailable on this platform",
            ));
        }

        #[cfg(unix)]
        let backend = Backend::ProcessGroup;
        #[cfg(windows)]
        let backend = Backend::WindowsJob(WindowsJob::create()?);
        #[cfg(not(any(unix, windows)))]
        let backend = Backend::DirectChild;
        Ok(Self {
            backend,
            root_pid: None,
            cleanup_complete: false,
        })
    }

    pub(crate) fn configure(&self, command: &mut Command) -> io::Result<()> {
        command.kill_on_drop(true);
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.configure(command),
            #[cfg(unix)]
            Backend::ProcessGroup => {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
                Ok(())
            }
            #[cfg(windows)]
            Backend::WindowsJob(job) => job.configure(command),
            #[cfg(not(any(unix, windows)))]
            Backend::DirectChild => Ok(()),
        }
    }

    pub(crate) async fn bind(&mut self, child: &mut Child) -> io::Result<()> {
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("spawned evaluator child had no process id"))?;
        // Keep the unreaped direct child as the identity anchor while binding.
        // Even an immediate exit cannot recycle this PID until `reap_child`.
        self.root_pid = Some(pid);
        #[cfg(windows)]
        if let Backend::WindowsJob(job) = &self.backend
            && let Err(bind_error) = job.assign_and_resume(child, pid)
        {
            let cleanup_error = self.abort_failed_bind(child).await.err();
            return Err(match cleanup_error {
                Some(cleanup_error) => io::Error::other(format!(
                    "{bind_error}; suspended child cleanup failed: {cleanup_error}"
                )),
                None => bind_error,
            });
        }
        Ok(())
    }

    /// Force the owned tree down, reap the direct child, and prove the kernel
    /// containment is empty before returning success.
    pub(crate) async fn terminate(&mut self, child: &mut Child) -> io::Result<()> {
        #[cfg(unix)]
        let process_group = matches!(&self.backend, Backend::ProcessGroup);
        #[cfg(not(unix))]
        let process_group = false;
        let tree_error = self.kill_tree().err();
        let child_kill_error = child.start_kill().err();
        let reap_error = self.reap_child(child).await.err();
        let child_kill_error = reap_error.as_ref().and(child_kill_error);
        let verify_error = if process_group {
            // The leader's unreaped PID anchored the group while SIGKILL was
            // sent. Never address the numeric PGID again after reaping: it can
            // be recycled for an unrelated process group.
            self.cleanup_complete = true;
            None
        } else {
            self.finish_cleanup().await.err()
        };
        let result = combine_cleanup_errors(tree_error, child_kill_error, reap_error, verify_error);
        if result.is_ok() {
            self.cleanup_complete = true;
        }
        result
    }

    /// Wait for a normal exit without surrendering a Unix process-group
    /// leader's identity, then clean descendants and reap the direct child.
    pub(crate) async fn wait_for_exit_and_cleanup(
        &mut self,
        child: &mut Child,
        timeout: Duration,
    ) -> io::Result<Option<(ExitStatus, Option<io::Error>)>> {
        #[cfg(unix)]
        if matches!(&self.backend, Backend::ProcessGroup) {
            let pid = self
                .root_pid
                .ok_or_else(|| io::Error::other("process-group child had no anchored PID"))?;
            let group = UnixProcessGroup::from_pid(pid)?;
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if group.child_exited_unreaped()? {
                    let cleanup_error = group.kill().err();
                    self.cleanup_complete = true;
                    let status = child.wait().await?;
                    // Group ownership is non-authoritative, but the anchored
                    // cleanup attempt is complete. Do not risk a stale-PGID
                    // retry from Drop after the leader has been reaped.
                    return Ok(Some((status, cleanup_error)));
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(status)) => {
                let tree_error = self.kill_tree().err();
                let verify_error = self.finish_cleanup().await.err();
                let cleanup_error =
                    combine_cleanup_errors(tree_error, None, None, verify_error).err();
                if cleanup_error.is_none() {
                    self.cleanup_complete = true;
                }
                Ok(Some((status, cleanup_error)))
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Ok(None),
        }
    }

    fn kill_tree(&self) -> io::Result<()> {
        if self.cleanup_complete {
            return Ok(());
        }
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.kill(),
            #[cfg(unix)]
            Backend::ProcessGroup => {
                if let Some(pid) = self.root_pid {
                    UnixProcessGroup::from_pid(pid)?.kill()?;
                }
                Ok(())
            }
            #[cfg(windows)]
            Backend::WindowsJob(job) => job.terminate(),
            #[cfg(not(any(unix, windows)))]
            Backend::DirectChild => Ok(()),
        }
    }

    async fn finish_cleanup(&mut self) -> io::Result<()> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.wait_empty_and_remove().await,
            #[cfg(unix)]
            Backend::ProcessGroup => Ok(()),
            #[cfg(windows)]
            Backend::WindowsJob(job) => job.wait_empty().await,
            #[cfg(not(any(unix, windows)))]
            Backend::DirectChild => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "direct-child cleanup cannot verify descendant absence",
            )),
        }
    }

    #[cfg(windows)]
    async fn abort_failed_bind(&mut self, child: &mut Child) -> io::Result<()> {
        let tree_error = self.kill_tree().err();
        let child_kill_error = child.start_kill().err();
        let reap_error = self.reap_child(child).await.err();
        let child_kill_error = reap_error.as_ref().and(child_kill_error);
        let verify_error = self.finish_cleanup().await.err();
        combine_cleanup_errors(tree_error, child_kill_error, reap_error, verify_error)
    }

    async fn reap_child(&self, child: &mut Child) -> io::Result<()> {
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "direct evaluator child was not reaped within 5 seconds",
            )),
        }
    }
}

fn combine_cleanup_errors(
    tree: Option<io::Error>,
    child_kill: Option<io::Error>,
    reap: Option<io::Error>,
    verify: Option<io::Error>,
) -> io::Result<()> {
    let errors = [tree, child_kill, reap, verify]
        .into_iter()
        .flatten()
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = self.kill_tree();
        #[cfg(target_os = "linux")]
        if let Backend::Cgroup(cgroup) = &mut self.backend {
            let _ = cgroup.remove_if_empty();
        }
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnixProcessGroup {
    id: libc::pid_t,
}

#[cfg(unix)]
impl UnixProcessGroup {
    pub(crate) fn from_pid(pid: u32) -> io::Result<Self> {
        let id = libc::pid_t::try_from(pid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "child PID exceeds pid_t"))?;
        if id <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "child PID must be positive",
            ));
        }
        Ok(Self { id })
    }

    pub(crate) fn verify_session_leader(&self) -> io::Result<()> {
        // SAFETY: both calls only inspect the live child identified by `id`.
        let session = unsafe { libc::getsid(self.id) };
        if session == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: getpgid only inspects the live child identified by `id`.
        let group = unsafe { libc::getpgid(self.id) };
        if group == -1 {
            return Err(io::Error::last_os_error());
        }
        if session != self.id || group != self.id {
            return Err(io::Error::other(format!(
                "PTY child {} did not become its session/process-group leader (sid={session}, pgid={group})",
                self.id
            )));
        }
        Ok(())
    }

    pub(crate) fn kill(&self) -> io::Result<()> {
        // SAFETY: the negative id addresses only the evaluator-owned group.
        let rc = unsafe { libc::kill(-self.id, libc::SIGKILL) };
        if rc == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    pub(crate) fn child_exited_unreaped(&self) -> io::Result<bool> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is writable and P_PID scopes observation to the
        // evaluator-owned direct child. WNOWAIT preserves the zombie as the
        // PID/PGID identity anchor until group cleanup has been issued.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                self.id as libc::id_t,
                &raw mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: waitid initialized siginfo for either the requested child or
        // the WNOHANG no-state-change result, whose si_pid is zero.
        Ok(unsafe { info.si_pid() } == self.id)
    }
}

pub(crate) fn authoritative_required() -> bool {
    std::env::var_os("WCORE_EVAL_REQUIRE_CONTAINMENT").is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::mem;
    use std::ptr;
    use std::time::{Duration, Instant};

    use tokio::process::{Child, Command};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    #[derive(Debug)]
    pub(super) struct WindowsJob(HANDLE);

    // SAFETY: this wrapper uniquely owns a process-wide kernel handle.
    unsafe impl Send for WindowsJob {}

    impl WindowsJob {
        pub(super) fn create() -> io::Result<Self> {
            // SAFETY: null security attributes and name request an unnamed Job.
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` is initialized for the exact information class.
            let configured = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(super) fn configure(&self, command: &mut Command) -> io::Result<()> {
            use std::os::windows::process::CommandExt;
            command.as_std_mut().creation_flags(CREATE_SUSPENDED);
            Ok(())
        }

        pub(super) fn assign_and_resume(
            &self,
            child: &Child,
            suspended_pid: u32,
        ) -> io::Result<()> {
            let process = child.raw_handle().ok_or_else(|| {
                io::Error::other("suspended evaluator child had no process handle")
            })? as HANDLE;
            // SAFETY: Tokio retains this exact process handle until the child is
            // reaped. The child is still suspended, so it cannot fork first.
            if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut in_job = 0;
            // SAFETY: both handles identify the still-suspended child and this
            // evaluator-owned Job. This check happens before any ResumeThread.
            if unsafe { IsProcessInJob(process, self.0, &raw mut in_job) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if in_job == 0 || self.active_processes()? != 1 {
                return Err(io::Error::other(
                    "suspended evaluator child was not the sole active Job member",
                ));
            }
            resume_only_thread(suspended_pid)
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            // SAFETY: this wrapper owns a valid Job handle.
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub(super) async fn wait_empty(&mut self) -> io::Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if self.active_processes()? == 0 {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Windows evaluator Job remained populated",
                    ));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        fn active_processes(&self) -> io::Result<u32> {
            let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { mem::zeroed() };
            // SAFETY: `accounting` is initialized for the exact query class.
            let queried = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    (&raw mut accounting).cast(),
                    mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    ptr::null_mut(),
                )
            };
            if queried == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(accounting.ActiveProcesses)
            }
        }
    }

    impl Drop for WindowsJob {
        fn drop(&mut self) {
            // SAFETY: termination closes the final cancellation race; closing a
            // KILL_ON_JOB_CLOSE handle is the kernel-enforced fallback.
            unsafe {
                TerminateJobObject(self.0, 1);
                CloseHandle(self.0);
            }
        }
    }

    fn resume_only_thread(pid: u32) -> io::Result<()> {
        // SAFETY: the snapshot handle is closed on every path below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        let thread_id = (|| {
            let mut entry: THREADENTRY32 = unsafe { mem::zeroed() };
            entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
            // SAFETY: snapshot and entry are valid.
            if unsafe { Thread32First(snapshot, &raw mut entry) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut found = None;
            loop {
                if entry.th32OwnerProcessID == pid {
                    if found.replace(entry.th32ThreadID).is_some() {
                        return Err(io::Error::other(
                            "suspended evaluator child unexpectedly had multiple threads",
                        ));
                    }
                }
                // SAFETY: clear stale state so end-of-snapshot is distinguishable
                // from a real enumeration failure.
                unsafe { SetLastError(0) };
                // SAFETY: snapshot and entry are valid.
                if unsafe { Thread32Next(snapshot, &raw mut entry) } == 0 {
                    // SAFETY: reads the calling thread's last-error slot.
                    let error = unsafe { GetLastError() };
                    if error != ERROR_NO_MORE_FILES {
                        return Err(io::Error::from_raw_os_error(error as i32));
                    }
                    break;
                }
            }
            found.ok_or_else(|| io::Error::other("suspended evaluator thread was not found"))
        })();
        // SAFETY: snapshot was returned by CreateToolhelp32Snapshot.
        unsafe { CloseHandle(snapshot) };
        let thread_id = thread_id?;

        // SAFETY: the suspended process cannot exit or replace its sole thread
        // before it is resumed; OpenThread returns a separately owned handle.
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if thread.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `thread` has THREAD_SUSPEND_RESUME access.
        let previous = unsafe { ResumeThread(thread) };
        // SAFETY: thread was returned by OpenThread.
        unsafe { CloseHandle(thread) };
        if previous == 1 {
            Ok(())
        } else if previous == u32::MAX {
            Err(io::Error::last_os_error())
        } else {
            Err(io::Error::other(format!(
                "suspended evaluator thread had unexpected suspend count {previous}"
            )))
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read};
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use tokio::process::Command;

    static CGROUP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug)]
    pub(super) struct Cgroup {
        path: PathBuf,
        procs: File,
        removed: bool,
    }

    impl Cgroup {
        pub(super) fn create() -> io::Result<Self> {
            let self_cgroup = std::fs::read_to_string("/proc/self/cgroup")?;
            let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")?;
            let current = unified_path(&self_cgroup)?;
            let parent = cgroup_directory(&mountinfo, &current)?;
            let sequence = CGROUP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("wayland-eval-{}-{sequence}", std::process::id()));
            std::fs::create_dir(&path)?;

            let result = (|| {
                let procs = OpenOptions::new()
                    .write(true)
                    .open(path.join("cgroup.procs"))?;
                OpenOptions::new()
                    .write(true)
                    .open(path.join("cgroup.kill"))?;
                File::open(path.join("cgroup.events"))?;
                Ok(Self {
                    path: path.clone(),
                    procs,
                    removed: false,
                })
            })();
            if result.is_err() {
                let _ = std::fs::remove_dir(&path);
            }
            result
        }

        pub(super) fn configure(&self, command: &mut Command) -> io::Result<()> {
            let procs = self.procs.try_clone()?;
            // SAFETY: the closure runs after fork and before exec. It performs
            // one async-signal-safe `write(2)` to an already-open fd and does
            // not allocate, lock, log, or access the filesystem by path.
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    let byte = b'0';
                    let written = libc::write(
                        procs.as_raw_fd(),
                        (&byte as *const u8).cast::<libc::c_void>(),
                        1,
                    );
                    if written == 1 {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                });
            }
            Ok(())
        }

        pub(super) fn kill(&self) -> io::Result<()> {
            match std::fs::write(self.path.join("cgroup.kill"), b"1") {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound && self.removed => Ok(()),
                Err(error) => Err(error),
            }
        }

        pub(super) async fn wait_empty_and_remove(&mut self) -> io::Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if !is_populated(&self.path.join("cgroup.events"))? {
                    self.remove_if_empty()?;
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("cgroup remained populated: {}", self.path.display()),
                    ));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        pub(super) fn remove_if_empty(&mut self) -> io::Result<()> {
            if !self.removed {
                std::fs::remove_dir(&self.path)?;
                self.removed = true;
            }
            Ok(())
        }
    }

    impl Drop for Cgroup {
        fn drop(&mut self) {
            if self.removed {
                return;
            }
            let path = self.path.clone();
            let _ = std::thread::Builder::new()
                .name("wcore-eval-cgroup-reaper".to_string())
                .spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    loop {
                        match is_populated(&path.join("cgroup.events")) {
                            Ok(false) => {
                                let _ = std::fs::remove_dir(&path);
                                break;
                            }
                            Ok(true) if Instant::now() < deadline => {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            Ok(true) | Err(_) => break,
                        }
                    }
                });
        }
    }

    fn unified_path(contents: &str) -> io::Result<PathBuf> {
        contents
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("no unified cgroup v2 entry in /proc/self/cgroup"))
    }

    fn cgroup_directory(mountinfo: &str, current: &Path) -> io::Result<PathBuf> {
        for line in mountinfo.lines() {
            let Some((before, after)) = line.split_once(" - ") else {
                continue;
            };
            if after.split_whitespace().next() != Some("cgroup2") {
                continue;
            }
            let fields: Vec<&str> = before.split_whitespace().collect();
            if fields.len() < 5 {
                continue;
            }
            let mount_root = Path::new(fields[3]);
            let mount_point = Path::new(fields[4]);
            let relative = current.strip_prefix(mount_root).map_err(|_| {
                io::Error::other(format!(
                    "current cgroup {} is outside mount root {}",
                    current.display(),
                    mount_root.display()
                ))
            })?;
            return Ok(mount_point.join(relative));
        }
        Err(io::Error::other(
            "no cgroup v2 mount in /proc/self/mountinfo",
        ))
    }

    fn is_populated(events: &Path) -> io::Result<bool> {
        let mut contents = String::new();
        File::open(events)?.read_to_string(&mut contents)?;
        contents
            .lines()
            .find_map(|line| line.strip_prefix("populated "))
            .map(|value| value.trim() != "0")
            .ok_or_else(|| io::Error::other("cgroup.events lacked populated field"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_current_path_through_mount_root() {
            let mountinfo = "31 24 0:27 /tenant /sys/fs/cgroup rw - cgroup2 cgroup2 rw";
            let path = cgroup_directory(mountinfo, Path::new("/tenant/session/core"))
                .expect("map cgroup path");
            assert_eq!(path, Path::new("/sys/fs/cgroup/session/core"));
        }

        #[test]
        fn parses_unified_self_cgroup_entry() {
            let path =
                unified_path("5:cpu:/legacy\n0::/user.slice/eval.scope\n").expect("unified entry");
            assert_eq!(path, Path::new("/user.slice/eval.scope"));
        }
    }
}
