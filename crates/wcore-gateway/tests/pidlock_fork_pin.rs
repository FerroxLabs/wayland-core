//! Does a released gateway pid lock survive the gateway's own subprocesses?
//!
//! `PidLock` claims `gateway.lock` with `flock`/`LockFileEx`. That lock binds
//! to the OPEN FILE DESCRIPTION, not the descriptor and not the process, so
//! `close(2)` releases it only when the LAST descriptor referring to that
//! description goes away. `fork(2)` duplicates the descriptor table, so any
//! subprocess spawned while the lock is held keeps the description - and the
//! lock - alive until that child execs (`O_CLOEXEC`) or exits.
//!
//! The gateway hosts an agent that spawns subprocesses constantly, and unlike
//! the session journal its sentinel is a STABLE pathname and a stable inode:
//! the next `gateway start` locks the same inode. A pinned lock therefore does
//! not merely leak a descriptor, it refuses the next launch with `AlreadyHeld`
//! naming a pid that is already gone.
//!
//! Both cases here use the production API only - `PidLock::acquire` and
//! `PidLock::lock_path` - and the second grades the kernel's own record in
//! `/proc/locks` rather than the API's return value.
//!
//! Unix only: the defect is `fork(2)` duplicating a descriptor table, and the
//! probe reproduces it with a real `fork`. Windows has no equivalent - its
//! `UnlockFileEx` release is covered by the cross-target clippy gate alone.
#![cfg(unix)]

use wcore_gateway::pidlock::PidLock;

/// A child that forks and never execs, released by closing a pipe.
struct PinnedChild {
    pid: libc::pid_t,
    read_fd: i32,
    write_fd: i32,
}

impl PinnedChild {
    /// Fork a child that inherits every currently-open descriptor and blocks
    /// without ever calling `exec`. This is the shape of any daemonised helper
    /// the gateway backgrounds.
    fn spawn() -> Self {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` is a live two-element array, the only argument pipe(2) reads.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe failed");
        let (read_fd, write_fd) = (fds[0], fds[1]);

        // SAFETY: the child touches nothing but `close`/`read`/`_exit`, all
        // async-signal-safe, and never runs a Rust destructor.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe {
                libc::close(write_fd);
                let mut byte = 0u8;
                while libc::read(read_fd, std::ptr::addr_of_mut!(byte).cast(), 1) == -1 {}
                libc::_exit(0);
            }
        }
        Self {
            pid,
            read_fd,
            write_fd,
        }
    }

    fn release(self) {
        // SAFETY: both descriptors are owned by this process and still open.
        unsafe {
            libc::close(self.write_fd);
            let mut status = 0i32;
            libc::waitpid(self.pid, &mut status, 0);
            libc::close(self.read_fd);
        }
    }
}

/// Every `FLOCK` line in `/proc/locks` naming `inode`, as the kernel reports
/// it. This is the world state, not the API's opinion of the world state.
#[cfg(target_os = "linux")]
fn kernel_flocks_on(path: &std::path::Path) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).expect("sentinel must exist");
    let (dev, ino) = (meta.dev(), meta.ino());
    // `fs/locks.c` prints the target as `MAJOR:MINOR:INODE`, major and minor in
    // two hex digits and the inode in decimal. Match it as a whole token: a
    // substring match on the inode alone would collide with the pid column.
    let token = format!("{:02x}:{:02x}:{ino}", libc::major(dev), libc::minor(dev));
    std::fs::read_to_string("/proc/locks")
        .expect("/proc/locks must be readable")
        .lines()
        .filter(|line| {
            let mut fields = line.split_whitespace();
            fields.any(|field| field == "FLOCK") && fields.any(|field| field == token)
        })
        .map(str::to_owned)
        .collect()
}

/// The case no retry budget can cover.
///
/// The child never execs, so the inherited description lives for as long as it
/// does. Without an explicit `LOCK_UN` in `Drop for PidLock`, the home is
/// unclaimable for that whole lifetime even though its holder released it.
#[test]
fn a_released_home_is_reclaimable_while_a_forked_child_never_execs() {
    let dir = tempfile::tempdir().expect("tempdir");

    let held = PidLock::acquire(dir.path()).expect("first acquire succeeds");
    let child = PinnedChild::spawn();

    // The holder releases cleanly. The child still references the locked open
    // file description, so `close(2)` in this process cannot free it.
    drop(held);

    let reclaimed = PidLock::acquire(dir.path());
    child.release();

    assert!(
        reclaimed.is_ok(),
        "releasing a gateway pid lock must free the home even while a forked \
         child still holds the open file description; a stopped gateway that \
         refuses the next start is indistinguishable from a running one: {:?}",
        reclaimed.err()
    );
}

/// Grade the kernel, not the return value.
///
/// `PidLock::acquire` succeeding is a claim about the API. `/proc/locks` having
/// no `FLOCK` record for the sentinel inode is the fact underneath it, and it
/// is the fact a second PROCESS would contend with.
#[test]
#[cfg(target_os = "linux")]
fn releasing_the_lock_removes_the_kernel_record_despite_a_forked_child() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sentinel = PidLock::lock_path(dir.path());

    let held = PidLock::acquire(dir.path()).expect("acquire succeeds");
    let child = PinnedChild::spawn();

    let while_held = kernel_flocks_on(&sentinel);
    assert!(
        !while_held.is_empty(),
        "positive control: a held pid lock must appear in /proc/locks, or this \
         probe cannot observe the defect it exists to catch"
    );

    drop(held);
    let after_release = kernel_flocks_on(&sentinel);
    child.release();

    assert!(
        after_release.is_empty(),
        "a released pid lock must leave no lock in the kernel; a forked child \
         still references the open file description and {} record(s) survived: \
         {after_release:?}",
        after_release.len()
    );
}
