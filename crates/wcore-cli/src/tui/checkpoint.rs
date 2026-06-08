//! Workspace checkpoint store — the real engine behind `/rewind` (D019).
//!
//! `/rewind` advertises "restore files to a snapshot", but historically no
//! snapshot store existed: the handler only printed git advice. This module
//! is that missing store.
//!
//! A [`Checkpoint`] captures the on-disk contents of a caller-supplied set of
//! files — in practice, the files the agent touched this session — and writes
//! them, byte-for-byte, into an on-disk store keyed by a generated id. A later
//! [`CheckpointStore::restore`] writes those bytes back over the working tree,
//! undoing every change made since the checkpoint was taken.
//!
//! ## Layout
//!
//! The store lives under a single directory (one per session, chosen by the
//! caller):
//!
//! ```text
//! <store-dir>/
//!   <checkpoint-id>/
//!     meta.json          # CheckpointMeta: id, label, created_at, file list
//!     blobs/
//!       0000             # captured bytes of file #0
//!       0001             # captured bytes of file #1
//!       ...
//! ```
//!
//! Each captured file maps to an opaque numbered blob, so paths with slashes
//! or platform-specific separators never collide and never escape the store
//! directory. The original absolute path is recorded in `meta.json` and is the
//! destination `restore` writes back to.
//!
//! ## Design constraints
//!
//! - **Dependency-light.** `std::fs` + `serde`/`serde_json` only. No git, no
//!   external process, no tokio. Capture and restore are synchronous.
//! - **Provider-neutral, path-list driven.** The store takes a plain list of
//!   files. The `/rewind` handler decides *which* files (the touched-files
//!   signal); this module decides *how* they are snapshotted and restored.
//! - **Honest about absence.** A file that does not exist at capture time is
//!   recorded as absent, and `restore` deletes it back to non-existence rather
//!   than fabricating empty content. This keeps "create a new file, then
//!   rewind" correct: the file goes away on restore.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Errors raised by the checkpoint store. Public, structured, matchable —
/// callers (the `/rewind` handler) branch on these to render honest copy.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// The requested checkpoint id is not present in the store.
    #[error("no checkpoint with id `{0}`")]
    NotFound(String),

    /// An on-disk metadata file could not be parsed as a [`CheckpointMeta`].
    #[error("checkpoint metadata at {path} is corrupt: {source}")]
    CorruptMeta {
        /// The metadata file that failed to parse.
        path: PathBuf,
        /// The underlying serde error.
        source: serde_json::Error,
    },

    /// A filesystem operation (read, write, create, remove) failed.
    #[error("checkpoint i/o failed at {path}: {source}")]
    Io {
        /// The path the failing operation targeted.
        path: PathBuf,
        /// The underlying i/o error.
        source: std::io::Error,
    },
}

/// Result alias for checkpoint operations.
pub type Result<T> = std::result::Result<T, CheckpointError>;

/// Identifier for a single checkpoint. Sortable: ids embed the creation
/// timestamp first, so lexicographic order is chronological order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckpointId(pub String);

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One captured file inside a checkpoint: where it came from, and which blob
/// holds its bytes (or that it was absent at capture time).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileEntry {
    /// Original path the bytes were captured from and will be restored to.
    path: PathBuf,
    /// Blob filename under `<checkpoint>/blobs/`, or `None` if the file did
    /// not exist at capture time (restore then deletes it).
    blob: Option<String>,
}

/// Metadata describing a checkpoint, surfaced by [`CheckpointStore::list`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMeta {
    /// Stable identifier for this checkpoint.
    pub id: CheckpointId,
    /// Human-facing label supplied at capture time (e.g. a turn summary).
    pub label: String,
    /// Unix timestamp (seconds) when the checkpoint was captured.
    pub created_at: u64,
    /// The files this checkpoint snapshotted.
    files: Vec<FileEntry>,
}

impl CheckpointMeta {
    /// Number of files captured by this checkpoint.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// The original paths this checkpoint will restore.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|f| f.path.as_path())
    }
}

/// An on-disk store of workspace checkpoints, rooted at a single directory.
///
/// Construct one per session with [`CheckpointStore::new`]; the directory is
/// created lazily on the first [`capture`](CheckpointStore::capture).
#[derive(Debug, Clone)]
pub struct CheckpointStore {
    root: PathBuf,
    /// The workspace boundary. Capture and restore refuse any file path that
    /// resolves OUTSIDE this directory (see [`path_within_root`]). The touched
    /// paths recorded into `meta.json` are the raw `file_path` strings from a
    /// Read/Write/Edit tool call, which a misbehaving or prompt-injected model
    /// can set to ANY absolute path; restore writes/deletes those paths, so an
    /// unvalidated store is an arbitrary-file-write/delete primitive. Confining
    /// to the workspace root mirrors the containment the Write tool enforces.
    workspace_root: PathBuf,
    /// Process-lifetime monotonic counter, bumped once per `capture`, folded
    /// into the id as a trailing segment. `next_seq` alone (a dir-count of the
    /// same wall-clock second) is racy: B1b moved `capture` off the render lock
    /// onto `spawn_blocking`, so two concurrent same-second captures both read
    /// the same dir count, mint the SAME id, and clobber each other's
    /// `meta.json`/blobs. This counter is `Arc`-shared so it stays single across
    /// the cheap `clone()` the bridge takes per capture — every capture, even
    /// concurrent ones in the same second, draws a distinct value, so the ids
    /// never collide WITHOUT a global lock around the filesystem work.
    seq_counter: Arc<AtomicU64>,
}

impl CheckpointStore {
    /// Create a store rooted at `dir`, confining capture/restore to files under
    /// `workspace_root`. The store directory is not touched until the first
    /// `capture`; an empty or missing directory is a valid empty store.
    pub fn new(dir: impl Into<PathBuf>, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            root: dir.into(),
            workspace_root: workspace_root.into(),
            seq_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Capture the current contents of `files` as a new checkpoint and return
    /// its id.
    ///
    /// Each file's bytes are copied into the store. A file that does not exist
    /// is recorded as absent (restore will delete it). Duplicate paths in
    /// `files` are de-duplicated, preserving first-seen order.
    ///
    /// `label` is free-form caller text (a turn summary, a command name) shown
    /// in [`list`](CheckpointStore::list).
    pub fn capture<I, P>(&self, label: impl Into<String>, files: I) -> Result<CheckpointId>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let created_at = now_secs();
        // Id = <secs>-<within-second seq>-<process-monotonic counter>. The
        // timestamp prefix keeps lexicographic order chronological; `next_seq`
        // gives a readable within-second ordinal; the trailing atomic counter is
        // the collision guard — two concurrent same-second captures (B1b runs
        // `capture` on `spawn_blocking`) share the same dir count, so `next_seq`
        // alone would mint identical ids and clobber each other. The atomic is
        // bumped exactly once per capture and is distinct even across the cheap
        // `clone()` the bridge takes, so the full id is always unique without a
        // lock around the filesystem work.
        let unique = self.seq_counter.fetch_add(1, Ordering::Relaxed);
        let id = CheckpointId(format!(
            "{:020}-{:04}-{:010}",
            created_at,
            self.next_seq(created_at),
            unique,
        ));

        let cp_dir = self.root.join(&id.0);
        let blobs_dir = cp_dir.join("blobs");
        create_dir_all(&blobs_dir)?;

        let mut entries: Vec<FileEntry> = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();

        for file in files {
            let path = file.as_ref().to_path_buf();
            if seen.contains(&path) {
                continue;
            }
            seen.push(path.clone());

            // SECURITY: never snapshot a path that resolves outside the
            // workspace root. The path comes (via touched-files) from a tool
            // call's raw `file_path` arg, which an untrusted model controls; a
            // captured out-of-tree path would later be written/deleted by
            // restore, past the Write tool's deny-list and the approval gate.
            // Drop it from the checkpoint entirely.
            if !path_within_root(&path, &self.workspace_root) {
                continue;
            }

            let blob = match fs::read(&path) {
                Ok(bytes) => {
                    let blob_name = format!("{:04}", entries.len());
                    let blob_path = blobs_dir.join(&blob_name);
                    write_file(&blob_path, &bytes)?;
                    Some(blob_name)
                }
                // Absent at capture time: record it so restore can delete it
                // back to non-existence ("created this file, then rewound").
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    return Err(CheckpointError::Io { path, source: e });
                }
            };

            entries.push(FileEntry { path, blob });
        }

        let meta = CheckpointMeta {
            id: id.clone(),
            label: label.into(),
            created_at,
            files: entries,
        };

        let meta_path = cp_dir.join("meta.json");
        let json =
            serde_json::to_vec_pretty(&meta).map_err(|source| CheckpointError::CorruptMeta {
                path: meta_path.clone(),
                source,
            })?;
        write_file(&meta_path, &json)?;

        Ok(id)
    }

    /// List all checkpoints in the store, newest first.
    ///
    /// A missing store directory yields an empty list (not an error): a
    /// session that never captured anything has nothing to rewind to. A
    /// checkpoint sub-directory with unreadable or corrupt metadata is an
    /// error, because silently dropping it would let `/rewind` lie about
    /// which restore points exist.
    pub fn list(&self) -> Result<Vec<CheckpointMeta>> {
        let read = match fs::read_dir(&self.root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(CheckpointError::Io {
                    path: self.root.clone(),
                    source,
                });
            }
        };

        let mut metas = Vec::new();
        for entry in read {
            let entry = entry.map_err(|source| CheckpointError::Io {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join("meta.json");
            // A directory without meta.json is not a checkpoint (e.g. a
            // half-written capture or unrelated content); skip it.
            if !meta_path.exists() {
                continue;
            }
            metas.push(self.load_meta(&meta_path)?);
        }

        // Ids are timestamp-prefixed, so descending id order is newest-first.
        metas.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(metas)
    }

    /// Restore the working tree to the named checkpoint.
    ///
    /// Every captured file is overwritten with its checkpointed bytes; every
    /// file that was *absent* at capture time is deleted. Returns
    /// [`CheckpointError::NotFound`] if `id` is not in the store.
    pub fn restore(&self, id: &CheckpointId) -> Result<()> {
        let cp_dir = self.root.join(&id.0);
        let meta_path = cp_dir.join("meta.json");
        if !meta_path.exists() {
            return Err(CheckpointError::NotFound(id.0.clone()));
        }
        let meta = self.load_meta(&meta_path)?;
        let blobs_dir = cp_dir.join("blobs");

        // Two-pass restore. PASS 1 plans every action without mutating the
        // working tree: it (a) SECURITY-validates each path is inside the
        // workspace root — defence in depth against a poisoned/older meta.json
        // whose paths were not validated at capture (capture now drops them,
        // but restore must not trust stored metadata) — and (b) reads every
        // blob into memory. PASS 2 applies the planned writes/deletes.
        //
        // Atomicity is SCOPED, not total. The guarantee PASS 1 buys is: if any
        // blob read fails, the restore aborts BEFORE a single working-tree file
        // is touched — a missing/corrupt blob can no longer leave the workspace
        // half-rewound. It does NOT make PASS 2 transactional: PASS 2 applies
        // writes/deletes sequentially, so a filesystem error on the k-th action
        // (e.g. a permission/ENOSPC failure mid-loop) still leaves entries
        // 1..k-1 rewound and k..n untouched — a torn state. Closing that would
        // need a staging-dir + atomic-rename or rollback journal, deliberately
        // out of scope for this dependency-light store.
        enum Action {
            Write(PathBuf, Vec<u8>),
            Delete(PathBuf),
        }
        let mut plan: Vec<Action> = Vec::with_capacity(meta.files.len());
        for entry in &meta.files {
            // Refuse any path that resolves outside the workspace root.
            if !path_within_root(&entry.path, &self.workspace_root) {
                continue;
            }
            match &entry.blob {
                Some(blob_name) => {
                    let blob_path = blobs_dir.join(blob_name);
                    let bytes = fs::read(&blob_path).map_err(|source| CheckpointError::Io {
                        path: blob_path.clone(),
                        source,
                    })?;
                    plan.push(Action::Write(entry.path.clone(), bytes));
                }
                None => plan.push(Action::Delete(entry.path.clone())),
            }
        }

        for action in plan {
            match action {
                Action::Write(path, bytes) => {
                    if let Some(parent) = path.parent() {
                        create_dir_all(parent)?;
                    }
                    write_file(&path, &bytes)?;
                }
                Action::Delete(path) => match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(CheckpointError::Io { path, source });
                    }
                },
            }
        }

        Ok(())
    }

    /// Load and parse a `meta.json`, mapping i/o and parse failures to typed
    /// errors.
    fn load_meta(&self, meta_path: &Path) -> Result<CheckpointMeta> {
        let bytes = fs::read(meta_path).map_err(|source| CheckpointError::Io {
            path: meta_path.to_path_buf(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| CheckpointError::CorruptMeta {
            path: meta_path.to_path_buf(),
            source,
        })
    }

    /// Count checkpoints already created in the same wall-clock second so two
    /// captures in one second get distinct, monotonically increasing ids.
    fn next_seq(&self, created_at: u64) -> u32 {
        let prefix = format!("{:020}-", created_at);
        let read = match fs::read_dir(&self.root) {
            Ok(rd) => rd,
            Err(_) => return 0,
        };
        read.flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| name.starts_with(&prefix))
            .count() as u32
    }
}

/// Current Unix time in whole seconds. A clock set before the epoch yields 0
/// rather than panicking — id uniqueness is preserved by the sequence suffix.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// True iff `path` resolves to a location INSIDE `root` — the workspace
/// boundary `capture`/`restore` enforce.
///
/// The check is symlink-safe: it rejects a non-absolute path or any `..`
/// component outright, then canonicalizes `root` and the longest EXISTING
/// ancestor of `path` and requires the latter to live under the former. Using
/// the existing ancestor (rather than `path` itself, which may be a
/// to-be-created file) still defeats a symlinked directory prefix that points
/// outside the root, because the symlink resolves during canonicalization. A
/// path whose root cannot be canonicalized (missing, permission) fails closed.
fn path_within_root(path: &Path, root: &Path) -> bool {
    use std::path::Component;
    if !path.is_absolute() {
        return false;
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let root_canon = match root.canonicalize() {
        Ok(r) => r,
        Err(_) => return false,
    };
    // Walk up to the longest ancestor that exists on disk, then canonicalize
    // it (resolving any symlinks in the prefix) and confirm containment.
    let mut ancestor = path;
    loop {
        if ancestor.exists() {
            return match ancestor.canonicalize() {
                Ok(c) => c.starts_with(&root_canon),
                Err(_) => false,
            };
        }
        match ancestor.parent() {
            Some(parent) => ancestor = parent,
            None => return false,
        }
    }
}

/// `fs::create_dir_all` with the failing path attached to the error.
fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| CheckpointError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// `fs::write` with the failing path attached to the error.
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|source| CheckpointError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Capture a file's contents, mutate the file on disk, restore the
    /// checkpoint, and assert the original bytes are back. This is the core
    /// `/rewind` round-trip.
    #[test]
    fn capture_then_restore_round_trips_original_content() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let file = work.join("main.rs");
        fs::write(&file, b"fn main() {}\n").unwrap();

        let store = CheckpointStore::new(tmp.path().join("store"), tmp.path());
        let id = store.capture("before edit", [&file]).unwrap();

        // Mutate the working tree after the checkpoint.
        fs::write(&file, b"fn main() { panic!() }\n").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"fn main() { panic!() }\n");

        store.restore(&id).unwrap();

        assert_eq!(
            fs::read(&file).unwrap(),
            b"fn main() {}\n",
            "restore must write the captured bytes back over the mutation",
        );
    }

    /// A file that did not exist at capture time must be deleted on restore —
    /// "the agent created this file, then I rewound" undoes the creation.
    #[test]
    fn restore_deletes_files_absent_at_capture() {
        let tmp = tempfile::tempdir().unwrap();
        let new_file = tmp.path().join("created_later.txt");

        let store = CheckpointStore::new(tmp.path().join("store"), tmp.path());
        // Capture references a path that does not exist yet.
        let id = store.capture("clean slate", [&new_file]).unwrap();

        // The agent then "creates" the file.
        fs::write(&new_file, b"generated").unwrap();
        assert!(new_file.exists());

        store.restore(&id).unwrap();

        assert!(
            !new_file.exists(),
            "restore must delete a file that was absent at capture time",
        );
    }

    /// Restore recreates a captured file even if it was deleted from the
    /// working tree after the checkpoint, including any missing parent dirs.
    #[test]
    fn restore_recreates_a_deleted_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("nested").join("deep").join("data.txt");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"keep me").unwrap();

        let store = CheckpointStore::new(tmp.path().join("store"), tmp.path());
        let id = store.capture("snapshot", [&file]).unwrap();

        // Delete the file (and its directory tree) after the checkpoint.
        fs::remove_dir_all(tmp.path().join("nested")).unwrap();
        assert!(!file.exists());

        store.restore(&id).unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"keep me");
    }

    /// `list` returns checkpoints newest-first and reports the right metadata.
    #[test]
    fn list_orders_newest_first_with_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();

        let store = CheckpointStore::new(tmp.path().join("store"), tmp.path());
        let first = store.capture("first", [&a]).unwrap();
        let second = store.capture("second", [&a, &b]).unwrap();

        let metas = store.list().unwrap();
        assert_eq!(metas.len(), 2, "both checkpoints must be listed");

        // Newest first: the second capture leads.
        assert_eq!(metas[0].id, second);
        assert_eq!(metas[0].label, "second");
        assert_eq!(metas[0].file_count(), 2);

        assert_eq!(metas[1].id, first);
        assert_eq!(metas[1].label, "first");
        assert_eq!(metas[1].file_count(), 1);
    }

    /// Two captures within the same wall-clock second get distinct ids via
    /// the sequence suffix, so neither overwrites the other.
    #[test]
    fn captures_in_same_second_get_distinct_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("f.txt");
        fs::write(&f, b"x").unwrap();

        let store = CheckpointStore::new(tmp.path().join("store"), tmp.path());
        let one = store.capture("one", [&f]).unwrap();
        let two = store.capture("two", [&f]).unwrap();

        assert_ne!(one, two, "ids captured back-to-back must be unique");
        assert_eq!(store.list().unwrap().len(), 2);
    }

    /// F1 regression: two CONCURRENT captures racing in the same wall-clock
    /// second (the B1b `spawn_blocking` shape) must still mint distinct ids and
    /// not clobber each other. Before the atomic counter, both reads of the
    /// same-second dir count returned the same `next_seq`, so the two captures
    /// minted the SAME id and the second overwrote the first's `meta.json`/blobs
    /// — a corrupted restore point. Each thread captures a file with DISTINCT
    /// content; afterwards both checkpoints must be listed and each must restore
    /// to its own content, proving neither blob set was clobbered.
    #[test]
    fn concurrent_same_second_captures_do_not_clobber() {
        use std::sync::{Arc, Barrier};

        let tmp = tempfile::tempdir().unwrap();
        let store_root = tmp.path().join("store");
        // Two distinct in-root files with distinct content.
        let fa = tmp.path().join("a.txt");
        let fb = tmp.path().join("b.txt");
        fs::write(&fa, b"AAA").unwrap();
        fs::write(&fb, b"BBB").unwrap();

        // Mirror production: clone the store handle so both captures share the
        // same `Arc<AtomicU64>` counter — exactly what `spawn_blocking` does.
        let store = CheckpointStore::new(&store_root, tmp.path());
        let store_a = store.clone();
        let store_b = store.clone();

        // A barrier maximizes the chance both `capture` calls land in the same
        // second AND interleave their `next_seq` reads — the original race.
        let barrier = Arc::new(Barrier::new(2));
        let b_a = barrier.clone();
        let b_b = barrier.clone();
        let fa_c = fa.clone();
        let fb_c = fb.clone();

        let h_a = std::thread::spawn(move || {
            b_a.wait();
            store_a.capture("thread a", [&fa_c]).unwrap()
        });
        let h_b = std::thread::spawn(move || {
            b_b.wait();
            store_b.capture("thread b", [&fb_c]).unwrap()
        });
        let id_a = h_a.join().unwrap();
        let id_b = h_b.join().unwrap();

        // Distinct ids → distinct on-disk checkpoint dirs.
        assert_ne!(id_a, id_b, "concurrent same-second captures must be unique");
        assert_eq!(
            store.list().unwrap().len(),
            2,
            "both checkpoints must survive — neither clobbered the other",
        );

        // Each checkpoint round-trips to ITS OWN content (proves blobs intact).
        fs::write(&fa, b"mutated-a").unwrap();
        fs::write(&fb, b"mutated-b").unwrap();
        store.restore(&id_a).unwrap();
        store.restore(&id_b).unwrap();
        assert_eq!(fs::read(&fa).unwrap(), b"AAA", "checkpoint a's blob intact");
        assert_eq!(fs::read(&fb).unwrap(), b"BBB", "checkpoint b's blob intact");
    }

    /// Duplicate paths in one capture are de-duplicated.
    #[test]
    fn capture_dedupes_repeated_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("dup.txt");
        fs::write(&f, b"v").unwrap();

        let store = CheckpointStore::new(tmp.path().join("store"), tmp.path());
        let id = store.capture("dup", [&f, &f, &f]).unwrap();

        let meta = store
            .list()
            .unwrap()
            .into_iter()
            .find(|m| m.id == id)
            .unwrap();
        assert_eq!(meta.file_count(), 1, "repeated paths collapse to one entry");
    }

    /// `list` on a never-captured store is an empty list, not an error.
    #[test]
    fn list_on_empty_store_is_ok_and_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(tmp.path().join("never-made"), tmp.path());
        assert!(store.list().unwrap().is_empty());
    }

    /// Restoring an unknown id is a typed `NotFound`, not a panic.
    #[test]
    fn restore_unknown_id_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(tmp.path().join("store"), tmp.path());
        let err = store
            .restore(&CheckpointId("does-not-exist".into()))
            .unwrap_err();
        assert!(matches!(err, CheckpointError::NotFound(_)));
    }

    /// SECURITY: the path validator confines to the workspace root and rejects
    /// traversal, absolute escapes, relative paths, and symlinked prefixes.
    #[test]
    fn path_within_root_confines_to_workspace() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("sub")).unwrap();
        fs::write(root.path().join("sub/in.txt"), b"x").unwrap();
        fs::write(outside.path().join("secret"), b"x").unwrap();

        // In-root (existing and to-be-created) paths are allowed.
        assert!(path_within_root(
            &root.path().join("sub/in.txt"),
            root.path()
        ));
        assert!(path_within_root(
            &root.path().join("sub/new.txt"),
            root.path()
        ));
        // Out-of-root absolute path is refused.
        assert!(!path_within_root(
            &outside.path().join("secret"),
            root.path()
        ));
        // A `..` escape is refused even if it would resolve back inside.
        assert!(!path_within_root(
            &root.path().join("../escape"),
            root.path()
        ));
        // A relative path is refused (must be absolute).
        assert!(!path_within_root(Path::new("relative/x"), root.path()));

        // A symlink inside the root that points OUTSIDE it is refused (the
        // prefix resolves out of the workspace during canonicalization).
        #[cfg(unix)]
        {
            let link = root.path().join("link");
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            assert!(!path_within_root(&link.join("secret"), root.path()));
        }
    }

    /// SECURITY: capture DROPS any path outside the workspace root, so a later
    /// restore can never write or delete it — the arbitrary-file-write/delete
    /// primitive the red-team found is closed at the source.
    #[test]
    fn capture_drops_paths_outside_workspace_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let inside = root.path().join("keep.txt");
        fs::write(&inside, b"keep").unwrap();
        // An out-of-root file that exists, and an out-of-root absent path (the
        // delete primitive). Neither must be recorded.
        let outside_existing = outside.path().join("victim");
        fs::write(&outside_existing, b"do not touch").unwrap();
        let outside_absent = outside.path().join("would-be-deleted");

        let store = CheckpointStore::new(root.path().join("store"), root.path());
        let id = store
            .capture("snap", [&inside, &outside_existing, &outside_absent])
            .unwrap();

        // Only the in-root file was captured.
        let meta = &store.list().unwrap()[0];
        assert_eq!(meta.file_count(), 1, "out-of-root paths must be dropped");
        assert!(meta.paths().all(|p| p.starts_with(root.path())));

        // Restore touches nothing outside the root: the victim survives.
        store.restore(&id).unwrap();
        assert_eq!(fs::read(&outside_existing).unwrap(), b"do not touch");
        assert!(!outside_absent.exists());
    }
}
