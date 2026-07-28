//! The single-owner scheduling lease.
//!
//! Phase 24 plan 24-02, Task 1.
//!
//! # The defect this closes
//!
//! Before this module, schedule ownership was ASSUMED rather than held. The
//! runner is spawned per session at engine boot (`wcore-agent`'s bootstrap)
//! and can ALSO be spawned as a detached background process against the same
//! `jobs.json` (`wayland-core cron daemon`). Two owners against one store is
//! a double-fire, and the only thing standing between that and a duplicated
//! job was the store's advance-on-fire bookkeeping — a read-then-write race,
//! not a guarantee.
//!
//! # The model
//!
//! Exactly one process is the OWNER of a schedule directory. Every other
//! process attaching to the same directory is an OBSERVER: it may read jobs,
//! report status and show history, and it never fires. Ownership is claimed,
//! held for as long as the claiming process lives, and released explicitly
//! at drain.
//!
//! # Why the exclusion is an owned OS lock and not a timestamp
//!
//! Reclaiming a dead holder's lease must require PROOF the holder is gone. A
//! timestamp comparison ("the lease looks old, take it") is not proof: a
//! healthy holder that was stopped for longer than the heuristic loses its
//! schedule to a second firer, which is the exact double-fire this module
//! exists to prevent. An `flock`/`LockFileEx` claim is released by the
//! OPERATING SYSTEM when the holding descriptor closes — including on
//! `SIGKILL`, on a panic, and on a power loss followed by a reboot. So
//! "the lock is acquirable" IS the proof of death, and it is the same
//! liveness story `wcore-gateway`'s pid lock tells for the gateway home.
//! One home, one liveness story.
//!
//! Neither is the recorded pid the proof: an unrelated process that inherits
//! a recycled identifier cannot hold this lock, so it cannot masquerade as
//! the schedule owner.
//!
//! # `flock` and not `fcntl`
//!
//! POSIX `fcntl` record locks are owned by the PROCESS: a second `open` plus
//! lock from within one process succeeds by merging with the first. The
//! single-owner test drives two runners inside one test process, so under
//! `fcntl` that test could never go red. `flock` locks are owned by the OPEN
//! FILE DESCRIPTION and genuinely conflict across two opens; `LockFileEx` on
//! Windows is owned by the HANDLE and behaves the same way.
//!
//! # Recorded divergence from "no duplicate code across crates"
//!
//! `wcore-gateway::pidlock` already implements this same primitive. It is not
//! reused here, and that is deliberate and constrained rather than an
//! oversight:
//!
//! - the dependency edge runs `wcore-gateway` → `wcore-cron` (the gateway's
//!   automation plane acquires this lease), so `wcore-cron` cannot depend on
//!   `wcore-gateway` without a cycle;
//! - `wcore-agent`'s session-boot runner must also attempt this lease, and
//!   `wcore-agent` has no `wcore-gateway` dependency;
//! - extracting the primitive into a lower crate, or adding `libc` /
//!   `windows-sys` to `wcore-cron`, both require a `Cargo.lock` edit, which
//!   plan 24-02 forbids outright because it is a shared seam.
//!
//! The FFI is therefore declared locally, exactly as `store.rs` already
//! declares `getuid` locally for the same "keep this crate's dependency
//! surface tiny" reason. Unification is filed as backlog item F24-02-L1.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// The one-byte sentinel the OS lock is taken on. Nothing ever reads it, so
/// the mandatory Windows lock over its single byte excludes no reader.
const LEASE_LOCK_FILE: &str = "schedule.lock";
/// The freely readable record naming the current owner. Never locked, so an
/// observer can identify the owner while the owner holds the lock.
const LEASE_RECORD_FILE: &str = "schedule.owner";

/// Why a lease could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// Another LIVE process owns this schedule. `pid` comes from the readable
    /// record so the refusal is actionable; the refusal itself rests on the
    /// lock, never on the pid.
    #[error("schedule already owned by a live process (pid {pid})")]
    AlreadyOwned { pid: u32 },

    #[error("schedule directory is not usable: {0}")]
    Directory(#[source] std::io::Error),

    #[error("schedule lease could not be taken: {0}")]
    Lock(#[source] std::io::Error),

    #[error("schedule owner record could not be written: {0}")]
    Record(#[source] std::io::Error),
}

/// The readable record of who owns a schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub pid: u32,
    /// RFC3339 instant the owner claimed the schedule.
    pub acquired_at: String,
    /// What kind of process took it, for operator diagnostics only. This is
    /// self-reported and carries no authority.
    pub holder: String,
}

/// The role a process plays against one schedule directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseRole {
    /// This process fires the schedule.
    Owner,
    /// This process reads and reports and never fires.
    Observer,
}

/// The result of attempting to become the schedule owner.
///
/// There is no "failed" arm: contention is not an error, it is the OTHER
/// valid role. A session that boots alongside a running gateway is expected
/// to land here and carry on observing.
#[derive(Debug)]
pub enum LeaseAttempt {
    Owner(ScheduleLease),
    /// Refused because a live owner holds it. `holder_pid` is the recorded
    /// owner identity, or `None` when no record could be read.
    Observer {
        holder_pid: Option<u32>,
    },
}

impl LeaseAttempt {
    pub fn role(&self) -> LeaseRole {
        match self {
            Self::Owner(_) => LeaseRole::Owner,
            Self::Observer { .. } => LeaseRole::Observer,
        }
    }

    pub fn is_owner(&self) -> bool {
        matches!(self, Self::Owner(_))
    }

    /// The lease, when this attempt won ownership.
    pub fn into_lease(self) -> Option<ScheduleLease> {
        match self {
            Self::Owner(l) => Some(l),
            Self::Observer { .. } => None,
        }
    }
}

/// A cheap, clonable view of a lease that the tick loop consults.
///
/// The tick re-checks this IMMEDIATELY BEFORE every dispatch, not only once
/// at the top of the tick. A lease released mid-tick (a gateway entering
/// drain) must stop the loser from completing a fire it had already selected;
/// checking only at the top would let a selected job dispatch after ownership
/// was already handed over, which is the double-fire in a slower costume.
#[derive(Debug, Clone)]
pub struct LeaseHandle {
    owned: Arc<AtomicBool>,
    owner_pid: u32,
}

impl LeaseHandle {
    /// A handle that never owns anything. Used by an observer runner, and by
    /// the legacy un-leased entry points so their behaviour is unchanged.
    pub fn observer() -> Self {
        Self {
            owned: Arc::new(AtomicBool::new(false)),
            owner_pid: 0,
        }
    }

    /// A handle that always owns. This is the shape the pre-lease entry
    /// points (`tick_once`, `tick_once_with_history`) pass, so a process that
    /// never asked about ownership keeps firing exactly as it did before.
    pub fn unleased() -> Self {
        Self {
            owned: Arc::new(AtomicBool::new(true)),
            owner_pid: std::process::id(),
        }
    }

    pub fn is_owner(&self) -> bool {
        self.owned.load(Ordering::SeqCst)
    }

    pub fn role(&self) -> LeaseRole {
        if self.is_owner() {
            LeaseRole::Owner
        } else {
            LeaseRole::Observer
        }
    }

    pub fn owner_pid(&self) -> u32 {
        self.owner_pid
    }

    /// Stand this handle down. The lease's own `release`/`Drop` calls this;
    /// it is also the operation a drain performs, and the operation the
    /// mid-tick-loss test drives.
    pub fn revoke(&self) {
        self.owned.store(false, Ordering::SeqCst);
    }
}

/// An owned, exclusive claim on one schedule directory.
///
/// The claim lives for as long as the `File` inside is open. Dropping it —
/// including by process death — releases the OS lock, which is what makes a
/// dead owner's schedule reclaimable without any timeout heuristic.
#[derive(Debug)]
pub struct ScheduleLease {
    dir: PathBuf,
    /// Basename of this lease's owner record. Carried rather than assumed
    /// because `Drop` must remove the record it actually wrote — see
    /// [`ScheduleLease::attempt_named`].
    record_file: String,
    handle: LeaseHandle,
    _sentinel: File,
}

impl ScheduleLease {
    /// Path of the one-byte lock sentinel for `dir`.
    pub fn lock_path(dir: impl AsRef<Path>) -> PathBuf {
        dir.as_ref().join(LEASE_LOCK_FILE)
    }

    /// Path of the freely readable owner record for `dir`.
    pub fn record_path(dir: impl AsRef<Path>) -> PathBuf {
        dir.as_ref().join(LEASE_RECORD_FILE)
    }

    /// The schedule directory this lease was taken against.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A handle the tick loop consults.
    pub fn handle(&self) -> LeaseHandle {
        self.handle.clone()
    }

    /// Attempt to become the owner of `dir`.
    ///
    /// Returns [`LeaseAttempt::Observer`] — not an error — when a live owner
    /// already holds it, because contention is a valid role rather than a
    /// failure. Genuine failures (an unusable directory, a lock call that
    /// itself errored) are still errors.
    pub fn attempt(dir: impl AsRef<Path>, holder: &str) -> Result<LeaseAttempt, LeaseError> {
        Self::attempt_named(dir, holder, LEASE_LOCK_FILE, LEASE_RECORD_FILE)
    }

    /// [`attempt`](Self::attempt) with caller-chosen sentinel and record
    /// basenames.
    ///
    /// # Why this exists
    ///
    /// Phase 24 shipped this lease for the cron SCHEDULE, and the schedule was
    /// not the only thing in this workspace with exactly one legitimate owner
    /// per home. Inbound channel polling has the same shape and a sharper
    /// failure: polling is a DESTRUCTIVE read — Telegram's `getUpdates?offset=`
    /// permanently deletes, IMAP sets `\Seen`, Discord allows one gateway
    /// session per token — so a second poller does not duplicate a message, it
    /// DESTROYS it for the first, silently.
    ///
    /// The channel guard therefore reuses this primitive rather than declaring
    /// a second exclusion concept. Two mechanisms for one invariant is how the
    /// double-`ChannelManager` defect arose in the first place, and a lease
    /// whose release story differed from this one would be a second chance to
    /// reintroduce the stale-lock wedge this module's OS-owned lock exists to
    /// prevent.
    ///
    /// Only the FILE NAMES vary. The exclusion, the liveness story and the
    /// release-on-death guarantee are shared verbatim, which is the point.
    pub fn attempt_named(
        dir: impl AsRef<Path>,
        holder: &str,
        lock_file: &str,
        record_file: &str,
    ) -> Result<LeaseAttempt, LeaseError> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(LeaseError::Directory)?;

        let lock_path = dir.join(lock_file);
        let mut sentinel = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(LeaseError::Lock)?;

        // Exactly one byte, so the mandatory Windows lock covers one byte and
        // nothing anybody wants to read.
        if sentinel.metadata().map_err(LeaseError::Lock)?.len() != 1 {
            sentinel.set_len(0).map_err(LeaseError::Lock)?;
            sentinel.write_all(b"\0").map_err(LeaseError::Lock)?;
            sentinel.flush().map_err(LeaseError::Lock)?;
        }

        if !try_lock_exclusive(&sentinel).map_err(LeaseError::Lock)? {
            // Held, which by OS construction means the holder is alive.
            let holder_pid = Self::read_record_named(&dir, record_file).map(|r| r.pid);
            return Ok(LeaseAttempt::Observer { holder_pid });
        }

        let record = LeaseRecord {
            pid: std::process::id(),
            acquired_at: chrono::Utc::now().to_rfc3339(),
            holder: holder.to_string(),
        };
        write_record(&dir, &record, record_file).map_err(LeaseError::Record)?;

        Ok(LeaseAttempt::Owner(Self {
            dir,
            record_file: record_file.to_string(),
            handle: LeaseHandle {
                owned: Arc::new(AtomicBool::new(true)),
                owner_pid: std::process::id(),
            },
            _sentinel: sentinel,
        }))
    }

    /// Like [`attempt`](Self::attempt) but turns contention into an error.
    /// Used where the caller genuinely cannot proceed as an observer.
    pub fn acquire(dir: impl AsRef<Path>, holder: &str) -> Result<Self, LeaseError> {
        match Self::attempt(dir, holder)? {
            LeaseAttempt::Owner(l) => Ok(l),
            LeaseAttempt::Observer { holder_pid } => Err(LeaseError::AlreadyOwned {
                pid: holder_pid.unwrap_or(0),
            }),
        }
    }

    /// Read the owner record without taking the lock and without being
    /// blocked by it.
    pub fn read_record(dir: impl AsRef<Path>) -> Option<LeaseRecord> {
        Self::read_record_named(dir, LEASE_RECORD_FILE)
    }

    /// [`read_record`](Self::read_record) for a caller-chosen record basename.
    pub fn read_record_named(dir: impl AsRef<Path>, record_file: &str) -> Option<LeaseRecord> {
        let path = dir.as_ref().join(record_file);
        let mut buf = String::new();
        File::open(path).ok()?.read_to_string(&mut buf).ok()?;
        serde_json::from_str(&buf).ok()
    }

    /// Release the lease explicitly. This is what a gateway drain calls: it
    /// stands the handle down FIRST, so a tick already in progress abandons
    /// its selected fire rather than completing it after ownership was
    /// surrendered.
    pub fn release(self) {
        self.handle.revoke();
        // `Drop` performs the rest.
    }
}

impl Drop for ScheduleLease {
    fn drop(&mut self) {
        self.handle.revoke();
        // A clean release removes the record so a later read reports no owner
        // rather than naming a process that has exited. The OS releases the
        // lock itself when `_sentinel` closes — which is also why an UNCLEAN
        // death (SIGKILL, panic, power loss) still frees the lease: nothing
        // here has to run for the next process to acquire it.
        let _ = std::fs::remove_file(self.dir.join(&self.record_file));
    }
}

/// Write the owner record through a same-directory temporary plus a rename.
///
/// `std::fs::rename` maps to `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING` on
/// Windows, which replaces an existing destination, and no handle is held on
/// the destination while the rename runs.
fn write_record(dir: &Path, record: &LeaseRecord, record_file: &str) -> std::io::Result<()> {
    let tmp = dir.join(format!("{record_file}.{}.tmp", std::process::id()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(record)?.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(record_file))
}

// ---------------------------------------------------------------------------
// The exclusion primitive.
//
// Declared with local FFI rather than through `libc` / `windows-sys` for the
// reason recorded in the module documentation: adding either crate to
// `wcore-cron` is a `Cargo.lock` edit, which plan 24-02 forbids as a shared
// seam. `store.rs` already sets this precedent for `getuid`.
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod sys {
    use std::fs::File;

    // `flock` operation constants. Identical on Linux, macOS and the BSDs.
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    // `EWOULDBLOCK` is `EAGAIN` on Linux (11) and is 35 on macOS/BSD. Both are
    // checked because both mean "another open file description holds it",
    // which is contention rather than a failure.
    const EAGAIN_LINUX: i32 = 11;
    const EWOULDBLOCK_BSD: i32 = 35;

    unsafe extern "C" {
        #[link_name = "flock"]
        fn libc_flock(fd: i32, operation: i32) -> i32;
    }

    /// Take an exclusive, non-blocking lock. `Ok(false)` means another open
    /// file description holds it; `Err` means the attempt itself failed.
    pub(super) fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
        use std::os::unix::io::AsRawFd;
        // SAFETY: `file` owns a valid descriptor for the duration of the call,
        // and `flock` neither retains it nor writes through it.
        let rc = unsafe { libc_flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if rc == 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(code) if code == EAGAIN_LINUX || code == EWOULDBLOCK_BSD => Ok(false),
            _ => Err(err),
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::fs::File;

    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x0000_0001;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x0000_0002;
    const ERROR_LOCK_VIOLATION: i32 = 33;
    const ERROR_IO_PENDING: i32 = 997;

    /// Layout-compatible with the Win32 `OVERLAPPED` structure. Only the
    /// offset fields are read by `LockFileEx` for a synchronous call; the rest
    /// must be zero.
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        h_event: *mut c_void,
    }

    unsafe extern "system" {
        fn LockFileEx(
            h_file: *mut c_void,
            dw_flags: u32,
            dw_reserved: u32,
            n_number_of_bytes_to_lock_low: u32,
            n_number_of_bytes_to_lock_high: u32,
            lp_overlapped: *mut Overlapped,
        ) -> i32;
    }

    pub(super) fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
        use std::os::windows::io::AsRawHandle;

        // Exactly ONE byte at offset zero. On Windows the lock is MANDATORY,
        // so this range must be a range nothing else reads — it covers the
        // sentinel's single byte and nothing an operator or this crate reads.
        let mut ov = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            h_event: std::ptr::null_mut(),
        };
        // SAFETY: Win32 FFI over a handle `file` owns for the call's duration,
        // and a zeroed OVERLAPPED is the documented initial state for a
        // synchronous LockFileEx call.
        let ok = unsafe {
            LockFileEx(
                file.as_raw_handle() as *mut c_void,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut ov,
            )
        };
        if ok != 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(ERROR_LOCK_VIOLATION) | Some(ERROR_IO_PENDING) => Ok(false),
            _ => Err(err),
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    use std::fs::File;

    /// No exclusion primitive on this target. Refuse rather than pretend: a
    /// lease that always succeeds is a lease that guarantees nothing, and a
    /// silent always-owner would reintroduce the double-fire.
    pub(super) fn try_lock_exclusive(_file: &File) -> std::io::Result<bool> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no file-locking primitive on this target; schedule ownership cannot be proved",
        ))
    }
}

use sys::try_lock_exclusive;

/// Resolve the default schedule directory: the parent of the default job
/// store, i.e. `$WAYLAND_HOME/cron` or `~/.wayland/cron`.
pub fn default_lease_dir() -> Option<PathBuf> {
    crate::store::default_store_path().and_then(|p| p.parent().map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_attempt_in_one_process_is_refused() {
        // This is the assertion that would silently pass under `fcntl`, whose
        // record locks merge across two opens inside one process. It must go
        // red if the primitive is ever swapped for one owned by the process
        // rather than by the open file description.
        let dir = tempfile::tempdir().unwrap();
        let first = ScheduleLease::attempt(dir.path(), "first").unwrap();
        assert!(first.is_owner());

        let second = ScheduleLease::attempt(dir.path(), "second").unwrap();
        assert_eq!(second.role(), LeaseRole::Observer);
        match second {
            LeaseAttempt::Observer { holder_pid } => {
                assert_eq!(holder_pid, Some(std::process::id()));
            }
            LeaseAttempt::Owner(_) => unreachable!(),
        }
    }

    #[test]
    fn releasing_lets_the_next_attempt_win() {
        let dir = tempfile::tempdir().unwrap();
        let first = ScheduleLease::attempt(dir.path(), "first")
            .unwrap()
            .into_lease()
            .unwrap();
        let handle = first.handle();
        assert!(handle.is_owner());

        first.release();
        assert!(!handle.is_owner(), "release must stand the handle down");

        let second = ScheduleLease::attempt(dir.path(), "second").unwrap();
        assert!(second.is_owner(), "a released schedule must be reclaimable");
    }

    #[test]
    fn a_released_lease_leaves_no_owner_record() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _l = ScheduleLease::attempt(dir.path(), "holder")
                .unwrap()
                .into_lease()
                .unwrap();
            assert!(ScheduleLease::read_record(dir.path()).is_some());
        }
        assert!(
            ScheduleLease::read_record(dir.path()).is_none(),
            "a released lease must not leave a record naming a dead owner"
        );
    }

    #[test]
    fn an_observer_handle_never_owns() {
        let h = LeaseHandle::observer();
        assert!(!h.is_owner());
        assert_eq!(h.role(), LeaseRole::Observer);
    }

    #[test]
    fn the_sentinel_stays_one_byte() {
        // The mandatory Windows lock covers exactly the sentinel's byte. If
        // the sentinel ever grew, the locked range would still be one byte but
        // the file would carry content a reader might want, which is the
        // defect class the separate sentinel exists to avoid.
        let dir = tempfile::tempdir().unwrap();
        let _l = ScheduleLease::attempt(dir.path(), "holder").unwrap();
        let len = std::fs::metadata(ScheduleLease::lock_path(dir.path()))
            .unwrap()
            .len();
        assert_eq!(len, 1, "lock sentinel must stay exactly one byte");
    }

    #[test]
    fn the_owner_record_is_readable_while_the_lock_is_held() {
        // On Windows the lock is MANDATORY. If the lock had been taken on the
        // record itself, this read would fail while the owner holds it.
        let dir = tempfile::tempdir().unwrap();
        let _l = ScheduleLease::attempt(dir.path(), "gateway").unwrap();
        let raw = std::fs::read(ScheduleLease::record_path(dir.path()))
            .expect("owner record must be readable while the lease is held");
        assert!(!raw.is_empty());
        let rec = ScheduleLease::read_record(dir.path()).unwrap();
        assert_eq!(rec.holder, "gateway");
        assert_eq!(rec.pid, std::process::id());
    }
}
