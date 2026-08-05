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
//! F13 persists both the snapshot cursor and the exact committed postimage
//! authority next to the shadow bytes. A process restart therefore cannot
//! silently turn an identity-guarded rollback into an unconditional write.
//! Legacy shadow buckets without cursor metadata remain inert: their bytes are
//! never treated as rollback authority.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use wcore_tools::vfs::{
    FileContentIdentity, FileObjectIdentity, FileObservation, RealFs, VfsError, VirtualFs,
};

/// Maximum snapshots kept per file. FIFO eviction past this cap.
pub const MAX_SNAPSHOTS_PER_FILE: usize = 10;

const PATH_CURSOR_VERSION: u32 = 1;
const PATH_CURSOR_FILE: &str = "cursor-v1.json";

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
    #[error("invalid durable file-history state for {path:?}: {reason}")]
    InvalidState { path: PathBuf, reason: String },
    #[error("failed to serialize durable file-history state: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Per-path bookkeeping: how many snapshots have been written (modulo
/// `MAX_SNAPSHOTS_PER_FILE`), the next slot, and the exact postimage authority
/// used by `RollbackTool`.
#[derive(Debug, Default, Clone)]
struct PathCursor {
    /// Total number of snapshots ever written for this path. Saturates at
    /// `usize::MAX`; only used to compute `snapshots_count()` (capped to
    /// `MAX_SNAPSHOTS_PER_FILE`) and the slot order for reads.
    total: usize,
    /// Exact committed postimage which authorizes one rollback. Missing
    /// authority always fails closed.
    rollback_authority: Option<RollbackAuthority>,
}

#[derive(Debug, Clone)]
pub(crate) struct RollbackAuthority {
    pub(crate) postimage: FileContentIdentity,
    pub(crate) object: FileObjectIdentity,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurablePathCursor {
    version: u32,
    path: PathBuf,
    total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback_authority: Option<DurableRollbackAuthority>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableRollbackAuthority {
    sha256: ByteDigest,
    len: u64,
    object: FileObjectIdentity,
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
    /// Serializes lazy metadata loading and durable cursor updates within this
    /// process. The VFS owns the stronger cross-process mutation boundary.
    cursor_io: Arc<tokio::sync::Mutex<()>>,
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
            cursor_io: Arc::new(tokio::sync::Mutex::new(())),
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
        let _io = self.cursor_io.lock().await;
        let mut cursor = self.load_cursor_locked(path).await?;
        let next = cursor.total;
        let slot = next % MAX_SNAPSHOTS_PER_FILE;
        let shadow_path = self.shadow_path_for(path, slot);
        self.vfs_root.write(&shadow_path, &bytes).await?;
        sync_parent_directory(&shadow_path).await?;
        cursor.total = cursor.total.saturating_add(1);
        self.persist_cursor_locked(path, &cursor).await?;
        self.cursors.lock().insert(path.to_path_buf(), cursor);
        Ok(())
    }

    /// Number of snapshots currently retained for `path` (saturating at
    /// `MAX_SNAPSHOTS_PER_FILE`).
    pub async fn snapshots_count(&self, path: &Path) -> usize {
        let _io = self.cursor_io.lock().await;
        self.load_cursor_locked(path)
            .await
            .map(|cursor| cursor.total.min(MAX_SNAPSHOTS_PER_FILE))
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
        let _io = self.cursor_io.lock().await;
        let cursor = self.load_cursor_locked(path).await?;
        let (total, available) = (cursor.total, cursor.total.min(MAX_SNAPSHOTS_PER_FILE));
        if available == 0 {
            return Err(FileHistoryError::NoSnapshots {
                path: path.to_path_buf(),
            });
        }
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

    /// Persist the exact committed file identity which authorizes one future
    /// rollback. Content-only evidence is deliberately insufficient.
    pub async fn record_committed_postimage(
        &self,
        path: &Path,
        vfs: &dyn VirtualFs,
    ) -> Result<(), FileHistoryError> {
        let observed = vfs.observe_file(path).await?;
        let FileObservation::Present(postimage) = observed.observation else {
            return Err(FileHistoryError::InvalidState {
                path: path.to_path_buf(),
                reason: "committed postimage is absent".to_string(),
            });
        };
        if observed.object.authority.is_empty()
            || observed.object.path.as_os_str().is_empty()
            || observed.object.file.is_none()
        {
            return Err(FileHistoryError::InvalidState {
                path: path.to_path_buf(),
                reason: "committed postimage lacks an exact object identity".to_string(),
            });
        }

        let _io = self.cursor_io.lock().await;
        let mut cursor = self.load_cursor_locked(path).await?;
        cursor.rollback_authority = Some(RollbackAuthority {
            postimage,
            object: observed.object,
        });
        self.persist_cursor_locked(path, &cursor).await?;
        self.cursors.lock().insert(path.to_path_buf(), cursor);
        Ok(())
    }

    pub(crate) async fn rollback_authority(
        &self,
        path: &Path,
    ) -> Result<Option<RollbackAuthority>, FileHistoryError> {
        let _io = self.cursor_io.lock().await;
        Ok(self.load_cursor_locked(path).await?.rollback_authority)
    }

    /// Consume rollback authority after a successful CAS. If persistence
    /// fails, the old guard remains safe because the live object no longer
    /// matches its recorded postimage.
    pub(crate) async fn retire_rollback_authority(
        &self,
        path: &Path,
    ) -> Result<(), FileHistoryError> {
        let _io = self.cursor_io.lock().await;
        let mut cursor = self.load_cursor_locked(path).await?;
        cursor.rollback_authority = None;
        self.persist_cursor_locked(path, &cursor).await?;
        self.cursors.lock().insert(path.to_path_buf(), cursor);
        Ok(())
    }

    fn shadow_path_for(&self, path: &Path, slot: usize) -> PathBuf {
        self.shadow_root
            .join(path_bucket(path))
            .join(format!("{slot}.bin"))
    }

    fn cursor_path_for(&self, path: &Path) -> PathBuf {
        self.shadow_root
            .join(path_bucket(path))
            .join(PATH_CURSOR_FILE)
    }

    async fn load_cursor_locked(&self, path: &Path) -> Result<PathCursor, FileHistoryError> {
        if let Some(cursor) = self.cursors.lock().get(path).cloned() {
            return Ok(cursor);
        }
        let cursor_path = self.cursor_path_for(path);
        let bytes = match self.vfs_root.read(&cursor_path).await {
            Ok(bytes) => bytes,
            Err(error) if vfs_error_is_not_found(&error) => return Ok(PathCursor::default()),
            Err(error) => return Err(error.into()),
        };
        let durable: DurablePathCursor = serde_json::from_slice(&bytes)?;
        if durable.version != PATH_CURSOR_VERSION || durable.path != path {
            return Err(FileHistoryError::InvalidState {
                path: path.to_path_buf(),
                reason: "cursor version or path binding does not match".to_string(),
            });
        }
        let rollback_authority = durable
            .rollback_authority
            .map(|authority| {
                if authority.object.authority.is_empty()
                    || authority.object.path.as_os_str().is_empty()
                    || authority.object.file.is_none()
                {
                    return Err(FileHistoryError::InvalidState {
                        path: path.to_path_buf(),
                        reason: "rollback authority lacks an exact object identity".to_string(),
                    });
                }
                Ok(RollbackAuthority {
                    postimage: FileContentIdentity {
                        sha256: authority.sha256,
                        len: authority.len,
                    },
                    object: authority.object,
                })
            })
            .transpose()?;
        let cursor = PathCursor {
            total: durable.total,
            rollback_authority,
        };
        self.cursors
            .lock()
            .insert(path.to_path_buf(), cursor.clone());
        Ok(cursor)
    }

    async fn persist_cursor_locked(
        &self,
        path: &Path,
        cursor: &PathCursor,
    ) -> Result<(), FileHistoryError> {
        let rollback_authority =
            cursor
                .rollback_authority
                .as_ref()
                .map(|authority| DurableRollbackAuthority {
                    sha256: authority.postimage.sha256,
                    len: authority.postimage.len,
                    object: authority.object.clone(),
                });
        let durable = DurablePathCursor {
            version: PATH_CURSOR_VERSION,
            path: path.to_path_buf(),
            total: cursor.total,
            rollback_authority,
        };
        let bytes = serde_json::to_vec(&durable)?;
        self.vfs_root
            .write(&self.cursor_path_for(path), &bytes)
            .await?;
        sync_parent_directory(&self.cursor_path_for(path)).await?;
        Ok(())
    }
}

fn vfs_error_is_not_found(error: &VfsError) -> bool {
    matches!(error, VfsError::NotFound { .. })
        || matches!(error, VfsError::Io(error) if error.kind() == std::io::ErrorKind::NotFound)
}

async fn sync_parent_directory(path: &Path) -> Result<(), FileHistoryError> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .ok_or_else(|| FileHistoryError::InvalidState {
                path: path.to_path_buf(),
                reason: "durable history path has no parent".to_string(),
            })?;
        let parent = parent.to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
            .await
            .map_err(|error| FileHistoryError::InvalidState {
                path: path.to_path_buf(),
                reason: format!("directory sync task failed: {error}"),
            })?
            .map_err(VfsError::Io)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
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
