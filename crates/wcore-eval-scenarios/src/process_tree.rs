//! Process-tree ownership for evaluator child processes.
//!
//! Linux uses a private cgroup v2 and moves the child into it from `pre_exec`,
//! before candidate code can fork. Other Unix hosts use a process group as a
//! best-effort fallback; it is intentionally not described as authoritative.

use std::io;
use std::time::Duration;

use tokio::process::{Child, Command};

#[cfg(target_os = "linux")]
use linux::Cgroup;

#[derive(Debug)]
pub(crate) struct ProcessTree {
    backend: Backend,
    root_pid: Option<u32>,
}

#[derive(Debug)]
enum Backend {
    #[cfg(target_os = "linux")]
    Cgroup(Cgroup),
    #[cfg(unix)]
    ProcessGroup,
    #[cfg(not(unix))]
    DirectChild,
}

impl ProcessTree {
    pub(crate) fn prepare() -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            match Cgroup::create() {
                Ok(cgroup) => {
                    return Ok(Self {
                        backend: Backend::Cgroup(cgroup),
                        root_pid: None,
                    });
                }
                Err(error) if containment_required() => return Err(error),
                Err(_) => {}
            }
        }

        #[cfg(unix)]
        let backend = Backend::ProcessGroup;
        #[cfg(not(unix))]
        let backend = Backend::DirectChild;
        Ok(Self {
            backend,
            root_pid: None,
        })
    }

    pub(crate) fn configure(&self, command: &mut Command) -> io::Result<()> {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.configure(command),
            #[cfg(unix)]
            Backend::ProcessGroup => {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
                Ok(())
            }
            #[cfg(not(unix))]
            Backend::DirectChild => Ok(()),
        }
    }

    pub(crate) fn bind(&mut self, child: &Child) -> io::Result<()> {
        self.root_pid = Some(
            child
                .id()
                .ok_or_else(|| io::Error::other("spawned evaluator child had no process id"))?,
        );
        Ok(())
    }

    /// Force the owned tree down, reap the direct child, and prove the kernel
    /// containment is empty before returning success.
    pub(crate) async fn terminate(&mut self, child: &mut Child) -> io::Result<()> {
        self.kill_tree()?;
        let _ = child.start_kill();
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "direct evaluator child was not reaped within 5 seconds",
                ));
            }
        }
        self.finish_cleanup().await
    }

    /// The direct child exited normally; remove any background descendants
    /// before the run is allowed to retain a successful shutdown receipt.
    pub(crate) async fn cleanup_descendants(&mut self) -> io::Result<()> {
        self.kill_tree()?;
        self.finish_cleanup().await
    }

    fn kill_tree(&self) -> io::Result<()> {
        match &self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.kill(),
            #[cfg(unix)]
            Backend::ProcessGroup => {
                if let Some(pid) = self.root_pid {
                    // SAFETY: negative PID addresses the evaluator-owned process
                    // group. SIGKILL is used only after the graceful deadline.
                    let rc = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
                    if rc != 0 {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::ESRCH) {
                            return Err(error);
                        }
                    }
                }
                Ok(())
            }
            #[cfg(not(unix))]
            Backend::DirectChild => Ok(()),
        }
    }

    async fn finish_cleanup(&mut self) -> io::Result<()> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            Backend::Cgroup(cgroup) => cgroup.wait_empty_and_remove().await,
            #[cfg(unix)]
            Backend::ProcessGroup => Ok(()),
            #[cfg(not(unix))]
            Backend::DirectChild => Ok(()),
        }
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

#[cfg(target_os = "linux")]
fn containment_required() -> bool {
    std::env::var_os("WCORE_EVAL_REQUIRE_CONTAINMENT").is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
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
                    path,
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
