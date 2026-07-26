//! The gateway's single-instance lock, its readable status record, and the
//! one place platform path/liveness differences live.
//!
//! Phase 24 plan 24-01, Task 1. This absorbs the daemon machinery that
//! `crates/wcore-cli/src/cron.rs` carried (home resolution, pid-file
//! staleness, the two-armed liveness probe) so the workspace has ONE
//! detach/liveness story per home rather than two that can both claim it.
//!
//! # The four Windows defect classes this answers
//!
//! 1. **Mandatory locking.** On Windows a file lock is MANDATORY, not
//!    advisory: a lock over the pid file would exclude this crate's own
//!    status reader. The lock therefore lives on a SEPARATE one-byte
//!    sentinel file (`gateway.lock`) that no reader ever opens, and the
//!    readable record (`gateway.pid`) is never locked at all. The
//!    `pidlock_hostile` suite asserts both halves.
//! 2. **Delete-bearing handles.** Nothing here holds a handle over the
//!    HOME DIRECTORY — only over a file inside it — so a working-directory
//!    change into that home is never blocked.
//! 3. **Verbatim canonical paths.** `canonicalize` returns `\\?\`-prefixed
//!    verbatim form on Windows, which other tooling cannot parse. The
//!    prefix is stripped inside [`normalise_path`], and normalisation is
//!    applied at the COMPARISON boundary on BOTH operands rather than only
//!    at storage — a store-side-only normalisation is a fail-open, because
//!    the caller's operand is then never normalised at all.
//! 4. **Recycled process identifiers.** Exclusion is proved by an OWNED OS
//!    lock, never by the pid value: an unrelated process that inherits a
//!    recycled identifier cannot hold this lock, so it cannot masquerade as
//!    the gateway (threat T-24-01-02).
//!
//! # Why `flock`/`LockFileEx` and not `fcntl`
//!
//! POSIX `fcntl` record locks are owned by the PROCESS: a second `open` +
//! lock from the same process succeeds by merging with the first, so a
//! second gateway launched inside one process would be silently admitted
//! and the exclusion test could never go red. `flock` locks are owned by
//! the OPEN FILE DESCRIPTION and genuinely conflict across two opens.
//! `LockFileEx` on Windows is owned by the HANDLE and behaves the same way.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The single-byte sentinel the OS lock is taken on.
const LOCK_FILE: &str = "gateway.lock";
/// The freely readable status record. Never locked.
const RECORD_FILE: &str = "gateway.pid";

#[derive(Debug, thiserror::Error)]
pub enum PidLockError {
    /// Another live gateway holds this home. `pid` is the holder's process
    /// identity as recorded, so an operator can act on it.
    #[error("gateway already running for this home (pid {pid})")]
    AlreadyHeld { pid: u32 },

    #[error("gateway home is not usable: {0}")]
    Home(#[source] std::io::Error),

    #[error("gateway lock could not be taken: {0}")]
    Lock(#[source] std::io::Error),

    #[error("gateway status record could not be written: {0}")]
    Record(#[source] std::io::Error),
}

/// The readable status record a `gateway status` from a SECOND process
/// reads while the first holds the lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: u32,
    /// The home this record was taken against, already normalised. Stored
    /// normalised AND compared normalised — see the module note on why
    /// storage-side-only normalisation is a fail-open.
    pub home: PathBuf,
    /// RFC3339 instant the holder acquired the lock.
    pub started_at: String,
    /// The binary the holder was launched from, when it could be resolved.
    pub binary_path: Option<PathBuf>,
}

/// An owned, exclusive claim on one gateway home.
///
/// The claim lives for as long as the `File` inside is open: both `flock`
/// and `LockFileEx` release when the descriptor/handle closes, including on
/// abnormal termination. That is what makes a crashed holder's home
/// reclaimable without a timeout heuristic.
#[derive(Debug)]
pub struct PidLock {
    home: PathBuf,
    _sentinel: File,
}

impl PidLock {
    /// Path of the one-byte lock sentinel for `home`.
    pub fn lock_path(home: impl AsRef<Path>) -> PathBuf {
        normalise_path(home).join(LOCK_FILE)
    }

    /// Path of the freely readable status record for `home`.
    pub fn record_path(home: impl AsRef<Path>) -> PathBuf {
        normalise_path(home).join(RECORD_FILE)
    }

    /// The normalised home this lock was taken against.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Claim `home` exclusively, or refuse by name.
    ///
    /// A stale record left by a crashed holder is reclaimed, because the OS
    /// released that holder's lock when its process died. Reclamation
    /// therefore requires the lock to be genuinely acquirable — never a
    /// timestamp comparison, and never the pid value alone.
    pub fn acquire(home: impl AsRef<Path>) -> Result<Self, PidLockError> {
        let home = normalise_path(home);
        std::fs::create_dir_all(&home).map_err(PidLockError::Home)?;
        // Re-normalise now that the directory certainly exists, so a home
        // created by this call still compares equal to the same home
        // reached by another representation later.
        let home = normalise_path(&home);

        let lock_path = home.join(LOCK_FILE);
        let mut sentinel = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(PidLockError::Lock)?;

        // Exactly one byte, so the mandatory Windows lock covers one byte
        // and nothing anybody wants to read.
        if sentinel.metadata().map_err(PidLockError::Lock)?.len() != 1 {
            sentinel.set_len(0).map_err(PidLockError::Lock)?;
            sentinel.write_all(b"\0").map_err(PidLockError::Lock)?;
            sentinel.flush().map_err(PidLockError::Lock)?;
        }

        if !try_lock_exclusive(&sentinel).map_err(PidLockError::Lock)? {
            // The lock is held, which by OS construction means the holder is
            // alive. Name it from the record so the refusal is actionable.
            let pid = Self::read_record(&home).map(|r| r.pid).unwrap_or(0);
            return Err(PidLockError::AlreadyHeld { pid });
        }

        let record = PidRecord {
            pid: std::process::id(),
            home: home.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
            binary_path: std::env::current_exe().ok(),
        };
        write_record(&home, &record).map_err(PidLockError::Record)?;

        Ok(Self {
            home,
            _sentinel: sentinel,
        })
    }

    /// Read the status record without taking the lock and without being
    /// blocked by it. Returns `None` when no record exists or it is
    /// unparsable.
    pub fn read_record(home: impl AsRef<Path>) -> Option<PidRecord> {
        let path = normalise_path(home).join(RECORD_FILE);
        let mut buf = String::new();
        File::open(path).ok()?.read_to_string(&mut buf).ok()?;
        serde_json::from_str(&buf).ok()
    }

    /// Seed `home` with a record for a holder that is NOT running, without
    /// taking the lock — exactly the state a crashed process leaves behind.
    ///
    /// Exposed because the reclamation case is an integration test in a
    /// separate crate and cannot reach a private helper. It writes a record
    /// and nothing else; it can neither take nor release a lock.
    #[doc(hidden)]
    pub fn write_stale_record_for_test(home: impl AsRef<Path>, pid: u32) {
        let home = normalise_path(home);
        let _ = std::fs::create_dir_all(&home);
        let record = PidRecord {
            pid,
            home: home.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
            binary_path: None,
        };
        let _ = write_record(&home, &record);
    }
}

impl Drop for PidLock {
    fn drop(&mut self) {
        // A clean release removes the record so a later status read reports
        // "not running" rather than naming a process that has exited. The
        // OS releases the lock itself when `_sentinel` closes.
        let _ = std::fs::remove_file(self.home.join(RECORD_FILE));
    }
}

/// Write the record through a same-directory temporary plus a rename.
///
/// `std::fs::rename` maps to `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`
/// on Windows, which replaces an existing destination — the plain
/// `MoveFile` form the 20A handoff records as rejected is not used, and no
/// handle is held on the destination while the rename runs.
fn write_record(home: &Path, record: &PidRecord) -> std::io::Result<()> {
    let tmp = home.join(format!("{RECORD_FILE}.{}.tmp", std::process::id()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(record)?.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, home.join(RECORD_FILE))
}

/// Normalise a path to the one representation this crate compares.
///
/// Apply this at the COMPARISON boundary to BOTH operands. Applying it only
/// where a path is stored leaves the caller's operand un-normalised, which
/// is a fail-open: the two then differ whenever the caller reached the same
/// directory by a different route.
pub fn normalise_path(p: impl AsRef<Path>) -> PathBuf {
    let p = p.as_ref();
    match std::fs::canonicalize(p) {
        Ok(c) => strip_verbatim(c),
        // The path does not exist yet (an install that has not run, a home
        // being created). Fall back to a lexical normalisation so the two
        // operands still agree.
        Err(_) => lexical_normalise(p),
    }
}

/// Strip the `\\?\` verbatim prefix Windows `canonicalize` returns.
///
/// A verbatim path is correct but other tooling cannot parse it, so it
/// never leaves this function.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return p;
    }
    let s = p.to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p
}

/// Lexical normalisation for a path that does not exist on disk: drop `.`
/// components and resolve `..` against the accumulated result.
fn lexical_normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop a real component; never eat the root or a prefix,
                // and keep a leading `..` that has nothing to pop — two
                // genuinely different paths must not collapse to one.
                let can_pop = matches!(out.components().next_back(), Some(Component::Normal(_)));
                if !(can_pop && out.pop()) {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Returns `true` when a process with `pid` appears to be alive.
///
/// Moved from `crates/wcore-cli/src/cron.rs` unchanged in meaning, so the
/// workspace has ONE liveness story. The Windows arm in particular is the
/// audit W-1 fix (`OpenProcess` + `GetExitCodeProcess`); the previous stub
/// returned a hardcoded `false` and every Windows daemon start spawned a
/// duplicate.
pub fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return true;
        }
        // macOS has no /proc.
        // SAFETY: kill(pid, 0) performs an existence/permission check and
        // sends no signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: Win32 FFI. OpenProcess returns NULL on failure.
        let handle: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        // SAFETY: handle is non-NULL and exit_code is a local u32.
        let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
        // SAFETY: opened by us, closed exactly once.
        unsafe { CloseHandle(handle) };
        ok != 0 && exit_code as i32 == STILL_ACTIVE
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Take an exclusive, non-blocking lock. `Ok(false)` means another open
/// file description holds it; `Err` means the attempt itself failed.
#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `file` owns a valid descriptor for the duration of the call.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => Ok(false),
        _ => Err(err),
    }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    // Exactly ONE byte at offset zero. On Windows the lock is MANDATORY,
    // so this range is a range nothing else may read — it covers the
    // sentinel's single byte and nothing an operator or this crate reads.
    // SAFETY: a zeroed OVERLAPPED is the documented initial state for a
    // synchronous LockFileEx call.
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    // SAFETY: Win32 FFI over a handle `file` owns for the call's duration.
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle() as _,
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
    const ERROR_LOCK_VIOLATION: i32 = 33;
    const ERROR_IO_PENDING: i32 = 997;
    match err.raw_os_error() {
        Some(ERROR_LOCK_VIOLATION) | Some(ERROR_IO_PENDING) => Ok(false),
        _ => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_normalise_drops_cur_dir() {
        assert_eq!(
            lexical_normalise(Path::new("a/./b/./c")),
            PathBuf::from("a/b/c")
        );
    }

    #[test]
    fn lexical_normalise_resolves_parent_dir() {
        assert_eq!(
            lexical_normalise(Path::new("a/b/../c")),
            PathBuf::from("a/c")
        );
    }

    #[test]
    fn lexical_normalise_keeps_leading_parent_dirs() {
        // Nothing to pop — a leading `..` is meaningful and must survive,
        // otherwise two genuinely different paths would compare equal.
        assert_eq!(lexical_normalise(Path::new("../a")), PathBuf::from("../a"));
    }

    #[test]
    fn a_nonexistent_home_still_normalises() {
        let p = normalise_path("/definitely/not/here/./x/../y");
        assert!(
            !p.to_string_lossy().contains("/./"),
            "normalisation must apply to a home that does not exist yet: {p:?}"
        );
    }
}
