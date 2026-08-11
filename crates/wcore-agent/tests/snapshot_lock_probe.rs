//! Does publishing a session snapshot leave locks behind?
//!
//! `replace_file_atomically_inner` locks every replacement inode before it is
//! published - the snapshot file and the `.authority` head file as well as the
//! journal itself. Those locks were released only by `close(2)`, which is not
//! the same thing: `flock` binds to the OPEN FILE DESCRIPTION, and `fork(2)`
//! duplicates the descriptor table, so a subprocess spawned while a published
//! handle is open keeps the lock alive until it execs or exits.
//!
//! This probe uses production APIs only - `SessionJournal::open` and
//! `SessionJournal::publish_snapshot` - and grades `/proc/locks`, the kernel's
//! own record, rather than any return value. The fork storm stands in for the
//! Bash tool, `git status`, the spawner and the forge, all of which fork on
//! other threads while a snapshot is being published.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wcore_agent::session_journal::SessionJournal;

const ITERATIONS: usize = 24;
const MAX_CHILDREN: usize = 400;

/// Children that fork and never exec, all released by closing one pipe.
struct ForkStorm {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<libc::pid_t>>>,
    children: Vec<libc::pid_t>,
    read_fd: i32,
    write_fd: i32,
}

impl ForkStorm {
    fn start() -> Self {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` is a live two-element array, the only argument pipe(2) reads.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe failed");
        let (read_fd, write_fd) = (fds[0], fds[1]);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut pids = Vec::new();
            while !thread_stop.load(Ordering::Relaxed) && pids.len() < MAX_CHILDREN {
                // SAFETY: the child calls only `close`/`read`/`_exit`, all
                // async-signal-safe, and runs no Rust destructor.
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
                pids.push(pid);
                std::thread::sleep(std::time::Duration::from_micros(500));
            }
            pids
        });
        Self {
            stop,
            thread: Some(thread),
            children: Vec::new(),
            read_fd,
            write_fd,
        }
    }

    /// Stop forking but keep every child alive, so any lock they pinned is
    /// still visible in `/proc/locks` when the sample is taken.
    fn freeze(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            self.children = thread.join().expect("fork storm thread");
        }
    }

    fn release(mut self) -> usize {
        self.freeze();
        // SAFETY: both descriptors are owned by this process and still open.
        unsafe { libc::close(self.write_fd) };
        for pid in std::mem::take(&mut self.children) {
            let mut status = 0i32;
            // SAFETY: `pid` is a child of this process.
            unsafe { libc::waitpid(pid, &mut status, 0) };
        }
        // SAFETY: owned descriptor, closed exactly once.
        unsafe { libc::close(self.read_fd) };
        MAX_CHILDREN
    }
}

/// Every `FLOCK` record the kernel holds against `path`'s inode.
fn kernel_flocks_on(path: &Path) -> Vec<String> {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    // `fs/locks.c` prints the target as `MAJOR:MINOR:INODE`, major and minor in
    // two hex digits and the inode in decimal. Match it as a whole token: a
    // substring match on the inode alone would collide with the pid column.
    let token = format!(
        "{:02x}:{:02x}:{}",
        libc::major(meta.dev()),
        libc::minor(meta.dev()),
        meta.ino()
    );
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

fn companions(journal: &Path) -> [PathBuf; 3] {
    let name = |suffix: &str| {
        let mut file = journal.file_name().expect("journal file name").to_owned();
        file.push(suffix);
        journal.with_file_name(file)
    };
    [journal.to_path_buf(), name(".snapshot"), name(".authority")]
}

/// The probe cannot certify anything unless it can see a lock that IS held.
#[test]
fn positive_control_a_held_journal_is_visible_in_proc_locks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("control.journal");
    let journal = SessionJournal::open(&path, "control").expect("open");
    let held = kernel_flocks_on(&path);
    drop(journal);
    assert!(
        !held.is_empty(),
        "an open session journal must appear in /proc/locks, or this probe is \
         blind to the class of defect it exists to catch"
    );
}

/// Publishing a snapshot must leave no lock behind on any file it published,
/// even though subprocesses forked throughout.
#[test]
fn publishing_snapshots_leaves_no_lock_behind_while_subprocesses_fork() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut storm = ForkStorm::start();

    let mut published: Vec<PathBuf> = Vec::new();
    for index in 0..ITERATIONS {
        let path = root.path().join(format!("probe-{index}.journal"));
        let journal = SessionJournal::open(&path, format!("probe-{index}")).expect("open");
        journal.publish_snapshot().expect("publish snapshot");
        journal.compact().expect("compact");
        drop(journal);
        published.extend(companions(&path));
    }

    // Stop forking, but keep the children: a lock one of them pinned is still
    // in the kernel right now, and that is the fact under test.
    storm.freeze();
    let leaked: Vec<(PathBuf, Vec<String>)> = published
        .iter()
        .map(|path| (path.clone(), kernel_flocks_on(path)))
        .filter(|(_, records)| !records.is_empty())
        .collect();
    storm.release();

    assert!(
        leaked.is_empty(),
        "publishing a snapshot must release every lock it took; {} of {} \
         published files were still locked after their journal was dropped, \
         with no live owner: {:?}",
        leaked.len(),
        published.len(),
        leaked.iter().take(4).collect::<Vec<_>>()
    );
}
