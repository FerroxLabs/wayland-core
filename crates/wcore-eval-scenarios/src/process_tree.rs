//! Process-tree ownership for evaluator child processes.
//!
//! Linux uses a private cgroup v2 and moves the child into it from `pre_exec`,
//! before candidate code can fork. Windows launches suspended and assigns the
//! child to a kill-on-close Job Object before resuming its primary thread.
//! Other Unix hosts use an observed process-group fallback; it remains
//! non-authoritative because a hostile descendant can leave that group.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use tokio::process::{Child, Command};

#[cfg(target_os = "linux")]
use linux::Cgroup;
#[cfg(windows)]
use windows::WindowsJob;

#[cfg(target_os = "linux")]
pub(crate) async fn serialize_candidate_identity() -> tokio::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[derive(Debug)]
pub(crate) struct ProcessTree {
    backend: Backend,
    root_pid: Option<u32>,
    cleanup_complete: bool,
    peak_memory_bytes: Option<u64>,
    peak_cpu_millis: Option<u64>,
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

#[derive(Debug)]
pub(crate) struct PreparedExecutable {
    path: PathBuf,
    #[cfg(target_os = "linux")]
    file: Option<std::fs::File>,
}

impl PreparedExecutable {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(target_os = "linux")]
    fn raw_fd(&self) -> Option<std::os::fd::RawFd> {
        use std::os::fd::AsRawFd;

        self.file.as_ref().map(std::fs::File::as_raw_fd)
    }
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

    pub(crate) fn peak_memory_bytes(&self) -> Option<u64> {
        self.peak_memory_bytes
    }

    pub(crate) fn peak_cpu_millis(&self) -> Option<u64> {
        self.peak_cpu_millis
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
                        peak_memory_bytes: None,
                        peak_cpu_millis: None,
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
            peak_memory_bytes: None,
            peak_cpu_millis: None,
        })
    }

    pub(crate) fn configure(
        &self,
        command: &mut Command,
        executable: Option<&PreparedExecutable>,
    ) -> io::Result<()> {
        command.kill_on_drop(true);
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => {
                cgroup.configure(command, executable.and_then(PreparedExecutable::raw_fd))
            }
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

    pub(crate) fn prepare_workspace(&self, cwd: &Path) -> io::Result<()> {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.prepare_workspace(cwd),
            _ => Ok(()),
        }
    }

    pub(crate) fn prepare_executable(&self, binary: &Path) -> io::Result<PreparedExecutable> {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.prepare_executable(binary),
            _ => Ok(PreparedExecutable {
                path: binary.to_path_buf(),
                #[cfg(target_os = "linux")]
                file: None,
            }),
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
            Backend::Cgroup(cgroup) => {
                if let Ok(usage) = cgroup.resource_usage() {
                    self.peak_memory_bytes = Some(usage.peak_memory_bytes);
                    self.peak_cpu_millis = Some(usage.cpu_millis);
                }
                cgroup.wait_empty_and_remove().await
            }
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
    use std::collections::VecDeque;
    use std::ffi::CString;
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    #[cfg(test)]
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use tokio::process::Command;

    static CGROUP_COUNTER: AtomicU64 = AtomicU64::new(0);
    static MATERIALIZED_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
    const CLEANUP_QUEUE_CAPACITY: usize = 1_024;
    const MAX_RECORDED_FAILURES: usize = 64;

    static CLEANUP_REAPER: OnceLock<Result<CleanupReaper, String>> = OnceLock::new();

    #[derive(Debug)]
    enum ReaperMessage {
        Cleanup(PathBuf),
        #[cfg(test)]
        Flush(mpsc::Sender<()>),
    }

    #[derive(Debug)]
    struct PendingCleanup {
        path: PathBuf,
        deadline: Instant,
    }

    #[derive(Debug)]
    struct CleanupReaper {
        sender: SyncSender<ReaperMessage>,
        failures: Arc<Mutex<VecDeque<String>>>,
        #[cfg(test)]
        worker_count: Arc<AtomicUsize>,
    }

    impl CleanupReaper {
        fn spawn() -> io::Result<Self> {
            Self::spawn_with_capacity(CLEANUP_QUEUE_CAPACITY)
        }

        fn spawn_with_capacity(capacity: usize) -> io::Result<Self> {
            let (sender, receiver) = mpsc::sync_channel(capacity);
            let failures = Arc::new(Mutex::new(VecDeque::new()));
            let worker_failures = Arc::clone(&failures);
            #[cfg(test)]
            let worker_count = Arc::new(AtomicUsize::new(0));
            #[cfg(test)]
            let thread_worker_count = Arc::clone(&worker_count);
            std::thread::Builder::new()
                .name("wcore-eval-cgroup-reaper".to_string())
                .spawn(move || {
                    #[cfg(test)]
                    thread_worker_count.fetch_add(1, Ordering::SeqCst);
                    cleanup_worker(receiver, &worker_failures);
                    #[cfg(test)]
                    thread_worker_count.fetch_sub(1, Ordering::SeqCst);
                })?;
            Ok(Self {
                sender,
                failures,
                #[cfg(test)]
                worker_count,
            })
        }

        fn enqueue(&self, path: PathBuf) -> io::Result<()> {
            match self.sender.try_send(ReaperMessage::Cleanup(path)) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(ReaperMessage::Cleanup(path))) => {
                    let message = format!(
                        "cgroup cleanup queue saturated before accepting {}",
                        path.display()
                    );
                    record_failure(&self.failures, message.clone());
                    Err(io::Error::other(message))
                }
                Err(TrySendError::Disconnected(ReaperMessage::Cleanup(path))) => {
                    let message = format!(
                        "cgroup cleanup worker disconnected before accepting {}",
                        path.display()
                    );
                    record_failure(&self.failures, message.clone());
                    Err(io::Error::other(message))
                }
                #[cfg(test)]
                Err(TrySendError::Full(ReaperMessage::Flush(_)))
                | Err(TrySendError::Disconnected(ReaperMessage::Flush(_))) => {
                    unreachable!("enqueue sends only cleanup messages")
                }
            }
        }

        fn check_failures(&self) -> io::Result<()> {
            let failures = self
                .failures
                .lock()
                .map_err(|_| io::Error::other("cgroup cleanup failure registry was poisoned"))?;
            if failures.is_empty() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "prior cgroup cleanup failures: {}",
                    failures.iter().cloned().collect::<Vec<_>>().join("; ")
                )))
            }
        }

        #[cfg(test)]
        fn flush(&self) -> io::Result<()> {
            let (sender, receiver) = mpsc::channel();
            self.sender
                .send(ReaperMessage::Flush(sender))
                .map_err(|_| io::Error::other("cgroup cleanup worker disconnected"))?;
            receiver
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "cleanup flush timed out"))
        }
    }

    fn cleanup_reaper() -> io::Result<&'static CleanupReaper> {
        match CLEANUP_REAPER
            .get_or_init(|| CleanupReaper::spawn().map_err(|error| error.to_string()))
        {
            Ok(reaper) => Ok(reaper),
            Err(error) => Err(io::Error::other(format!(
                "could not start cgroup cleanup worker: {error}"
            ))),
        }
    }

    fn cleanup_worker(receiver: Receiver<ReaperMessage>, failures: &Arc<Mutex<VecDeque<String>>>) {
        let mut pending = Vec::<PendingCleanup>::new();
        #[cfg(test)]
        let mut flush_waiters = Vec::<mpsc::Sender<()>>::new();
        let mut disconnected = false;

        loop {
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(message) => accept_message(
                    message,
                    &mut pending,
                    #[cfg(test)]
                    &mut flush_waiters,
                ),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
            }
            loop {
                match receiver.try_recv() {
                    Ok(message) => accept_message(
                        message,
                        &mut pending,
                        #[cfg(test)]
                        &mut flush_waiters,
                    ),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            let now = Instant::now();
            pending.retain(|task| match cleanup_path(&task.path) {
                Ok(true) => false,
                Ok(false) if now < task.deadline => true,
                Ok(false) => {
                    record_failure(
                        failures,
                        format!("cgroup remained populated: {}", task.path.display()),
                    );
                    false
                }
                Err(error) => {
                    record_failure(
                        failures,
                        format!("could not clean cgroup {}: {error}", task.path.display()),
                    );
                    false
                }
            });

            #[cfg(test)]
            if pending.is_empty() {
                for waiter in flush_waiters.drain(..) {
                    let _ = waiter.send(());
                }
            }
            if disconnected && pending.is_empty() {
                break;
            }
        }
    }

    fn accept_message(
        message: ReaperMessage,
        pending: &mut Vec<PendingCleanup>,
        #[cfg(test)] flush_waiters: &mut Vec<mpsc::Sender<()>>,
    ) {
        match message {
            ReaperMessage::Cleanup(path) => pending.push(PendingCleanup {
                path,
                deadline: Instant::now() + Duration::from_secs(5),
            }),
            #[cfg(test)]
            ReaperMessage::Flush(waiter) => flush_waiters.push(waiter),
        }
    }

    fn cleanup_path(path: &Path) -> io::Result<bool> {
        if !path.exists() {
            return Ok(true);
        }
        if is_populated(&path.join("cgroup.events"))? {
            return Ok(false);
        }
        std::fs::remove_dir(path)?;
        Ok(true)
    }

    fn record_failure(failures: &Arc<Mutex<VecDeque<String>>>, message: String) {
        tracing::error!(target: "wcore_eval", error = %message, "persistent cgroup cleanup failure");
        if let Ok(mut failures) = failures.lock() {
            if failures.len() == MAX_RECORDED_FAILURES {
                failures.pop_front();
            }
            failures.push_back(message);
        }
    }

    #[derive(Debug)]
    pub(super) struct Cgroup {
        path: PathBuf,
        procs: File,
        identity: CandidateIdentity,
        _identity_lock: File,
        removed: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct CandidateIdentity {
        uid: libc::uid_t,
        gid: libc::gid_t,
    }

    impl CandidateIdentity {
        fn from_environment() -> io::Result<Self> {
            if unsafe { libc::geteuid() } != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "authoritative Linux evaluation requires a privileged supervisor that can drop the candidate identity",
                ));
            }
            let uid = parse_identity("WCORE_EVAL_CANDIDATE_UID")?;
            let gid = parse_identity("WCORE_EVAL_CANDIDATE_GID")?;
            if uid == 0 || gid == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "evaluator candidate UID and GID must both be non-zero",
                ));
            }
            if uid == unsafe { libc::getuid() } || gid == unsafe { libc::getgid() } {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "evaluator candidate identity must differ from the supervisor identity",
                ));
            }
            Ok(Self { uid, gid })
        }

        fn drop_in_child(self) -> io::Result<()> {
            // SAFETY: use raw Linux syscalls rather than glibc's credential
            // wrappers. The wrappers coordinate credentials across threads and
            // are not safe in a post-fork child of the Tokio supervisor.
            unsafe {
                raw_child_syscall(
                    libc::SYS_prctl,
                    &[libc::PR_SET_KEEPCAPS as libc::c_long, 0, 0, 0, 0],
                )?;
                raw_child_syscall(
                    libc::SYS_prctl,
                    &[
                        libc::PR_CAP_AMBIENT as libc::c_long,
                        libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_long,
                        0,
                        0,
                        0,
                    ],
                )?;
                raw_child_syscall(libc::SYS_setgroups, &[0, 0])?;
                raw_child_syscall(
                    libc::SYS_setresgid,
                    &[self.gid.into(), self.gid.into(), self.gid.into()],
                )?;
                raw_child_syscall(
                    libc::SYS_setresuid,
                    &[self.uid.into(), self.uid.into(), self.uid.into()],
                )?;
                raw_child_syscall(
                    libc::SYS_prctl,
                    &[libc::PR_SET_NO_NEW_PRIVS as libc::c_long, 1, 0, 0, 0],
                )?;
                if libc::syscall(libc::SYS_getuid) as libc::uid_t != self.uid
                    || libc::syscall(libc::SYS_geteuid) as libc::uid_t != self.uid
                    || libc::syscall(libc::SYS_getgid) as libc::gid_t != self.gid
                    || libc::syscall(libc::SYS_getegid) as libc::gid_t != self.gid
                {
                    return Err(io::Error::from_raw_os_error(libc::EPERM));
                }
            }
            Ok(())
        }

        fn acquire_exclusive_lock(self) -> io::Result<File> {
            let path = CString::new(format!(
                "/run/wayland-eval-identity-{}-{}.lock",
                self.uid, self.gid
            ))
            .expect("numeric identity lock path has no NUL");
            // SAFETY: `/run` is the root-owned runtime directory. O_NOFOLLOW
            // rejects a pre-created symlink and the post-open checks reject any
            // non-root-owned or group/world-writable object.
            let fd = unsafe {
                libc::open(
                    path.as_ptr(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if fd == -1 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `open` returned a new owned descriptor.
            let file = unsafe { File::from_raw_fd(fd) };
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: `stat` is a valid output pointer for this open descriptor.
            if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fstat initialized `stat` on success.
            let stat = unsafe { stat.assume_init() };
            if stat.st_uid != 0
                || stat.st_mode & libc::S_IFMT != libc::S_IFREG
                || stat.st_mode & 0o022 != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "candidate identity lock was not a root-owned private regular file",
                ));
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                // SAFETY: flock applies to the open file description retained
                // by Cgroup for the complete authoritative run.
                if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    break;
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::WouldBlock {
                    return Err(error);
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "candidate identity remained assigned to another evaluator for 30 seconds",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            ensure_identity_inactive(self)?;
            Ok(file)
        }
    }

    unsafe fn raw_child_syscall(number: libc::c_long, args: &[libc::c_long]) -> io::Result<()> {
        let result = match args {
            [] => unsafe { libc::syscall(number) },
            [a] => unsafe { libc::syscall(number, *a) },
            [a, b] => unsafe { libc::syscall(number, *a, *b) },
            [a, b, c] => unsafe { libc::syscall(number, *a, *b, *c) },
            [a, b, c, d] => unsafe { libc::syscall(number, *a, *b, *c, *d) },
            [a, b, c, d, e] => unsafe { libc::syscall(number, *a, *b, *c, *d, *e) },
            _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
        };
        if result == -1 {
            // SAFETY: errno is thread-local and reading it does not allocate or
            // acquire a process-shared lock in the post-fork child.
            Err(io::Error::from_raw_os_error(unsafe {
                *libc::__errno_location()
            }))
        } else {
            Ok(())
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct ResourceUsage {
        pub(super) peak_memory_bytes: u64,
        pub(super) cpu_millis: u64,
    }

    impl Cgroup {
        pub(super) fn create() -> io::Result<Self> {
            let reaper = cleanup_reaper()?;
            reaper.check_failures()?;
            let identity = CandidateIdentity::from_environment()?;
            let identity_lock = identity.acquire_exclusive_lock()?;
            let self_cgroup = std::fs::read_to_string("/proc/self/cgroup")?;
            let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")?;
            let current = unified_path(&self_cgroup)?;
            let current = cgroup_directory(&mountinfo, &current)?;
            let parent = delegated_parent(&current)?;
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
                    identity,
                    _identity_lock: identity_lock,
                    removed: false,
                })
            })();
            if result.is_err() {
                let _ = std::fs::remove_dir(&path);
            }
            result
        }

        pub(super) fn configure(
            &self,
            command: &mut Command,
            executable_fd: Option<libc::c_int>,
        ) -> io::Result<()> {
            executable_fd.ok_or_else(|| {
                io::Error::other("authoritative Linux evaluation lacked a pinned executable")
            })?;
            let procs = self.procs.try_clone()?;
            let identity = self.identity;
            // SAFETY: the closure runs after fork and before exec. It performs
            // only async-signal-safe syscalls against already-open fds before
            // dropping identity. It does not lock, log, or access a path.
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    let byte = b'0';
                    let written = libc::write(
                        procs.as_raw_fd(),
                        (&byte as *const u8).cast::<libc::c_void>(),
                        1,
                    );
                    if written != 1 {
                        return Err(io::Error::last_os_error());
                    }
                    identity.drop_in_child()
                });
            }
            Ok(())
        }

        pub(super) fn prepare_workspace(&self, cwd: &Path) -> io::Result<()> {
            chown_tree_without_following(cwd, self.identity)
        }

        pub(super) fn prepare_executable(
            &self,
            binary: &Path,
        ) -> io::Result<super::PreparedExecutable> {
            prepare_pinned_executable(binary)
        }

        pub(super) fn kill(&self) -> io::Result<()> {
            match std::fs::write(self.path.join("cgroup.kill"), b"1") {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound && self.removed => Ok(()),
                Err(error) => Err(error),
            }
        }

        pub(super) fn resource_usage(&self) -> io::Result<ResourceUsage> {
            let memory_peak = std::fs::read_to_string(self.path.join("memory.peak"))?;
            let cpu_stat = std::fs::read_to_string(self.path.join("cpu.stat"))?;
            Ok(ResourceUsage {
                peak_memory_bytes: parse_single_u64("memory.peak", &memory_peak)?,
                cpu_millis: parse_cpu_usage_micros(&cpu_stat)? / 1_000,
            })
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
            if let Err(error) = cleanup_reaper().and_then(|reaper| reaper.enqueue(path)) {
                tracing::error!(target: "wcore_eval", error = %error, "could not enqueue cgroup cleanup");
            }
        }
    }

    fn parse_identity(name: &str) -> io::Result<u32> {
        let value = std::env::var_os(name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} is required for authoritative Linux evaluation"),
            )
        })?;
        value.to_string_lossy().parse::<u32>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a decimal numeric identity"),
            )
        })
    }

    fn prepare_pinned_executable(binary: &Path) -> io::Result<super::PreparedExecutable> {
        let file = File::open(binary)?;
        let fd = file.as_raw_fd();
        // The kernel resolves this procfs path before closing CLOEXEC
        // descriptors during exec. The candidate therefore starts from the
        // pinned inode without inheriting the descriptor afterward.
        let path = PathBuf::from(format!("/proc/self/fd/{fd}"));
        Ok(super::PreparedExecutable {
            path,
            file: Some(file),
        })
    }

    fn chown_tree_without_following(root: &Path, identity: CandidateIdentity) -> io::Result<()> {
        let root = CString::new(root.as_os_str().as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace path contained an interior NUL byte",
            )
        })?;
        // SAFETY: O_NOFOLLOW rejects a symlink root; the returned descriptor
        // pins the directory against rename/replacement for the complete walk.
        let fd = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `open` returned a new owned descriptor.
        let root = unsafe { File::from_raw_fd(fd) };
        chown_directory_by_fd(&root, identity)
    }

    fn chown_directory_by_fd(directory: &File, identity: CandidateIdentity) -> io::Result<()> {
        let fd = directory.as_raw_fd();
        for entry in std::fs::read_dir(format!("/proc/self/fd/{fd}"))? {
            let name = entry?.file_name();
            let name = CString::new(name.as_bytes()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "workspace entry contained an interior NUL byte",
                )
            })?;
            #[cfg(test)]
            pause_before_directory_open(name.as_bytes());
            // O_PATH pins the exact directory entry without opening a FIFO,
            // device, or other hostile special file. All metadata and
            // ownership operations below address this descriptor, never the
            // replaceable parent/name pair.
            let entry_fd = unsafe {
                libc::openat(
                    fd,
                    name.as_ptr(),
                    libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if entry_fd == -1 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: openat returned a new owned descriptor.
            let entry = unsafe { File::from_raw_fd(entry_fd) };
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: entry is an open descriptor and stat points to writable
            // storage for the kernel result.
            if unsafe { libc::fstat(entry.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fstat initialized `stat` on success.
            let stat = unsafe { stat.assume_init() };
            if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
                // Resolve `.` from the pinned O_PATH directory descriptor so
                // rename or replacement of its former name cannot redirect
                // traversal.
                let child_fd = unsafe {
                    libc::openat(
                        entry.as_raw_fd(),
                        c".".as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    )
                };
                if child_fd == -1 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: openat returned a new owned descriptor.
                let child = unsafe { File::from_raw_fd(child_fd) };
                chown_directory_by_fd(&child, identity)?;
            } else if stat.st_mode & libc::S_IFMT == libc::S_IFREG {
                if stat.st_nlink != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "workspace regular file has hard-link aliases",
                    ));
                }
                #[cfg(test)]
                pause_before_regular_materialization(name.as_bytes());
                materialize_regular_file(fd, &name, &entry, &stat, identity)?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "workspace contains an unsupported special file",
                ));
            }
        }
        // Chown the directory only after all children. This keeps an
        // unprivileged candidate from gaining mutation rights mid-walk.
        if unsafe { libc::fchown(fd, identity.uid, identity.gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn materialize_regular_file(
        parent_fd: libc::c_int,
        name: &std::ffi::CStr,
        pinned: &File,
        stat: &libc::stat,
        identity: CandidateIdentity,
    ) -> io::Result<()> {
        // Reopen the exact inode behind the pinned O_PATH descriptor for
        // reading. No replaceable workspace path is resolved here.
        let mut source = File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))?;
        let (temporary_name, mut materialized) = create_materialized_file(parent_fd)?;
        let materialize_result = (|| {
            io::copy(&mut source, &mut materialized)?;
            materialized.sync_all()?;
            if unsafe { libc::fchown(materialized.as_raw_fd(), identity.uid, identity.gid) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // Preserve ordinary access semantics but never reproduce
            // set-user-ID, set-group-ID, or sticky privilege-bearing bits.
            let mode = stat.st_mode & 0o777;
            if unsafe { libc::fchmod(materialized.as_raw_fd(), mode) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // Exchange is atomic: the private candidate-owned inode becomes
            // the workspace entry while the original caller-owned inode moves
            // to the temporary name. Any hard-link alias created after the
            // initial nlink check remains attached only to that unmodified
            // original inode.
            exchange_entries(parent_fd, &temporary_name, name)?;
            #[cfg(test)]
            pause_after_materialization_exchange(name.to_bytes());
            let displaced = match metadata_at(parent_fd, &temporary_name) {
                Ok(displaced) => displaced,
                Err(error) => {
                    return restore_exchange(parent_fd, &temporary_name, name, error);
                }
            };
            if displaced.st_dev != stat.st_dev || displaced.st_ino != stat.st_ino {
                return restore_exchange(
                    parent_fd,
                    &temporary_name,
                    name,
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "workspace entry changed during private materialization",
                    ),
                );
            }
            if unsafe { libc::unlinkat(parent_fd, temporary_name.as_ptr(), 0) } != 0 {
                return restore_exchange(
                    parent_fd,
                    &temporary_name,
                    name,
                    io::Error::last_os_error(),
                );
            }
            Ok(())
        })();

        // Retain every error-path entry for recovery. There is no race-free
        // pathname unlink here: after an exchange or concurrent namespace
        // mutation, the temporary name may hold caller data.
        materialize_result
    }

    fn create_materialized_file(parent_fd: libc::c_int) -> io::Result<(CString, File)> {
        for _ in 0..128 {
            let sequence = MATERIALIZED_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = CString::new(format!(
                ".wcore-eval-materialize-{}-{sequence}",
                std::process::id()
            ))
            .expect("generated materialization name has no NUL");
            let fd = unsafe {
                libc::openat(
                    parent_fd,
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if fd != -1 {
                return Ok((name, unsafe { File::from_raw_fd(fd) }));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a private workspace materialization file",
        ))
    }

    fn metadata_at(parent_fd: libc::c_int, name: &std::ffi::CStr) -> io::Result<libc::stat> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                parent_fd,
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { stat.assume_init() })
    }

    fn exchange_entries(
        parent_fd: libc::c_int,
        left: &std::ffi::CStr,
        right: &std::ffi::CStr,
    ) -> io::Result<()> {
        if unsafe {
            libc::renameat2(
                parent_fd,
                left.as_ptr(),
                parent_fd,
                right.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn restore_exchange(
        parent_fd: libc::c_int,
        temporary_name: &std::ffi::CStr,
        original_name: &std::ffi::CStr,
        cause: io::Error,
    ) -> io::Result<()> {
        exchange_entries(parent_fd, temporary_name, original_name).map_err(|restore_error| {
            io::Error::other(format!(
                "{cause}; workspace materialization restoration failed: {restore_error}"
            ))
        })?;
        Err(cause)
    }

    #[cfg(test)]
    #[derive(Clone)]
    struct DirectoryOpenPause {
        name: Vec<u8>,
        reached: Arc<std::sync::Barrier>,
        resume: Arc<std::sync::Barrier>,
    }

    #[cfg(test)]
    static DIRECTORY_OPEN_PAUSE: OnceLock<Mutex<Option<DirectoryOpenPause>>> = OnceLock::new();

    #[cfg(test)]
    static REGULAR_MATERIALIZATION_PAUSE: OnceLock<Mutex<Option<DirectoryOpenPause>>> =
        OnceLock::new();

    #[cfg(test)]
    static MATERIALIZATION_EXCHANGE_PAUSE: OnceLock<Mutex<Option<DirectoryOpenPause>>> =
        OnceLock::new();

    #[cfg(test)]
    fn pause_before_directory_open(name: &[u8]) {
        let pause = DIRECTORY_OPEN_PAUSE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|pause| pause.as_ref().filter(|pause| pause.name == name).cloned());
        if let Some(pause) = pause {
            pause.reached.wait();
            pause.resume.wait();
        }
    }

    #[cfg(test)]
    fn pause_before_regular_materialization(name: &[u8]) {
        let pause = REGULAR_MATERIALIZATION_PAUSE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|pause| pause.as_ref().filter(|pause| pause.name == name).cloned());
        if let Some(pause) = pause {
            pause.reached.wait();
            pause.resume.wait();
        }
    }

    #[cfg(test)]
    fn pause_after_materialization_exchange(name: &[u8]) {
        let pause = MATERIALIZATION_EXCHANGE_PAUSE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|pause| pause.as_ref().filter(|pause| pause.name == name).cloned());
        if let Some(pause) = pause {
            pause.reached.wait();
            pause.resume.wait();
        }
    }

    fn ensure_identity_inactive(identity: CandidateIdentity) -> io::Result<()> {
        for entry in std::fs::read_dir("/proc")? {
            let entry = entry?;
            if entry
                .file_name()
                .as_bytes()
                .iter()
                .any(|byte| !byte.is_ascii_digit())
            {
                continue;
            }
            let status = match std::fs::read_to_string(entry.path().join("status")) {
                Ok(status) => status,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let uid_in_use = status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .is_some_and(|ids| {
                    ids.split_whitespace()
                        .filter_map(|id| id.parse::<u32>().ok())
                        .any(|id| id == identity.uid)
                });
            if uid_in_use {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "candidate UID already has a live process; authoritative isolation requires a dedicated idle identity",
                ));
            }
        }
        Ok(())
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

    fn delegated_parent(current: &Path) -> io::Result<PathBuf> {
        let Some(configured) = std::env::var_os("WCORE_EVAL_CGROUP_PARENT") else {
            return Ok(current.to_path_buf());
        };
        let configured = PathBuf::from(configured).canonicalize()?;
        let current = current.canonicalize()?;
        if current.parent() != Some(configured.as_path()) {
            return Err(io::Error::other(format!(
                "WCORE_EVAL_CGROUP_PARENT must be the direct parent of the evaluator cgroup; current={} configured={}",
                current.display(),
                configured.display()
            )));
        }
        let enabled = std::fs::read_to_string(configured.join("cgroup.subtree_control"))?;
        for required in ["cpu", "memory"] {
            if !enabled
                .split_whitespace()
                .any(|controller| controller == required)
            {
                return Err(io::Error::other(format!(
                    "delegated evaluator cgroup has not enabled the {required} controller"
                )));
            }
        }
        Ok(configured)
    }

    fn parse_single_u64(name: &str, contents: &str) -> io::Result<u64> {
        contents
            .trim()
            .parse()
            .map_err(|error| io::Error::other(format!("invalid {name}: {error}")))
    }

    fn parse_cpu_usage_micros(contents: &str) -> io::Result<u64> {
        contents
            .lines()
            .find_map(|line| line.strip_prefix("usage_usec "))
            .ok_or_else(|| io::Error::other("cpu.stat lacked usage_usec"))
            .and_then(|value| parse_single_u64("cpu.stat usage_usec", value))
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
        #[test]
        fn parses_kernel_resource_counters() {
            assert_eq!(parse_single_u64("memory.peak", "4096\n").unwrap(), 4096);
            assert_eq!(
                parse_cpu_usage_micros("usage_usec 9876\nuser_usec 9000\nsystem_usec 876\n")
                    .unwrap(),
                9876
            );
        }

        #[test]
        fn pinned_executable_is_cloexec_in_supervisor() {
            let executable = prepare_pinned_executable(Path::new("/proc/self/exe"))
                .expect("pin current executable");
            let fd = executable.raw_fd().expect("pinned executable fd");
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert_ne!(flags, -1);
            assert_ne!(flags & libc::FD_CLOEXEC, 0);
        }

        #[test]
        fn descriptor_walk_rejects_concurrent_directory_swap() {
            if unsafe { libc::geteuid() } != 0 {
                return;
            }
            use std::os::unix::fs::{MetadataExt, symlink};

            static TEST_SERIAL: Mutex<()> = Mutex::new(());
            let _serial = TEST_SERIAL.lock().expect("serialize swap hook");
            let fixture = tempfile::tempdir().expect("create ownership fixture");
            let workspace = fixture.path().join("workspace");
            let target = workspace.join("swap-target");
            let parked = workspace.join("parked-target");
            let outside = fixture.path().join("outside");
            std::fs::create_dir_all(target.join("nested")).expect("create workspace target");
            std::fs::create_dir_all(&outside).expect("create outside directory");
            let outside_file = outside.join("must-remain-root-owned");
            std::fs::write(&outside_file, b"host data").expect("seed outside file");
            let outside_uid = std::fs::symlink_metadata(&outside_file)
                .expect("outside metadata")
                .uid();

            let reached = Arc::new(std::sync::Barrier::new(2));
            let resume = Arc::new(std::sync::Barrier::new(2));
            *DIRECTORY_OPEN_PAUSE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .expect("install swap hook") = Some(DirectoryOpenPause {
                name: b"swap-target".to_vec(),
                reached: Arc::clone(&reached),
                resume: Arc::clone(&resume),
            });

            let worker = std::thread::spawn({
                let workspace = workspace.clone();
                move || {
                    chown_tree_without_following(
                        &workspace,
                        CandidateIdentity {
                            uid: 65_532,
                            gid: 65_532,
                        },
                    )
                }
            });
            reached.wait();
            std::fs::rename(&target, &parked).expect("replace inspected directory");
            symlink(&outside, &target).expect("install hostile replacement symlink");
            resume.wait();
            let error = worker
                .join()
                .expect("ownership worker did not panic")
                .expect_err("raced replacement must fail closed");
            assert!(
                error.kind() == io::ErrorKind::InvalidInput
                    || matches!(
                        error.raw_os_error(),
                        Some(libc::ELOOP) | Some(libc::ENOTDIR) | Some(libc::ENOENT)
                    ),
                "unexpected race error: {error}"
            );
            assert_eq!(
                std::fs::symlink_metadata(&outside_file)
                    .expect("outside metadata after attack")
                    .uid(),
                outside_uid,
                "descriptor walk must not chown outside the pinned workspace"
            );
            *DIRECTORY_OPEN_PAUSE
                .get()
                .expect("swap hook initialized")
                .lock()
                .expect("clear swap hook") = None;
        }

        #[test]
        fn descriptor_walk_rejects_hard_link_alias_without_touching_external_inode() {
            if unsafe { libc::geteuid() } != 0 {
                return;
            }
            use std::os::unix::fs::MetadataExt;

            let fixture = tempfile::tempdir().expect("create ownership fixture");
            let workspace = fixture.path().join("workspace");
            let outside = fixture.path().join("outside");
            std::fs::create_dir(&workspace).expect("create workspace");
            std::fs::create_dir(&outside).expect("create outside directory");
            let outside_file = outside.join("must-remain-supervisor-owned");
            let workspace_alias = workspace.join("hostile-hard-link");
            std::fs::write(&outside_file, b"host data").expect("seed outside file");
            std::fs::hard_link(&outside_file, &workspace_alias).expect("install hard-link alias");
            let before = std::fs::metadata(&outside_file).expect("outside metadata");
            let before_content = std::fs::read(&outside_file).expect("outside content");

            let error = chown_tree_without_following(
                &workspace,
                CandidateIdentity {
                    uid: 65_532,
                    gid: 65_532,
                },
            )
            .expect_err("multiply-linked file must fail closed");

            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("hard-link aliases"));
            let after = std::fs::metadata(&outside_file).expect("outside metadata after rejection");
            assert_eq!(after.uid(), before.uid(), "external owner UID changed");
            assert_eq!(after.gid(), before.gid(), "external owner GID changed");
            assert_eq!(
                std::fs::read(&outside_file).expect("outside content after rejection"),
                before_content,
                "external inode content changed"
            );
        }

        #[test]
        fn private_materialization_preserves_alias_created_after_link_check() {
            if unsafe { libc::geteuid() } != 0 {
                return;
            }
            use std::os::unix::fs::MetadataExt;

            let fixture = tempfile::tempdir().expect("create ownership fixture");
            let workspace = fixture.path().join("workspace");
            let outside = fixture.path().join("outside");
            std::fs::create_dir(&workspace).expect("create workspace");
            std::fs::create_dir(&outside).expect("create outside directory");
            let workspace_file = workspace.join("race-target");
            let outside_alias = outside.join("late-hard-link");
            std::fs::write(&workspace_file, b"host data").expect("seed workspace file");
            let before = std::fs::metadata(&workspace_file).expect("workspace metadata");
            let before_content = std::fs::read(&workspace_file).expect("workspace content");

            let reached = Arc::new(std::sync::Barrier::new(2));
            let resume = Arc::new(std::sync::Barrier::new(2));
            *REGULAR_MATERIALIZATION_PAUSE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .expect("install hard-link hook") = Some(DirectoryOpenPause {
                name: b"race-target".to_vec(),
                reached: Arc::clone(&reached),
                resume: Arc::clone(&resume),
            });

            let worker = std::thread::spawn({
                let workspace = workspace.clone();
                move || {
                    chown_tree_without_following(
                        &workspace,
                        CandidateIdentity {
                            uid: 65_532,
                            gid: 65_532,
                        },
                    )
                }
            });
            reached.wait();
            std::fs::hard_link(&workspace_file, &outside_alias)
                .expect("install alias after link-count check");
            resume.wait();
            worker
                .join()
                .expect("ownership worker did not panic")
                .expect("private materialization must safely close the late-alias race");

            let outside_after =
                std::fs::metadata(&outside_alias).expect("outside alias metadata after transfer");
            assert_eq!(
                outside_after.uid(),
                before.uid(),
                "external owner UID changed"
            );
            assert_eq!(
                outside_after.gid(),
                before.gid(),
                "external owner GID changed"
            );
            assert_eq!(outside_after.ino(), before.ino(), "external inode changed");
            assert_eq!(
                std::fs::read(&outside_alias).expect("outside alias content"),
                before_content,
                "external alias content changed"
            );
            let workspace_after =
                std::fs::metadata(&workspace_file).expect("materialized workspace metadata");
            assert_eq!(workspace_after.uid(), 65_532);
            assert_eq!(workspace_after.gid(), 65_532);
            assert_ne!(
                workspace_after.ino(),
                before.ino(),
                "candidate workspace must use a private inode"
            );
            *REGULAR_MATERIALIZATION_PAUSE
                .get()
                .expect("hard-link hook initialized")
                .lock()
                .expect("clear hard-link hook") = None;
        }

        #[test]
        fn rollback_failure_retains_displaced_original_inode_for_recovery() {
            if unsafe { libc::geteuid() } != 0 {
                return;
            }
            use std::os::unix::fs::MetadataExt;

            let fixture = tempfile::tempdir().expect("create ownership fixture");
            let workspace = fixture.path().join("workspace");
            std::fs::create_dir(&workspace).expect("create workspace");
            let workspace_file = workspace.join("rollback-target");
            let recovery_file = fixture.path().join("recovered-original");
            std::fs::write(&workspace_file, b"original host data").expect("seed workspace file");
            let before = std::fs::metadata(&workspace_file).expect("workspace metadata");
            let before_content = std::fs::read(&workspace_file).expect("workspace content");

            let reached = Arc::new(std::sync::Barrier::new(2));
            let resume = Arc::new(std::sync::Barrier::new(2));
            *MATERIALIZATION_EXCHANGE_PAUSE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .expect("install exchange hook") = Some(DirectoryOpenPause {
                name: b"rollback-target".to_vec(),
                reached: Arc::clone(&reached),
                resume: Arc::clone(&resume),
            });

            let worker = std::thread::spawn({
                let workspace = workspace.clone();
                move || {
                    chown_tree_without_following(
                        &workspace,
                        CandidateIdentity {
                            uid: 65_532,
                            gid: 65_532,
                        },
                    )
                }
            });
            reached.wait();
            let displaced_name = std::fs::read_dir(&workspace)
                .expect("list exchanged workspace")
                .map(|entry| entry.expect("read workspace entry"))
                .find(|entry| {
                    entry
                        .file_name()
                        .as_bytes()
                        .starts_with(b".wcore-eval-materialize-")
                })
                .expect("find displaced original")
                .path();
            std::fs::rename(&displaced_name, &recovery_file)
                .expect("move displaced original to recovery path");
            resume.wait();
            let error = worker
                .join()
                .expect("ownership worker did not panic")
                .expect_err("missing exchange entry must force rollback failure");
            *MATERIALIZATION_EXCHANGE_PAUSE
                .get()
                .expect("exchange hook initialized")
                .lock()
                .expect("clear exchange hook") = None;

            assert!(
                error.to_string().contains("restoration failed"),
                "unexpected rollback error: {error}"
            );
            let recovered = std::fs::metadata(&recovery_file).expect("recovered original metadata");
            assert_eq!(recovered.dev(), before.dev(), "original device changed");
            assert_eq!(recovered.ino(), before.ino(), "original inode changed");
            assert_eq!(recovered.uid(), before.uid(), "original owner UID changed");
            assert_eq!(recovered.gid(), before.gid(), "original owner GID changed");
            assert_eq!(
                std::fs::read(&recovery_file).expect("recovered original content"),
                before_content,
                "original inode content changed"
            );
            let workspace_after =
                std::fs::metadata(&workspace_file).expect("private workspace metadata");
            assert_ne!(
                workspace_after.ino(),
                before.ino(),
                "failed rollback must leave the private inode at the workspace path"
            );
        }

        #[test]
        fn pre_exchange_failure_retains_private_materialization_for_recovery() {
            if unsafe { libc::geteuid() } != 0 {
                return;
            }
            use std::os::unix::fs::MetadataExt;

            let fixture = tempfile::tempdir().expect("create ownership fixture");
            let workspace = fixture.path().join("workspace");
            std::fs::create_dir(&workspace).expect("create workspace");
            let workspace_file = workspace.join("pre-exchange-target");
            let recovery_file = fixture.path().join("recovered-original");
            std::fs::write(&workspace_file, b"original host data").expect("seed workspace file");
            let before = std::fs::metadata(&workspace_file).expect("workspace metadata");
            let before_content = std::fs::read(&workspace_file).expect("workspace content");

            let reached = Arc::new(std::sync::Barrier::new(2));
            let resume = Arc::new(std::sync::Barrier::new(2));
            *REGULAR_MATERIALIZATION_PAUSE
                .get_or_init(|| Mutex::new(None))
                .lock()
                .expect("install materialization hook") = Some(DirectoryOpenPause {
                name: b"pre-exchange-target".to_vec(),
                reached: Arc::clone(&reached),
                resume: Arc::clone(&resume),
            });

            let worker = std::thread::spawn({
                let workspace = workspace.clone();
                move || {
                    chown_tree_without_following(
                        &workspace,
                        CandidateIdentity {
                            uid: 65_532,
                            gid: 65_532,
                        },
                    )
                }
            });
            reached.wait();
            std::fs::rename(&workspace_file, &recovery_file)
                .expect("move original before exchange");
            resume.wait();
            let error = worker
                .join()
                .expect("ownership worker did not panic")
                .expect_err("missing workspace entry must fail before exchange");
            *REGULAR_MATERIALIZATION_PAUSE
                .get()
                .expect("materialization hook initialized")
                .lock()
                .expect("clear materialization hook") = None;

            assert_eq!(error.kind(), io::ErrorKind::NotFound);
            let recovered = std::fs::metadata(&recovery_file).expect("recovered original metadata");
            assert_eq!(recovered.dev(), before.dev(), "original device changed");
            assert_eq!(recovered.ino(), before.ino(), "original inode changed");
            assert_eq!(
                std::fs::read(&recovery_file).expect("recovered original content"),
                before_content,
                "original inode content changed"
            );
            let retained = std::fs::read_dir(&workspace)
                .expect("list failed materialization workspace")
                .map(|entry| entry.expect("read workspace entry"))
                .find(|entry| {
                    entry
                        .file_name()
                        .as_bytes()
                        .starts_with(b".wcore-eval-materialize-")
                })
                .expect("private materialization must remain recoverable");
            let retained_metadata = retained.metadata().expect("retained private metadata");
            assert_ne!(
                retained_metadata.ino(),
                before.ino(),
                "retained materialization must be a private inode"
            );
            assert_eq!(retained_metadata.uid(), 65_532);
            assert_eq!(retained_metadata.gid(), 65_532);
            assert_eq!(
                std::fs::read(retained.path()).expect("retained private content"),
                before_content,
                "retained private content changed"
            );
        }

        #[test]
        fn cleanup_reaper_handles_burst_on_one_worker() {
            let reaper = CleanupReaper::spawn_with_capacity(1_024).expect("start cleanup reaper");
            let paths = tempfile::tempdir().expect("cleanup fixture");
            for index in 0..512 {
                reaper
                    .enqueue(paths.path().join(format!("already-removed-{index}")))
                    .expect("queue cleanup");
            }
            reaper.flush().expect("flush cleanup burst");
            reaper.check_failures().expect("cleanup burst is clean");
            assert_eq!(reaper.worker_count.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn persistent_cleanup_failure_is_surfaced() {
            let reaper = CleanupReaper::spawn_with_capacity(1).expect("start cleanup reaper");
            record_failure(&reaper.failures, "fixture cleanup failure".to_string());
            let error = reaper
                .check_failures()
                .expect_err("persistent cleanup failure must fail closed");
            assert!(error.to_string().contains("fixture cleanup failure"));
        }
    }
}
