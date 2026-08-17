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
            mac_group: MacProcessGroupAuthority::attach(libc::pid_t::try_from(pid).map_err(
                |_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "child PID exceeds pid_t")
                },
            )?)?,
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
            // `None` here is not "the authority is missing", it is "the
            // workload's process group no longer exists" — see
            // `MacProcessGroupAuthority::attach_with_hook`. There is nothing to
            // ask to unwind, and reporting that as a refusal made a completed
            // workload look like a containment fault.
            match self.mac_group.as_ref() {
                Some(group) => group.signal_group(libc::SIGTERM),
                None => Ok(()),
            }
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
    fn attach(process_group: libc::pid_t) -> std::io::Result<Option<Self>> {
        Self::attach_with_hook(process_group, || {})
    }

    /// Attach to the workload's process group, or answer `Ok(None)` when that
    /// group no longer exists at all.
    ///
    /// # The ENTRY half of the Darwin corpse defect
    ///
    /// The post-fork check below already documents that Darwin answers ESRCH
    /// from BOTH `proc_pidinfo(PROC_PIDTBSDINFO)` and `getpgid()` for an
    /// unreaped zombie, where Linux answers normally — and it relaxed the
    /// check that ran AFTER the sentinel joined. The two probes at the TOP of
    /// this function were left on the old rule, so the identical failure
    /// survived one window earlier: a child that finishes BEFORE
    /// `ProcessTreeGuard::new` is even reached — routine for `git config`,
    /// `git rev-list` and every other fast command on a loaded host — made
    /// `MacProcessIdentity::open` return ESRCH and turned a subprocess that had
    /// already run to completion into
    /// `failed to establish process-tree containment: No such process`.
    ///
    /// Measured on `macos-latest` (runner image macos-26-arm64/20260728.0273),
    /// 12 concurrent runs of `wcore-swarm`'s
    /// `independent_cli_processes_cannot_overbook_shared_capacity`: 4 spurious
    /// failures in 3 rounds, every one of them this message. Linux never
    /// reproduces it — `/proc/<pid>` survives a zombie — which is why this is
    /// a macOS-only leg failure.
    ///
    /// # Why ESRCH is safe to read as "our child is a corpse"
    ///
    /// Every caller passes the pid of a child it has not yet reaped, so the
    /// kernel cannot hand that pid to a stranger while the zombie is held. A
    /// stranger would in any case be LIVE, which takes the normal path and is
    /// still refused by the `Recycled` arm below.
    ///
    /// # Why `Ok(None)` is not a hole in containment
    ///
    /// The sentinel's `setpgid(0, <vanished group>)` is refused by the kernel
    /// (pinned by `joining_a_vanished_process_group_is_refused_by_the_kernel`),
    /// so `Ok(None)` is reached only when the group has NO members left: the
    /// direct child is a corpse and it left no descendant behind. If a
    /// descendant IS alive the group still exists, the sentinel joins, and a
    /// full authority is returned exactly as before — that case is pinned by
    /// `a_corpse_root_with_a_live_descendant_still_yields_containment`.
    fn attach_with_hook(
        process_group: libc::pid_t,
        after_sentinel_ready: impl FnOnce(),
    ) -> std::io::Result<Option<Self>> {
        use std::os::fd::FromRawFd;

        // RED ARM MUTATION (temporary): restore the pre-fix fail-closed entry
        // probe. Behaviour only - the signature is unchanged, so a failure here
        // is the SYMPTOM and not a compile break.
        let root = match MacProcessIdentity::open(process_group) {
            Ok(identity) => Some(identity),
            Err(error) => return Err(error),
        };
        if root.is_some() {
            match macos_process_group(process_group) {
                Ok(group) if group == process_group => {}
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "spawned macOS process does not own its expected process group",
                    ));
                }
                // The root exited between the two probes. Same disposition as
                // a root that was already a corpse when this function started.
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => return Err(error),
            }
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
            if root.is_none() {
                // The only way the sentinel's `setpgid` is refused for a corpse
                // root is that the group has no members left, so there is no
                // process tree to own. Reporting that as a containment fault
                // failed a subprocess that had already finished.
                return Ok(None);
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
        // What must hold once the sentinel has joined:
        //
        //   1. the SENTINEL is really in `process_group` — confirmed here by
        //      the parent rather than taken from the child's own report, and
        //   2. the root has not been replaced by a different process.
        //
        // This used to read `root.still_matches() && getpgid(root) == root`,
        // and on Darwin **neither conjunct can hold once the root exits**:
        // both `proc_pidinfo` and `getpgid` answer ESRCH for a zombie
        // (measured). A workload that finishes during the socketpair + fork
        // window — `git config`, and every other fast child — therefore
        // reported "authority changed while containment was attached" and
        // could never establish containment at all. That is a gate with no
        // reachable pass state, not a security check.
        //
        // The anchor is the sentinel, not the root: a process group id cannot
        // be recycled while a live process sits in it, and the sentinel is a
        // live unreaped child of ours held open by `channel`. `signal_group`
        // already rests on exactly that and guards with `sentinel`, not
        // `root`. A finished root is the SUCCESS case; a *replaced* root is
        // still refused.
        let sentinel_holds_the_group =
            macos_process_group(sentinel_pid).is_ok_and(|group| group == process_group);
        // The OLD check `root.still_matches() && getpgid(root) == root` carried
        // TWO properties: root identity, and root group-leadership. Only the
        // first is unsatisfiable for a corpse, so only the first is relaxed —
        // a LIVE root must still be in the group it claims to lead. Dropping
        // that conjunct would accept a root that called `setpgid`/`setsid`
        // itself during the attach window, and teardown would then kill a
        // group the workload had already left. (The module header is explicit
        // that a child can escape at any later instant, so this closes an
        // attach-time window rather than providing hard containment — but a
        // window that was closed should not be opened for free.)
        let root_state = match &root {
            Some(root) => root.recheck(),
            // Already a corpse when this function started: the same state the
            // `Corpse` arm below accepts, reached one window earlier.
            None => MacIdentityRecheck::Corpse,
        };
        let root_ok = match &root_state {
            MacIdentityRecheck::Same => root.as_ref().is_some_and(|root| {
                macos_process_group(root.pid).is_ok_and(|group| group == process_group)
            }),
            MacIdentityRecheck::Corpse => true,
            MacIdentityRecheck::Recycled | MacIdentityRecheck::Unreadable(_) => false,
        };
        if !(sentinel_holds_the_group && root_ok) {
            unsafe {
                libc::kill(sentinel_pid, libc::SIGKILL);
                libc::waitpid(sentinel_pid, std::ptr::null_mut(), 0);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                // `root_state` is the value the DECISION was made on. Calling
                // `recheck()` again here would let the diagnostic disagree
                // with the branch that produced it.
                //
                // Destructured rather than `{root_state:?}` so the io::Error
                // inside `Unreadable` actually reaches a human. Derived Debug
                // does NOT count as a read for dead-code analysis, so the
                // `{:?}` form left field 0 unread — which is a `-D warnings`
                // error on macOS, and it meant the one variant that carries a
                // diagnosis was the one that discarded it.
                format!(
                    "macOS process-group authority changed while containment was attached \
                     (sentinel in group: {sentinel_holds_the_group}, root: {})",
                    match &root_state {
                        MacIdentityRecheck::Same =>
                            "same generation, but NOT in its own group".to_owned(),
                        MacIdentityRecheck::Corpse => "exited".to_owned(),
                        MacIdentityRecheck::Recycled =>
                            "REPLACED by a different process".to_owned(),
                        MacIdentityRecheck::Unreadable(error) =>
                            format!("could not be read: {error}"),
                    }
                ),
            ));
        }
        Ok(Some(Self {
            process_group,
            sentinel,
            channel,
        }))
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
        matches!(self.recheck(), MacIdentityRecheck::Same)
    }

    /// Four-valued identity recheck.
    ///
    /// `still_matches()` collapses this to a bool, which is right for the
    /// sentinel (held alive by its socketpair, so a corpse would be a real
    /// fault) and WRONG for a workload root, which is allowed to have
    /// finished. See [`MacIdentityRecheck::Corpse`].
    fn recheck(&self) -> MacIdentityRecheck {
        match macos_bsd_info(self.pid) {
            Ok(info) => {
                if info.pbi_start_tvsec == self.start_sec
                    && info.pbi_start_tvusec == self.start_usec
                {
                    MacIdentityRecheck::Same
                } else {
                    MacIdentityRecheck::Recycled
                }
            }
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => MacIdentityRecheck::Corpse,
            Err(error) => MacIdentityRecheck::Unreadable(error),
        }
    }
}

/// The outcome of re-checking a captured macOS process identity.
#[cfg(target_os = "macos")]
#[derive(Debug)]
enum MacIdentityRecheck {
    /// Same pid, same start tuple: the same process generation.
    Same,
    /// The pid names NO live process.
    ///
    /// On Darwin this is what an unreaped zombie looks like — measured:
    /// `proc_pidinfo(PROC_PIDTBSDINFO)` on a zombie fails with **ESRCH**, and
    /// so does `getpgid()`. Linux answers both for a corpse; Darwin does not.
    /// That difference is the entire defect this enum exists to express,
    /// because a two-valued check reads "the process finished" and "the pid
    /// was handed to a stranger" as the same answer.
    Corpse,
    /// The pid resolves to a DIFFERENT process generation. Never acceptable.
    Recycled,
    /// Could not be read at all. Not a measurement; never treated as Corpse.
    Unreadable(std::io::Error),
}

#[cfg(target_os = "macos")]
impl MacProcessIdentity {
    /// Identity-checked kill of this exact process generation.
    ///
    /// `cfg(test)` because production has no caller: `signal_group` addresses
    /// the whole group in one syscall (which is why it guards with
    /// `still_matches` itself), and `MacProcessGroupAuthority::drop` reaps the
    /// sentinel with a raw `kill` that is safe for a different reason — the
    /// sentinel is an unreaped child, so its PID cannot be recycled before the
    /// `waitpid` that follows. Gated rather than deleted so
    /// `identity_drift_never_signals_foreign_process` keeps proving the guard.
    #[cfg(test)]
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
        let mut authority = MacProcessGroupAuthority::attach(process_group)
            .expect("group authority")
            .expect("a live root must yield a full authority, never \"nothing to contain\"");

        authority.sentinel.start_usec = authority.sentinel.start_usec.saturating_add(1);
        let error = authority
            .signal_group(libc::SIGKILL)
            .expect_err("drifted generation must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(child.try_wait().expect("wait status").is_none());

        child.kill().expect("cleanup fixture");
        child.wait().expect("reap fixture");
    }

    /// A root that FINISHES after the sentinel has joined is the success
    /// case, not a fault.
    ///
    /// This test previously asserted the opposite — that attachment must fail
    /// closed. **That is a deliberate change of security semantics**, made
    /// because the old rule had no reachable pass state on this platform:
    ///
    /// * Darwin answers ESRCH from BOTH `proc_pidinfo` and `getpgid` for an
    ///   unreaped zombie (measured), so `root.still_matches() && getpgid(root)
    ///   == root` is unsatisfiable the instant the root exits;
    /// * the root routinely exits inside the socketpair + fork window — a
    ///   `git config` invocation does — so delegated dispatch on macOS could
    ///   not establish containment AT ALL, failing with "authority changed
    ///   while containment was attached".
    ///
    /// Why relaxing it is safe, and not merely convenient: the anchor is the
    /// live sentinel, never the root. A process-group id cannot be recycled
    /// while a live process sits in it, and `setpgid(0, <vanished group>)`
    /// returns **EPERM** — pinned by
    /// [`joining_a_vanished_process_group_is_refused_by_the_kernel`] below.
    /// So the sentinel's successful join, which `attach_with_hook` already
    /// requires before it reaches the post-check, is itself proof that the
    /// group still exists. A root that was *replaced* by a different process
    /// is still refused; only a corpse is tolerated.
    /// Spawn `argv` as its own process-group leader, let it exit, and leave it
    /// UNREAPED so its pid still names a corpse rather than a stranger.
    ///
    /// Returns only once Darwin actually reports that corpse state, so neither
    /// test below can pass by racing the fixture instead of exercising it. The
    /// preconditions are asserted rather than assumed: a fixture that never
    /// reached the state under test has proved nothing and must not read as a
    /// pass.
    fn corpse_group_leader(argv: &[&str]) -> (std::process::Child, libc::pid_t) {
        let mut child_command = std::process::Command::new(argv[0]);
        child_command.args(&argv[1..]);
        isolate_std(&mut child_command);
        let child = child_command.spawn().expect("spawn fixture");
        let process_group = child.id() as libc::pid_t;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while macos_process_group(process_group).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "fixture root never became an unreaped corpse"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // `MacProcessIdentity::open` is the exact probe `attach_with_hook` runs
        // FIRST, and the one that used to fail the whole capture closed. Assert
        // it refuses too, so the tests are pinned to that probe and not merely
        // to `getpgid`.
        assert_eq!(
            MacProcessIdentity::open(process_group)
                .err()
                .and_then(|error| error.raw_os_error()),
            Some(libc::ESRCH),
            "precondition: Darwin must answer ESRCH for an unreaped corpse"
        );
        (child, process_group)
    }

    /// A root that is ALREADY a corpse when containment attaches, having left
    /// no descendant behind, is "nothing to contain" — not a containment fault.
    ///
    /// This is the macOS-only defect that made `main` red. On a loaded host a
    /// fast `git` child routinely exits before `ProcessTreeGuard::new` runs,
    /// and the entry probes then failed the whole capture with
    /// `failed to establish process-tree containment: No such process`, even
    /// though the subprocess had already completed successfully.
    #[test]
    fn attaching_to_an_already_exited_root_is_not_a_containment_failure() {
        let (mut child, process_group) = corpse_group_leader(&["sh", "-c", "exit 0"]);
        // Without this the assertion below would be about the descendant case
        // instead — a group that still exists is a different disposition.
        assert_eq!(
            unsafe { libc::kill(-process_group, 0) },
            -1,
            "precondition: the fixture's process group must have no members left"
        );

        let authority = MacProcessGroupAuthority::attach(process_group)
            .expect("an already-exited root must not be a containment failure");
        assert!(
            authority.is_none(),
            "an empty process group must report nothing to contain"
        );

        child.wait().expect("reap fixture");
    }

    /// The security half of the same relaxation: a corpse root that DID leave a
    /// descendant keeps its group alive, so containment must still be a real
    /// authority that can kill the survivor. If this ever answers `None`, the
    /// relaxation above has become a hole rather than a repair.
    #[test]
    fn a_corpse_root_with_a_live_descendant_still_yields_containment() {
        // The shell exits immediately; the backgrounded `sleep` stays in the
        // group it inherited, so the group outlives its leader.
        let (mut child, process_group) = corpse_group_leader(&["sh", "-c", "sleep 30 & exit 0"]);
        assert_eq!(
            unsafe { libc::kill(-process_group, 0) },
            0,
            "precondition: the descendant must keep the fixture's group alive"
        );

        let authority = MacProcessGroupAuthority::attach(process_group)
            .expect("a corpse root with a live descendant must still attach")
            .expect("a surviving descendant means there IS a tree to contain");
        authority
            .signal_group(libc::SIGKILL)
            .expect("the attached authority must be able to kill the surviving descendant");

        child.wait().expect("reap fixture");
    }

    #[test]
    fn root_exit_after_sentinel_joins_still_yields_containment() {
        let mut child_command = std::process::Command::new("sleep");
        child_command.arg("30");
        isolate_std(&mut child_command);
        let mut child = child_command.spawn().expect("spawn fixture");
        let process_group = child.id() as libc::pid_t;

        let authority = MacProcessGroupAuthority::attach_with_hook(process_group, || {
            child.kill().expect("stop original root");
            child.wait().expect("reap original root");
        })
        .expect("a root that finished after the sentinel joined must still yield containment")
        .expect(
            "the sentinel joined, so the group exists and containment must be a full authority",
        );

        // And the authority must be usable, not merely constructible: a
        // handle that cannot signal its group is containment in name only.
        authority
            .signal_group(libc::SIGKILL)
            .expect("the attached authority must be able to signal its own group");
    }

    /// The kernel fact the whole sentinel argument rests on.
    ///
    /// If Apple ever lets a process join a process group that no longer has
    /// any members, "the sentinel joined" stops proving "the group exists",
    /// and [`root_exit_after_sentinel_joins_still_yields_containment`] above
    /// becomes unsound. This test is the tripwire for that.
    #[test]
    fn joining_a_vanished_process_group_is_refused_by_the_kernel() {
        // `isolate_std` is what makes this a real test: without
        // `process_group(0)` the child inherits THIS process's group and
        // never becomes a group leader, so the setpgid below would fail
        // because the group never existed rather than because it vanished —
        // the right answer for the wrong reason.
        // `sleep 30`, not `true`: the leader must still be RUNNING when its
        // group membership is confirmed. Darwin `getpgid` on a zombie answers
        // ESRCH (the very fact this file documents), and `true` routinely
        // exits before the parent's next statement — so a short-lived fixture
        // makes this assertion flake, and a flaky tripwire gets quarantined,
        // which silently removes the guard on the whole safety argument.
        let mut leader_command = std::process::Command::new("sleep");
        leader_command.arg("30");
        isolate_std(&mut leader_command);
        let mut leader = leader_command.spawn().expect("spawn leader");
        let vanished = leader.id() as libc::pid_t;
        assert_eq!(
            macos_process_group(vanished).expect("leader pgid"),
            vanished,
            "fixture did not become its own process-group leader"
        );
        leader.kill().expect("stop leader");
        leader.wait().expect("reap leader");

        // SAFETY: setpgid only moves the CALLING process, and it is expected
        // to fail. A success would move this test process into a foreign
        // group, which is precisely the outcome being ruled out — so the
        // assertion below runs before anything else can depend on it.
        let rc = unsafe { libc::setpgid(0, vanished) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(
            rc, -1,
            "joining the reaped, empty group {vanished} SUCCEEDED; the sentinel can no longer \
             prove a group exists and the macOS attach relaxation is unsound"
        );
        assert!(
            matches!(errno, Some(libc::EPERM) | Some(libc::ESRCH)),
            "expected EPERM/ESRCH joining a vanished group, got {errno:?}"
        );
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

/// Was `kill(pid, 0) == 0`, which a **zombie** satisfies — so the
/// process-tree containment tests below could not distinguish a descendant
/// that was killed successfully from one that survived. Centralised in
/// `wcore_types::process_liveness`; see `.planning/ZOMBIE-PROBE.md`.
#[cfg(all(test, unix))]
fn pid_is_alive(pid: libc::pid_t) -> bool {
    wcore_types::process_liveness::process_is_alive(pid as u32)
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
