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

/// The mechanism that owns and reaps the COMPLETE process tree of a
/// hard-contained execution.
///
/// Deliberately has no ordinary-process-group variant. A Unix process group is
/// a Dangerous-mode reliability backstop (see [`isolate`]) that an adversarial
/// child can leave via `setsid`/`setpgid`; it must NEVER by itself qualify as
/// the hard containment boundary. Only these kernel-backed mechanisms — each
/// live-probed by its backend — can name the tree owner, and the ordinary
/// [`ProcessTreeGuard`] helpers below remain purely for cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // per-target: not every variant is constructed on every OS/feature build.
pub enum ProcessTreeMechanism {
    /// The bwrap PID-namespace init reaped via `/proc` descendant discovery.
    LinuxPidNamespaceReap,
    /// The Docker daemon force-removing the container and its process tree.
    DockerContainerReap,
    /// A Windows kill-on-close Job Object.
    WindowsJobObject,
}

/// Armed while a direct child is alive. Dropping it kills the dedicated Unix
/// process group or the Windows Job. A Windows Job is a hard descendant
/// boundary; see [`isolate`] for the documented Unix limitation.
pub struct ProcessTreeGuard {
    #[cfg(unix)]
    process_group: Option<libc::pid_t>,
    #[cfg(target_os = "linux")]
    root: Option<LinuxProcessIdentity>,
    #[cfg(target_os = "linux")]
    linux_group: Option<LinuxProcessGroupAuthority>,
    #[cfg(target_os = "macos")]
    mac_group: Option<MacProcessGroupAuthority>,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}

impl ProcessTreeGuard {
    pub fn new(_pid: Option<u32>) -> std::io::Result<Self> {
        let pid = _pid.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "spawned child has no PID")
        })?;
        #[cfg(target_os = "linux")]
        let root = LinuxProcessIdentity::open(pid)?;
        #[cfg(target_os = "linux")]
        let linux_group = LinuxProcessGroupAuthority::attach(&root)?;
        Ok(Self {
            #[cfg(unix)]
            process_group: Some(libc::pid_t::try_from(pid).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "child PID exceeds pid_t")
            })?),
            #[cfg(target_os = "linux")]
            root: Some(root),
            #[cfg(target_os = "linux")]
            linux_group: Some(linux_group),
            #[cfg(target_os = "macos")]
            mac_group: Some(MacProcessGroupAuthority::attach(
                libc::pid_t::try_from(pid).map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "child PID exceeds pid_t")
                })?,
            )?),
            #[cfg(windows)]
            job: Some(WindowsJob::attach(pid)?),
        })
    }

    /// Own a Linux process subtree whose root may have created a new session
    /// and therefore cannot be addressed through the launcher's process group.
    #[cfg(target_os = "linux")]
    pub(crate) fn from_observed_root(pid: u32) -> std::io::Result<Self> {
        Ok(Self {
            process_group: None,
            root: Some(LinuxProcessIdentity::open(pid)?),
            linux_group: None,
        })
    }

    /// Ask a Unix child group to unwind cooperatively before the guard's hard
    /// kill. This lets a supervised process drop guards for nested process
    /// groups of its own. Callers must apply a bounded wait and then drop this
    /// guard; cooperation is not assumed.
    #[cfg(unix)]
    pub fn request_graceful_shutdown(&self) -> std::io::Result<()> {
        let Some(_process_group) = self.process_group else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        {
            self.linux_group
                .as_ref()
                .map_or(Ok(()), |group| group.signal_group(libc::SIGTERM))
        }
        #[cfg(target_os = "macos")]
        {
            self.mac_group
                .as_ref()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "macOS process-group authority is unavailable",
                    )
                })?
                .signal_group(libc::SIGTERM)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // SAFETY: `isolate_std` created a dedicated group whose ID is the
            // child PID. A negative PID targets that group only.
            let result = unsafe { libc::kill(-_process_group, libc::SIGTERM) };
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
    }

    pub(crate) fn disarm(&mut self) {
        #[cfg(unix)]
        if self.process_group.is_some() {
            terminate_process_tree(
                self.process_group.take(),
                #[cfg(target_os = "linux")]
                self.root.take(),
                #[cfg(target_os = "linux")]
                self.linux_group.take(),
                #[cfg(target_os = "macos")]
                self.mac_group.take(),
            );
        }
        #[cfg(target_os = "linux")]
        if self.root.is_some() {
            terminate_process_tree(None, self.root.take(), self.linux_group.take());
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
        if self.process_group.is_some() {
            terminate_process_tree(
                self.process_group.take(),
                #[cfg(target_os = "linux")]
                self.root.take(),
                #[cfg(target_os = "linux")]
                self.linux_group.take(),
                #[cfg(target_os = "macos")]
                self.mac_group.take(),
            );
        }
        #[cfg(target_os = "linux")]
        if self.root.is_some() {
            terminate_process_tree(None, self.root.take(), self.linux_group.take());
        }
    }
}

#[cfg(unix)]
fn terminate_process_tree(
    _process_group: Option<libc::pid_t>,
    #[cfg(target_os = "linux")] root: Option<LinuxProcessIdentity>,
    #[cfg(target_os = "linux")] linux_group: Option<LinuxProcessGroupAuthority>,
    #[cfg(target_os = "macos")] mac_group: Option<MacProcessGroupAuthority>,
) {
    #[cfg(target_os = "linux")]
    if let Some(root) = root {
        let root_matches = root.still_matches();
        if root_matches {
            for descendant in linux_descendants(root.pid).into_iter().rev() {
                descendant.kill();
            }
            root.kill();
        }
        if let Some(group) = linux_group {
            group.signal_group(libc::SIGKILL).ok();
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(group) = mac_group {
        group.signal_group(libc::SIGKILL).ok();
        return;
    }
    // SAFETY: `isolate` created a dedicated group whose ID is the child PID. A
    // negative PID targets only that group. Reaping the group on both future
    // drop and normal direct-child completion prevents background descendants
    // from outliving the bounded command.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    if let Some(process_group) = _process_group {
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug)]
struct MacProcessIdentity {
    pid: libc::pid_t,
    start_sec: u64,
    start_usec: u64,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct MacProcessGroupAuthority {
    process_group: libc::pid_t,
    sentinel: MacProcessIdentity,
    channel: std::os::fd::OwnedFd,
}

#[cfg(target_os = "macos")]
impl MacProcessGroupAuthority {
    fn attach(process_group: libc::pid_t) -> std::io::Result<Self> {
        Self::attach_with_hook(process_group, || {})
    }

    fn attach_with_hook(
        process_group: libc::pid_t,
        after_sentinel_ready: impl FnOnce(),
    ) -> std::io::Result<Self> {
        use std::os::fd::FromRawFd;

        let root = MacProcessIdentity::open(process_group)?;
        if macos_process_group(process_group)? != process_group {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "spawned macOS process does not own its expected process group",
            ));
        }
        let mut sockets = [0; 2];
        // SAFETY: `sockets` is writable storage for two descriptors.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sockets.as_mut_ptr()) }
            != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: fork duplicates only raw descriptors. The child performs
        // async-signal-safe syscalls and exits without touching Rust state.
        let sentinel_pid = unsafe { libc::fork() };
        if sentinel_pid < 0 {
            unsafe {
                libc::close(sockets[0]);
                libc::close(sockets[1]);
            }
            return Err(std::io::Error::last_os_error());
        }
        if sentinel_pid == 0 {
            unsafe {
                libc::close(sockets[0]);
                let joined = libc::setpgid(0, process_group) == 0;
                // The sentinel must survive the cooperative group signal so it
                // continues to pin this exact process-group generation until
                // the final atomic SIGKILL.
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
                let ready = if joined { 1_u8 } else { 0_u8 };
                libc::write(sockets[1], (&ready as *const u8).cast(), 1);
                let mut byte = 0_u8;
                while libc::read(sockets[1], (&mut byte as *mut u8).cast(), 1) > 0 {}
                libc::_exit(if joined { 0 } else { 1 });
            }
        }
        unsafe {
            libc::close(sockets[1]);
        }
        // SAFETY: socketpair returned a fresh descriptor owned by this branch.
        let channel = unsafe { std::os::fd::OwnedFd::from_raw_fd(sockets[0]) };
        let mut ready = 0_u8;
        use std::os::fd::AsRawFd;
        // SAFETY: `ready` is writable for one byte and channel is live.
        let read = unsafe { libc::read(channel.as_raw_fd(), (&mut ready as *mut u8).cast(), 1) };
        if read != 1 || ready != 1 {
            unsafe {
                libc::kill(sentinel_pid, libc::SIGKILL);
                libc::waitpid(sentinel_pid, std::ptr::null_mut(), 0);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "failed to attach macOS process-group sentinel",
            ));
        }
        after_sentinel_ready();
        let sentinel = match MacProcessIdentity::open(sentinel_pid) {
            Ok(identity) => identity,
            Err(error) => {
                unsafe {
                    libc::kill(sentinel_pid, libc::SIGKILL);
                    libc::waitpid(sentinel_pid, std::ptr::null_mut(), 0);
                }
                return Err(error);
            }
        };
        let root_and_group_still_match = root.still_matches()
            && macos_process_group(process_group).is_ok_and(|group| group == process_group);
        if !root_and_group_still_match {
            unsafe {
                libc::kill(sentinel_pid, libc::SIGKILL);
                libc::waitpid(sentinel_pid, std::ptr::null_mut(), 0);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "macOS process-group authority changed while containment was attached",
            ));
        }
        Ok(Self {
            process_group,
            sentinel,
            channel,
        })
    }

    fn signal_group(&self, signal: libc::c_int) -> std::io::Result<()> {
        if !self.sentinel.still_matches() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "macOS process-group generation identity changed",
            ));
        }
        // The live sentinel makes numeric group reuse impossible. Addressing
        // the group in one syscall avoids a check-then-signal PID-reuse race.
        let result = unsafe { libc::kill(-self.process_group, signal) };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for MacProcessGroupAuthority {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // Closing the channel lets the sentinel exit after it has preserved
        // the process-group generation through the final member scan.
        unsafe {
            libc::shutdown(self.channel.as_raw_fd(), libc::SHUT_RDWR);
            libc::kill(self.sentinel.pid, libc::SIGKILL);
            libc::waitpid(self.sentinel.pid, std::ptr::null_mut(), 0);
        }
    }
}

#[cfg(target_os = "macos")]
impl MacProcessIdentity {
    fn open(pid: libc::pid_t) -> std::io::Result<Self> {
        let info = macos_bsd_info(pid)?;
        Ok(Self {
            pid,
            start_sec: info.pbi_start_tvsec,
            start_usec: info.pbi_start_tvusec,
        })
    }

    fn still_matches(&self) -> bool {
        macos_bsd_info(self.pid).is_ok_and(|info| {
            info.pbi_start_tvsec == self.start_sec && info.pbi_start_tvusec == self.start_usec
        })
    }

    fn signal(&self, signal: libc::c_int) {
        if self.still_matches() {
            // SAFETY: the immediately preceding proc_pidinfo identity check
            // bound this numeric PID to its captured process start tuple.
            unsafe {
                libc::kill(self.pid, signal);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_bsd_info(pid: libc::pid_t) -> std::io::Result<libc::proc_bsdinfo> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: `info` points to writable storage of the exact advertised size.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as libc::c_int,
        )
    };
    if read != size as libc::c_int {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: proc_pidinfo reported a complete proc_bsdinfo payload.
    Ok(unsafe { info.assume_init() })
}

#[cfg(target_os = "macos")]
fn macos_process_group(pid: libc::pid_t) -> std::io::Result<libc::pid_t> {
    // SAFETY: getpgid is read-only and accepts the captured positive PID.
    let process_group = unsafe { libc::getpgid(pid) };
    if process_group < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(process_group)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn identity_drift_never_signals_foreign_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn fixture");
        let mut identity = MacProcessIdentity::open(child.id() as libc::pid_t).expect("identity");
        identity.start_usec = identity.start_usec.saturating_add(1);
        identity.signal(libc::SIGKILL);
        assert!(child.try_wait().expect("wait status").is_none());
        child.kill().expect("cleanup fixture");
        child.wait().expect("reap fixture");
    }

    #[test]
    fn process_group_generation_drift_fails_closed() {
        let mut child_command = std::process::Command::new("sleep");
        child_command.arg("30");
        isolate_std(&mut child_command);
        let mut child = child_command.spawn().expect("spawn fixture");
        let process_group = child.id() as libc::pid_t;
        let mut authority =
            MacProcessGroupAuthority::attach(process_group).expect("group authority");

        authority.sentinel.start_usec = authority.sentinel.start_usec.saturating_add(1);
        let error = authority
            .signal_group(libc::SIGKILL)
            .expect_err("drifted generation must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(child.try_wait().expect("wait status").is_none());

        child.kill().expect("cleanup fixture");
        child.wait().expect("reap fixture");
    }

    #[test]
    fn root_exit_during_group_attachment_fails_closed() {
        let mut child_command = std::process::Command::new("sleep");
        child_command.arg("30");
        isolate_std(&mut child_command);
        let mut child = child_command.spawn().expect("spawn fixture");
        let process_group = child.id() as libc::pid_t;

        let error = MacProcessGroupAuthority::attach_with_hook(process_group, || {
            child.kill().expect("stop original root");
            child.wait().expect("reap original root");
        })
        .expect_err("vanished root must invalidate attachment");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    /// Required macOS live acceptance: the owned process tree — including a
    /// descendant — is reaped by terminal teardown BEFORE workspace cleanup.
    /// The identity is present and non-skipping; native EXECUTION is validated
    /// on macOS in plan 20-08.
    #[test]
    fn required_live_descendant_teardown_before_workspace_cleanup() {
        super::assert_descendant_teardown_before_workspace_cleanup();
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct LinuxProcessIdentity {
    pid: libc::pid_t,
    start_time: u64,
    pidfd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl LinuxProcessIdentity {
    fn open(pid: u32) -> std::io::Result<Self> {
        use std::os::fd::FromRawFd;

        let pid = libc::pid_t::try_from(pid).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "child PID exceeds pid_t")
        })?;
        let start_time = linux_process_start_time(pid)?;
        // SAFETY: pidfd_open returns a new owned descriptor on success.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as libc::c_int };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor returned by pidfd_open.
        let pidfd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        let identity = Self {
            pid,
            start_time,
            pidfd,
        };
        if !identity.still_matches() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "process identity changed while containment was attached",
            ));
        }
        Ok(identity)
    }

    fn still_matches(&self) -> bool {
        linux_process_start_time(self.pid).is_ok_and(|start| start == self.start_time)
    }

    fn kill(&self) {
        if !self.still_matches() {
            return;
        }
        use std::os::fd::AsRawFd;
        // SAFETY: pidfd_send_signal addresses the kernel object referenced by
        // this owned pidfd, not whichever process may later reuse `pid`.
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.pidfd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct LinuxProcessGroupAuthority {
    process_group: libc::pid_t,
    sentinel: LinuxProcessIdentity,
    channel: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl LinuxProcessGroupAuthority {
    fn attach(root: &LinuxProcessIdentity) -> std::io::Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd};

        if linux_process_group(root.pid)? != root.pid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "spawned Linux process does not own its expected process group",
            ));
        }
        let mut sockets = [0; 2];
        // SAFETY: `sockets` is writable storage for two descriptors.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, sockets.as_mut_ptr()) }
            != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: the child branch invokes only async-signal-safe syscalls.
        let sentinel_pid = unsafe { libc::fork() };
        if sentinel_pid < 0 {
            unsafe {
                libc::close(sockets[0]);
                libc::close(sockets[1]);
            }
            return Err(std::io::Error::last_os_error());
        }
        if sentinel_pid == 0 {
            unsafe {
                libc::close(sockets[0]);
                let joined = libc::setpgid(0, root.pid) == 0;
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
                let ready = if joined { 1_u8 } else { 0_u8 };
                libc::write(sockets[1], (&ready as *const u8).cast(), 1);
                let mut byte = 0_u8;
                while libc::read(sockets[1], (&mut byte as *mut u8).cast(), 1) > 0 {}
                libc::_exit(if joined { 0 } else { 1 });
            }
        }
        unsafe {
            libc::close(sockets[1]);
        }
        // SAFETY: socketpair returned a fresh descriptor owned by this branch.
        let channel = unsafe { std::os::fd::OwnedFd::from_raw_fd(sockets[0]) };
        let mut ready = 0_u8;
        // SAFETY: `ready` is writable for one byte and channel is live.
        let read = unsafe { libc::read(channel.as_raw_fd(), (&mut ready as *mut u8).cast(), 1) };
        if read != 1 || ready != 1 {
            reap_sentinel(sentinel_pid);
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "failed to attach Linux process-group sentinel",
            ));
        }
        let sentinel = match LinuxProcessIdentity::open(sentinel_pid.try_into().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "sentinel PID exceeds u32")
        })?) {
            Ok(identity) => identity,
            Err(error) => {
                reap_sentinel(sentinel_pid);
                return Err(error);
            }
        };
        let root_and_group_still_match = root.still_matches()
            && linux_process_group(root.pid).is_ok_and(|group| group == root.pid);
        if !root_and_group_still_match {
            reap_sentinel(sentinel_pid);
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Linux process-group authority changed while containment was attached",
            ));
        }
        Ok(Self {
            process_group: root.pid,
            sentinel,
            channel,
        })
    }

    fn signal_group(&self, signal: libc::c_int) -> std::io::Result<()> {
        if !self.sentinel.still_matches() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Linux process-group generation identity changed",
            ));
        }
        // The live sentinel pins this generation, so one group signal cannot
        // race with numeric PGID reuse.
        if unsafe { libc::kill(-self.process_group, signal) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for LinuxProcessGroupAuthority {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::shutdown(self.channel.as_raw_fd(), libc::SHUT_RDWR);
        }
        reap_sentinel(self.sentinel.pid);
    }
}

#[cfg(target_os = "linux")]
fn linux_process_group(pid: libc::pid_t) -> std::io::Result<libc::pid_t> {
    // SAFETY: getpgid is read-only and accepts the captured positive PID.
    let process_group = unsafe { libc::getpgid(pid) };
    if process_group < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(process_group)
    }
}

#[cfg(target_os = "linux")]
fn reap_sentinel(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
}

#[cfg(target_os = "linux")]
fn linux_process_start_time(pid: libc::pid_t) -> std::io::Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, fields) = stat.rsplit_once(") ").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed /proc stat")
    })?;
    fields
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing starttime"))?
        .parse()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid starttime"))
}

#[cfg(target_os = "linux")]
fn linux_descendants(root: libc::pid_t) -> Vec<LinuxProcessIdentity> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut parent_by_pid = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let Some(parent) = fields
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        parent_by_pid.push((pid, parent));
    }
    let mut descendant_pids = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for &(pid, candidate_parent) in &parent_by_pid {
            if candidate_parent == parent && !descendant_pids.contains(&pid) {
                descendant_pids.push(pid);
                frontier.push(pid);
            }
        }
    }
    descendant_pids
        .into_iter()
        .filter_map(|pid| LinuxProcessIdentity::open(pid.try_into().ok()?).ok())
        .collect()
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[test]
    fn identity_drift_never_signals_foreign_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn fixture");
        let mut identity = LinuxProcessIdentity::open(child.id()).expect("open pidfd");
        identity.start_time = identity.start_time.saturating_add(1);
        identity.kill();
        assert!(child.try_wait().expect("wait status").is_none());
        child.kill().expect("cleanup fixture");
        child.wait().expect("reap fixture");
    }

    /// Required Linux live acceptance: the owned process tree — including a
    /// descendant — is reaped by terminal teardown BEFORE workspace cleanup
    /// runs. Fails if a descendant survives teardown.
    #[test]
    fn required_live_descendant_teardown_before_workspace_cleanup() {
        super::assert_descendant_teardown_before_workspace_cleanup();
    }
}

/// Spawn an owned tree with a backgrounded descendant, tear the tree down, and
/// prove the descendant is reaped BEFORE the workspace directory is cleaned up.
/// Shared by the Linux and macOS `required_live_*` identities.
#[cfg(all(test, unix))]
fn assert_descendant_teardown_before_workspace_cleanup() {
    let dir = tempfile::tempdir().expect("workspace");
    let pidfile = dir.path().join("descendant.pid");
    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(format!(
        "sh -c 'echo $$ > \"{}\"; exec sleep 300' & sleep 300",
        pidfile.display()
    ));
    isolate_std(&mut command);
    let mut child = command.spawn().expect("spawn owned process tree");
    let mut guard = ProcessTreeGuard::new(Some(child.id())).expect("own the process tree");
    let descendant = wait_for_recorded_pid(&pidfile);
    assert!(
        pid_is_alive(descendant),
        "owned descendant must be running before teardown"
    );
    // Terminal teardown runs BEFORE workspace cleanup.
    guard.disarm();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        wait_until_pid_gone(descendant, std::time::Duration::from_secs(10)),
        "owned descendant survived teardown before workspace cleanup"
    );
    // Workspace cleanup runs only on a confirmed-reaped tree.
    drop(dir);
    assert!(!pid_is_alive(descendant));
}

#[cfg(all(test, unix))]
fn wait_for_recorded_pid(path: &std::path::Path) -> libc::pid_t {
    for _ in 0..1000 {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(pid) = text.trim().parse::<libc::pid_t>()
            && pid > 0
        {
            return pid;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("owned descendant never recorded its PID");
}

#[cfg(all(test, unix))]
fn pid_is_alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only probes for existence/permission; it delivers nothing.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(all(test, unix))]
fn wait_until_pid_gone(pid: libc::pid_t, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !pid_is_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    !pid_is_alive(pid)
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
