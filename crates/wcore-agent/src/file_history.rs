//! W8b C.6 — `FileHistory` snapshot store backed by the root-level
//! `RealFs` (NOT the per-tool sandboxed VFS).
//!
//! Audit F9 (resolved here): rollback is a *meta*-operation on the global
//! edit history. The shadow directory holding pre-edit snapshots is
//! engine state, not project state — it lives outside any per-agent
//! sandbox. If a sub-agent with a `SandboxedFs` tried to write the
//! shadow path through its scoped VFS, the sandbox would reject paths
//! outside the sub-agent's root and rollback would silently break.
//!
//! The explicit design point: snapshots **read** the live bytes through
//! the *per-call* `VirtualFs` (so sandboxed sub-agents still snapshot
//! what they can see) but **write** the shadow copy through the
//! root-level `vfs_root: Arc<RealFs>` (so the shadow dir always lands on
//! the real filesystem regardless of the caller's sandbox).
//!
//! Layout on disk:
//!   `<shadow_root>/<digest-of-path>/<n>.bin`
//! where `<digest-of-path>` is the hex of `sha2::Sha256::digest(path)`
//! truncated to 16 hex chars (8 bytes) — sufficient for bucketing — and
//! `<n>` is a monotonically incremented per-path counter modulo
//! `MAX_SNAPSHOTS_PER_FILE`. The most-recent snapshot is index 0; index 9
//! is the oldest surviving snapshot under the 10-cap default.
//!
//! Wave SD SECURITY MAJOR #17 (closed here):
//!
//! The content digest used by `RollbackTool` to detect external
//! modifications is now SHA-256 (`[u8; 32]`) instead of a 64-bit
//! `DefaultHasher::finish()`. The previous 64-bit hash was
//! birthday-collidable in ~2^32 work — a sub-agent that gained write
//! access could craft a colliding payload and slip past the rollback
//! conflict-detection guard. SHA-256 makes that economically
//! impossible.
//!
//! Migration: the shadow store is per-session ephemeral state. We
//! intentionally do NOT carry old `u64` digests across boots — the
//! `last_engine_digest` field is reset on every `FileHistory::new`
//! anyway (it lives in the in-memory `cursors` map). Any pre-existing
//! shadow `.bin` files on disk remain readable (they're just bytes);
//! only the conflict-detection check changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use thiserror::Error;

use wcore_tools::vfs::{RealFs, VfsError, VirtualFs};

/// Maximum snapshots kept per file. FIFO eviction past this cap.
pub const MAX_SNAPSHOTS_PER_FILE: usize = 10;

/// Wave SD — SHA-256 byte digest. `[u8; 32]` keeps it stack-allocated +
/// `Copy` so the rollback guard can compare without heap traffic.
pub type ByteDigest = [u8; 32];

#[derive(Debug, Error)]
pub enum FileHistoryError {
    #[error("vfs error: {0}")]
    Vfs(#[from] VfsError),
    #[error(
        "requested snapshot step {requested} but only {available} snapshots available for {path:?}"
    )]
    StepOutOfRange {
        path: PathBuf,
        requested: usize,
        available: usize,
    },
    #[error("no snapshots recorded for {path:?}")]
    NoSnapshots { path: PathBuf },
}

/// Per-path bookkeeping: how many snapshots have been written (modulo
/// `MAX_SNAPSHOTS_PER_FILE`) and the next slot to write into, plus the
/// digest of the engine's most-recent post-write state (used by
/// `RollbackTool` to detect external modifications).
#[derive(Debug, Default, Clone, Copy)]
struct PathCursor {
    /// Total number of snapshots ever written for this path. Saturates at
    /// `usize::MAX`; only used to compute `snapshots_count()` (capped to
    /// `MAX_SNAPSHOTS_PER_FILE`) and the slot order for reads.
    total: usize,
    /// Digest of the bytes the engine last wrote to `path` (via the
    /// `Write`/`Edit` tools, which call `record_post_write_digest`).
    /// `None` if no engine-side write has been recorded — the conflict
    /// guard in `RollbackTool` then skips the external-change check
    /// (we have nothing to compare against).
    last_engine_digest: Option<ByteDigest>,
}

/// Snapshot store. Cheap to clone (Arc fields).
#[derive(Clone)]
pub struct FileHistory {
    /// Root-level filesystem; NOT sandboxed. All shadow-dir writes go
    /// through this handle so the shadow dir always lands on the real
    /// disk regardless of the caller's `ctx.vfs`.
    vfs_root: Arc<RealFs>,
    /// Directory where snapshot bytes live, e.g.
    /// `<project>/.wayland-core/shadow/`.
    shadow_root: PathBuf,
    /// Per-path cursors, keyed by the canonical input path.
    cursors: Arc<Mutex<HashMap<PathBuf, PathCursor>>>,
}

impl FileHistory {
    /// Build a new history store.
    ///
    /// `vfs_root` is the engine's root-level filesystem; it MUST be a
    /// `RealFs` (or test double of equivalent shape) — never the per-call
    /// `ctx.vfs` of a sandboxed sub-agent.
    pub fn new(vfs_root: Arc<RealFs>, shadow_root: PathBuf) -> Self {
        Self {
            vfs_root,
            shadow_root,
            cursors: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Snapshot the bytes the caller can currently see at `path`. Reads
    /// happen through `per_call_vfs` (so sandboxed sub-agents only
    /// capture bytes they have visibility to); the resulting shadow file
    /// is written via `self.vfs_root` (so the shadow dir always lands on
    /// real disk).
    pub async fn snapshot(
        &self,
        path: &Path,
        per_call_vfs: &dyn VirtualFs,
    ) -> Result<(), FileHistoryError> {
        let bytes = per_call_vfs.read(path).await?;
        let next = {
            let mut cursors = self.cursors.lock();
            let entry = cursors.entry(path.to_path_buf()).or_default();
            let n = entry.total;
            entry.total = entry.total.saturating_add(1);
            n
        };
        let slot = next % MAX_SNAPSHOTS_PER_FILE;
        let shadow_path = self.shadow_path_for(path, slot);
        self.vfs_root.write(&shadow_path, &bytes).await?;
        Ok(())
    }

    /// Number of snapshots currently retained for `path` (saturating at
    /// `MAX_SNAPSHOTS_PER_FILE`).
    pub async fn snapshots_count(&self, path: &Path) -> usize {
        let cursors = self.cursors.lock();
        cursors
            .get(path)
            .map(|c| c.total.min(MAX_SNAPSHOTS_PER_FILE))
            .unwrap_or(0)
    }

    /// Read snapshot at offset `steps_back` (0 = most recent).
    ///
    /// Errors if no snapshots exist for the path, or `steps_back >=
    /// snapshots_count()`.
    pub async fn read_snapshot(
        &self,
        path: &Path,
        steps_back: usize,
    ) -> Result<Vec<u8>, FileHistoryError> {
        let (total, available) = {
            let cursors = self.cursors.lock();
            let c = cursors
                .get(path)
                .copied()
                .ok_or_else(|| FileHistoryError::NoSnapshots {
                    path: path.to_path_buf(),
                })?;
            (c.total, c.total.min(MAX_SNAPSHOTS_PER_FILE))
        };
        if steps_back >= available {
            return Err(FileHistoryError::StepOutOfRange {
                path: path.to_path_buf(),
                requested: steps_back,
                available,
            });
        }
        // Most-recent snapshot's slot = (total - 1) % MAX.
        // Step n back from that is (total - 1 - n) % MAX.
        let absolute = total - 1 - steps_back;
        let slot = absolute % MAX_SNAPSHOTS_PER_FILE;
        let shadow_path = self.shadow_path_for(path, slot);
        Ok(self.vfs_root.read(&shadow_path).await?)
    }

    /// SHA-256 of the last snapshot bytes for `path`, used by tooling
    /// that wants to compare the live file to its most-recent
    /// pre-write snapshot. Returns `None` if no snapshots exist.
    pub async fn last_snapshot_digest(&self, path: &Path) -> Option<ByteDigest> {
        let bytes = self.read_snapshot(path, 0).await.ok()?;
        Some(byte_digest(&bytes))
    }

    /// Record the digest of the bytes the engine just wrote to `path`.
    /// Called by `Write`/`Edit` AFTER a successful write so `RollbackTool`
    /// can later detect external modifications by comparing the live
    /// file's digest against this recorded value.
    pub fn record_post_write_digest(&self, path: &Path, bytes: &[u8]) {
        let mut cursors = self.cursors.lock();
        let entry = cursors.entry(path.to_path_buf()).or_default();
        entry.last_engine_digest = Some(byte_digest(bytes));
    }

    /// Returns the digest of the engine's most-recent post-write bytes for
    /// `path`, or `None` if no engine write has been recorded. Used by
    /// `RollbackTool` to gate its external-change guard.
    pub fn last_engine_write_digest(&self, path: &Path) -> Option<ByteDigest> {
        self.cursors
            .lock()
            .get(path)
            .and_then(|c| c.last_engine_digest)
    }

    fn shadow_path_for(&self, path: &Path, slot: usize) -> PathBuf {
        self.shadow_root
            .join(path_bucket(path))
            .join(format!("{slot}.bin"))
    }
}

/// 16-hex-char path bucket (8 bytes of SHA-256 of the path) — sufficient
/// for distinct project paths within a session. Wave SD upgraded this
/// from `DefaultHasher` to `Sha256` so it inherits the same crypto
/// guarantees as `byte_digest`; the per-path collision risk is now
/// negligible.
fn path_bucket(path: &Path) -> String {
    let mut h = Sha256::new();
    h.update(path.as_os_str().as_encoded_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// SHA-256 byte content digest. Wave SD — closes SECURITY MAJOR #17 by
/// replacing the previous 64-bit `DefaultHasher` (birthday-collidable
/// in ~2^32 work) with a cryptographic 256-bit hash. The cost is
/// microseconds per write — irrelevant for tool-call cadence.
pub fn byte_digest(bytes: &[u8]) -> ByteDigest {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_is_stable_for_same_path() {
        let p = Path::new("/tmp/foo/bar.txt");
        assert_eq!(path_bucket(p), path_bucket(p));
    }

    #[test]
    fn bucket_differs_for_distinct_paths() {
        let a = Path::new("/tmp/a.txt");
        let b = Path::new("/tmp/b.txt");
        assert_ne!(path_bucket(a), path_bucket(b));
    }

    #[test]
    fn byte_digest_is_32_bytes_sha256() {
        let d = byte_digest(b"hello world");
        assert_eq!(d.len(), 32);
        // Sanity: known SHA-256 of "hello world" starts with b94d27b9...
        assert_eq!(d[0], 0xb9);
        assert_eq!(d[1], 0x4d);
        assert_eq!(d[2], 0x27);
    }

    #[test]
    fn byte_digest_distinct_for_distinct_input() {
        assert_ne!(byte_digest(b"foo"), byte_digest(b"bar"));
    }

    #[test]
    fn byte_digest_stable_for_same_input() {
        assert_eq!(byte_digest(b"same"), byte_digest(b"same"));
    }
}
