//! Anvil climb lease — per-workspace mutual exclusion (spec §6.5).
//!
//! A climb mutates a real user workspace (candidate promotion, §6.5). Two climbs
//! on the same workspace — or a climb racing the user's own edits — must never
//! interleave. This lease is the guard: an atomically-created lock file under the
//! workspace whose presence means "a climb owns this workspace". Acquiring is a
//! single `O_EXCL` create (no check-then-create race); the holder's pid is
//! recorded so a crashed climb can reclaim ITS OWN orphaned lease on resume
//! (compare-and-swap [`steal_stale`](ClimbLease::steal_stale)) without a fragile
//! cross-platform liveness probe, and dropping the lease releases it — but only
//! if we still hold it, so a successor's lease is never clobbered.
//!
//! Detecting whether a *different* pid's lease is stale (its process died) is
//! left to the resume path (A1.6), which knows the pid it crashed as; this
//! module provides the safe primitive, not a liveness oracle.
//!
//! Spec: `docs/design/2026-07-12-anvil-native-gated-forge-design.md` (v2) §6.5.

use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Path of the lease file relative to the workspace root.
const LEASE_REL_PATH: &str = ".wayland/anvil/climb.lease";

/// Errors acquiring or managing a climb lease.
#[derive(Debug, Error)]
pub enum LeaseError {
    /// The workspace is already leased by another climb (pid recorded in the
    /// lease). `pid` is 0 if the lease file was present but unreadable.
    #[error("workspace climb lease is already held (pid {pid})")]
    Held {
        /// The recorded holder pid (0 if unreadable).
        pid: u32,
        /// The lease file path.
        path: PathBuf,
    },
    /// Filesystem I/O failed.
    #[error("climb lease I/O failed at {path}: {source}")]
    Io {
        /// The lease file path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// A held per-workspace climb lease. Dropping it releases the lease (removing the
/// lock file), unless it was already [`release`](Self::release)d or the file is
/// no longer ours.
#[derive(Debug)]
#[must_use = "dropping the lease immediately releases it; hold it for the climb's lifetime"]
pub struct ClimbLease {
    path: PathBuf,
    pid: u32,
    released: bool,
}

impl ClimbLease {
    /// The lease file path for `workspace_root`.
    fn lease_path(workspace_root: impl AsRef<Path>) -> PathBuf {
        workspace_root.as_ref().join(LEASE_REL_PATH)
    }

    /// Atomically acquire the climb lease for `workspace_root`, creating the
    /// `.wayland/anvil/` directory as needed. Fails with [`LeaseError::Held`] if a
    /// lease already exists (the create is `O_EXCL`, so two racing acquirers can
    /// never both win).
    pub fn acquire(workspace_root: impl AsRef<Path>) -> Result<Self, LeaseError> {
        let path = Self::lease_path(&workspace_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| LeaseError::Io {
                path: path.clone(),
                source,
            })?;
        }
        let pid = std::process::id();
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(pid.to_string().as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|source| LeaseError::Io {
                        path: path.clone(),
                        source,
                    })?;
                Ok(Self {
                    path,
                    pid,
                    released: false,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(LeaseError::Held {
                pid: read_holder(&path).unwrap_or(0),
                path,
            }),
            Err(source) => Err(LeaseError::Io { path, source }),
        }
    }

    /// The pid recorded in this lease (this process).
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The lease file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Explicitly release the lease now (idempotent with `Drop`). Removes the lock
    /// file only if we still hold it.
    pub fn release(mut self) -> Result<(), LeaseError> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> Result<(), LeaseError> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        // Only remove the file if it is still OUR lease — never clobber a lease a
        // successor acquired after we (thought we) held it.
        match read_holder(&self.path) {
            Some(holder) if holder == self.pid => {
                fs::remove_file(&self.path).map_err(|source| LeaseError::Io {
                    path: self.path.clone(),
                    source,
                })
            }
            // Not ours (or already gone): nothing to release.
            _ => Ok(()),
        }
    }

    /// Read the current holder pid of `workspace_root`'s lease, or `None` if no
    /// lease is held.
    pub fn holder(workspace_root: impl AsRef<Path>) -> Result<Option<u32>, LeaseError> {
        let path = Self::lease_path(workspace_root);
        match fs::metadata(&path) {
            Ok(_) => Ok(Some(read_holder(&path).unwrap_or(0))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(LeaseError::Io { path, source }),
        }
    }

    /// Reclaim a lease known to be orphaned by a specific dead process: remove the
    /// lease file IFF its recorded holder is `expected_pid`. Returns whether a
    /// lease was broken. This is the resume path's safe reclaim — a climb that
    /// crashed as `expected_pid` (recorded in its journal) can clear its own stale
    /// lease without risking a live successor's, because a mismatched pid is left
    /// untouched.
    pub fn steal_stale(
        workspace_root: impl AsRef<Path>,
        expected_pid: u32,
    ) -> Result<bool, LeaseError> {
        let path = Self::lease_path(workspace_root);
        match read_holder(&path) {
            Some(holder) if holder == expected_pid => {
                fs::remove_file(&path).map_err(|source| LeaseError::Io {
                    path: path.clone(),
                    source,
                })?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

impl Drop for ClimbLease {
    fn drop(&mut self) {
        // Best-effort release; a failure here is not actionable at drop time.
        let _ = self.release_inner();
    }
}

/// Read the holder pid recorded in the lease file, or `None` if it is
/// absent/unreadable/malformed.
fn read_holder(path: &Path) -> Option<u32> {
    let mut file = fs::File::open(path).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    buf.trim().parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn acquire_creates_lease_and_records_pid() {
        let ws = workspace();
        let lease = ClimbLease::acquire(ws.path()).unwrap();
        assert_eq!(lease.pid(), std::process::id());
        assert!(lease.path().exists());
        assert_eq!(
            ClimbLease::holder(ws.path()).unwrap(),
            Some(std::process::id())
        );
    }

    #[test]
    fn second_acquire_is_rejected_while_held() {
        let ws = workspace();
        let _held = ClimbLease::acquire(ws.path()).unwrap();
        match ClimbLease::acquire(ws.path()) {
            Err(LeaseError::Held { pid, .. }) => assert_eq!(pid, std::process::id()),
            other => panic!("expected Held, got {other:?}"),
        }
    }

    #[test]
    fn dropping_releases_and_allows_reacquire() {
        let ws = workspace();
        {
            let _lease = ClimbLease::acquire(ws.path()).unwrap();
            assert!(ClimbLease::holder(ws.path()).unwrap().is_some());
        }
        // Drop released it.
        assert_eq!(ClimbLease::holder(ws.path()).unwrap(), None);
        // And a fresh acquire now succeeds.
        let _again = ClimbLease::acquire(ws.path()).unwrap();
    }

    #[test]
    fn explicit_release_is_idempotent_with_drop() {
        let ws = workspace();
        let lease = ClimbLease::acquire(ws.path()).unwrap();
        lease.release().unwrap(); // consumes; Drop won't double-remove
        assert_eq!(ClimbLease::holder(ws.path()).unwrap(), None);
    }

    #[test]
    fn holder_of_unleased_workspace_is_none() {
        let ws = workspace();
        assert_eq!(ClimbLease::holder(ws.path()).unwrap(), None);
    }

    #[test]
    fn steal_stale_breaks_only_a_matching_pid() {
        let ws = workspace();
        // Simulate an orphaned lease held by a foreign pid.
        let path = ClimbLease::lease_path(ws.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "424242").unwrap();

        // Wrong expected pid: leave it untouched.
        assert!(!ClimbLease::steal_stale(ws.path(), 999_999).unwrap());
        assert_eq!(ClimbLease::holder(ws.path()).unwrap(), Some(424_242));

        // Matching pid: reclaim it.
        assert!(ClimbLease::steal_stale(ws.path(), 424_242).unwrap());
        assert_eq!(ClimbLease::holder(ws.path()).unwrap(), None);
        // Now acquirable again.
        let _fresh = ClimbLease::acquire(ws.path()).unwrap();
    }

    #[test]
    fn release_does_not_clobber_a_successor_lease() {
        let ws = workspace();
        let path = ClimbLease::lease_path(ws.path());
        // We hold the lease...
        let lease = ClimbLease::acquire(ws.path()).unwrap();
        // ...but a "successor" replaces the file with a different holder (as if we
        // had been considered stale and reclaimed). Our Drop must not remove it.
        fs::write(&path, "777777").unwrap();
        drop(lease);
        assert_eq!(
            ClimbLease::holder(ws.path()).unwrap(),
            Some(777_777),
            "our release must not clobber a successor's lease"
        );
    }
}
