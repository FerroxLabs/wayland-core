//! Crash-safe, append-only session execution journal.
//!
//! Records form a versioned SHA-256 chain independent of session snapshots.
//! Started external effects recover as [`ExternalEffectState::Unknown`], never
//! as safe to repeat.
//!
//! Raw incremental reduction is intentionally not a public API. Callers must
//! replay complete history through [`replay_state`], which enforces forward-only
//! schema evolution.
//!
//! ```compile_fail
//! use wcore_agent::session_journal::reduce;
//! ```

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

mod lease;
pub use lease::LeaseOwner;
use lease::WriterLease;
mod model;
pub use model::*;
mod reducer;
pub use reducer::{
    PREPARED_PROVIDER_REQUEST_SNAPSHOT_VERSION, decode_prepared_provider_request_snapshot,
    prepared_provider_request_snapshot, provider_request_digest, replay_state,
    state_payload_digest,
};
pub(crate) use reducer::{
    child_transaction_opening_token_digest, reduce, require_turn_descendants_terminal,
    validate_durable_child_lineage,
};
mod snapshot;
pub use snapshot::{
    LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION, SESSION_SNAPSHOT_SCHEMA_VERSION, SessionSnapshot,
    load_snapshot, snapshot_path_for,
};
use snapshot::{SnapshotAuthorityBinding, SnapshotAuthorityHead};

pub const SESSION_JOURNAL_SCHEMA_VERSION: u32 = 5;
pub const LEGACY_SESSION_JOURNAL_SCHEMA_VERSION: u32 = 4;
pub const GENESIS_CHECKSUM: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
const FRAME_MAGIC: &[u8; 4] = b"WJ01";
const SNAPSHOT_AUTHORITY_MAGIC: &[u8; 4] = b"WSA1";
const FRAME_HEADER_BYTES: usize = 12;
const FRAME_DIGEST_BYTES: usize = 32;
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const MAX_SNAPSHOT_AUTHORITY_BYTES: usize = 16 * 1024;
const MAX_EFFECT_CHECKPOINT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EFFECT_CHECKPOINT_SESSION_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[rustfmt::skip]
pub struct JournalEnvelope {
    pub schema_version: u32, pub session_id: String, pub seq: u64,
    pub previous_checksum: String, pub event: SessionEvent, pub checksum: String,
}

#[derive(Serialize)]
#[rustfmt::skip]
struct ChecksumMaterial<'a> {
    schema_version: u32, session_id: &'a str, seq: u64,
    previous_checksum: &'a str, event: &'a SessionEvent,
}

#[derive(Deserialize)]
struct EnvelopeSchema {
    schema_version: u32,
}

impl JournalEnvelope {
    fn create(
        session_id: String,
        seq: u64,
        previous_checksum: String,
        event: SessionEvent,
    ) -> Result<Self, JournalError> {
        let mut envelope = Self {
            schema_version: SESSION_JOURNAL_SCHEMA_VERSION,
            session_id,
            seq,
            previous_checksum,
            event,
            checksum: String::new(),
        };
        envelope.checksum = envelope.computed_checksum()?;
        Ok(envelope)
    }

    fn computed_checksum(&self) -> Result<String, JournalError> {
        let material = ChecksumMaterial {
            schema_version: self.schema_version,
            session_id: &self.session_id,
            seq: self.seq,
            previous_checksum: &self.previous_checksum,
            event: &self.event,
        };
        let bytes = serde_json::to_vec(&material).map_err(|source| JournalError::Json {
            context: "encoding checksum material",
            source,
        })?;
        Ok(sha256_hex(&bytes))
    }
}

#[derive(Debug, Error)]
#[rustfmt::skip]
pub enum JournalError {
    #[error("session journal I/O failed at {path}: {source}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
    #[error("session journal JSON failed while {context}: {source}")]
    Json { context: &'static str, #[source] source: serde_json::Error },
    #[error("session journal {path} has a corrupt complete frame {frame}: {source}")]
    CorruptFrame { path: PathBuf, frame: usize, #[source] source: serde_json::Error },
    #[error("session journal {path} has an invalid header at frame {frame}")]
    InvalidFrameHeader { path: PathBuf, frame: usize },
    #[error("session journal {path} frame {frame} exceeds the maximum size")]
    FrameTooLarge { path: PathBuf, frame: usize },
    #[error("session journal {path} frame {frame} digest mismatch")]
    FrameDigestMismatch { path: PathBuf, frame: usize },
    #[error("unsupported session journal schema {found}; supported schema is {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("journal event {event_type:?} requires journal schema {required}, found {found}")]
    EventRequiresSchema { event_type: String, found: u32, required: u32 },
    #[error("session journal schema regressed from {previous} to {found}")]
    SchemaRegression { previous: u32, found: u32 },
    #[error("unsupported session snapshot schema {found}; supported schema is {supported}")]
    UnsupportedSnapshotSchema { found: u32, supported: u32 },
    #[error("unsupported snapshot authority binding schema {found}; supported schema is {supported}")]
    UnsupportedSnapshotBindingSchema { found: u32, supported: u32 },
    #[error("current session snapshot is not bound to retained journal authority")]
    SnapshotAuthorityMismatch,
    #[error("unknown critical field {field:?} in {layer}")]
    UnknownCriticalField { layer: &'static str, field: String },
    #[error("journal session mismatch: expected {expected}, found {found}")]
    SessionMismatch { expected: String, found: String },
    #[error("journal sequence mismatch: expected {expected}, found {found}")]
    SequenceMismatch { expected: u64, found: u64 },
    #[error("journal previous checksum mismatch at sequence {seq}")]
    PreviousChecksumMismatch { seq: u64 },
    #[error("journal checksum mismatch at sequence {seq}")]
    ChecksumMismatch { seq: u64 },
    #[error("invalid journal state transition: {0}")]
    InvalidTransition(String),
    #[error("snapshot state digest mismatch")]
    SnapshotDigestMismatch,
    #[error("session snapshot {path} is {size} bytes, exceeding the maximum {max}")]
    SnapshotTooLarge { path: PathBuf, size: u64, max: u64 },
    #[error("session snapshot permissions are not private: {path}")]
    SnapshotUnsafePermissions { path: PathBuf },
    #[error("session snapshot owner does not match the effective user: {path}")]
    SnapshotOwnerMismatch { path: PathBuf },
    #[error("snapshot cursor does not match its reduced state")]
    SnapshotCursorMismatch,
    #[error("snapshot and journal do not describe the same authority: {0}")]
    SnapshotJournalMismatch(String),
    #[error("locked journal state and committed head do not describe the same authority: {0}")]
    JournalAuthorityMismatch(String),
    #[error("compacted journal begins at sequence {first_seq} but its snapshot is missing")]
    CompactedJournalMissingSnapshot { first_seq: u64 },
    #[error("session journal writer lock is poisoned")]
    WriterPoisoned,
    #[error("session journal writer is faulted after a previous I/O failure")]
    WriterFaulted,
    #[error("session journal writer lease is already held at {lease_path}")]
    AlreadyOwned { lease_path: PathBuf },
    #[error("session journal path must not be a symbolic link: {path}")]
    SymbolicLink { path: PathBuf },
    #[error("session journal canonical path no longer names the held file: {path}")]
    PathIdentityMismatch { path: PathBuf },
    #[error("session journal {path} must have exactly one filesystem link")]
    MultipleLinks { path: PathBuf },
}

#[derive(Debug)]
struct JournalWriter {
    path: PathBuf,
    session_id: String,
    file: File,
    next_seq: u64,
    previous_checksum: String,
    state: ReducedSessionState,
    last_envelope: Option<JournalEnvelope>,
    base_snapshot: Option<SessionSnapshot>,
    faulted: bool,
    _lease: WriterLease,
}

type SharedWriter = Arc<Mutex<JournalWriter>>;

#[derive(Debug, Clone)]
pub struct SessionJournal {
    inner: SharedWriter,
}

pub(crate) struct CommittedJournalAuthority {
    pub(crate) state: ReducedSessionState,
    pub(crate) entries: Vec<JournalEnvelope>,
    pub(crate) base_snapshot: Option<SessionSnapshot>,
}

/// Exact committed state observed while the live writer lease is held.
///
/// This value is crate-private: public callers cannot turn snapshot-shaped
/// bytes into journal authority. Transaction openings copy its fields into a
/// durable event before this locked operation returns.
pub(crate) struct JournalSnapshotAuthority {
    pub(crate) session_id: String,
    pub(crate) binding_schema_version: u32,
    pub(crate) snapshot_schema_version: u32,
    pub(crate) cursor: Option<u64>,
    pub(crate) cursor_checksum: String,
    pub(crate) state_digest: String,
    pub(crate) binding_digest: String,
    pub(crate) durable_authority_generation: String,
}

struct ParsedJournal {
    entries: Vec<JournalEnvelope>,
    bindings: Vec<SnapshotAuthorityBinding>,
    valid_len: usize,
}

#[cfg(test)]
thread_local! {
    static AFTER_JOURNAL_READ_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> =
        std::cell::RefCell::new(None);
    static AFTER_SNAPSHOT_AUTHORITY_WRITE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn set_after_journal_read_hook(hook: impl FnOnce(&Path) + 'static) {
    AFTER_JOURNAL_READ_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_journal_read_hook(path: &Path) {
    AFTER_JOURNAL_READ_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(path);
        }
    });
}

#[cfg(test)]
fn set_after_snapshot_authority_write_hook(hook: impl FnOnce(&Path) + 'static) {
    AFTER_SNAPSHOT_AUTHORITY_WRITE_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_snapshot_authority_write_hook(path: &Path) {
    AFTER_SNAPSHOT_AUTHORITY_WRITE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(path);
        }
    });
}

#[cfg(not(test))]
fn run_after_snapshot_authority_write_hook(_path: &Path) {}

#[cfg(not(test))]
fn run_after_journal_read_hook(_path: &Path) {}

fn read_journal_if_present(path: &Path) -> Result<Vec<u8>, JournalError> {
    let mut file = match lease::open_existing_nofollow(path) {
        Ok(file) => file,
        Err(JournalError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    run_after_journal_read_hook(path);
    lease::ensure_path_identity(&file, path)?;
    Ok(bytes)
}

/// Snapshot data promoted to durable recovery authority by journal evidence.
///
/// The wrapper is deliberately private: serialized [`SessionSnapshot`] values
/// remain caller-controlled data until a retained WSA1 binding or a replayed
/// full prefix proves their exact state.
struct BoundSessionSnapshot<'a> {
    snapshot: &'a SessionSnapshot,
}

impl<'a> BoundSessionSnapshot<'a> {
    fn from_retained_binding(
        snapshot: &'a SessionSnapshot,
        bindings: &[SnapshotAuthorityBinding],
    ) -> Result<Self, JournalError> {
        if bindings.iter().any(|binding| binding.matches(snapshot)) {
            Ok(Self { snapshot })
        } else {
            Err(JournalError::SnapshotAuthorityMismatch)
        }
    }

    fn from_replayed_prefix(
        snapshot: &'a SessionSnapshot,
        prefix_state: &ReducedSessionState,
    ) -> Result<Self, JournalError> {
        if prefix_state == &snapshot.state {
            Ok(Self { snapshot })
        } else {
            Err(JournalError::SnapshotJournalMismatch(
                "snapshot state does not equal its full-log prefix".to_owned(),
            ))
        }
    }
}

/// Exclusive authority used while retiring every durable file for a session.
///
/// The writer-lock sentinel is deliberately retained after this guard drops:
/// unlinking a lock inode permits two processes to lock different inodes under
/// the same pathname. It contains ownership metadata only, never session data.
pub(crate) struct SessionStorageLease {
    journal_path: PathBuf,
    session_id: String,
    _journal_file: Option<File>,
    _lease: WriterLease,
}

impl SessionJournal {
    /// Open or create a journal with an exclusive cross-process writer lease.
    /// Clone this handle to share authority; an independent open fails closed.
    pub fn open(
        path: impl AsRef<Path>,
        session_id: impl Into<String>,
    ) -> Result<Self, JournalError> {
        let path = lease::normalized_path(path.as_ref())?;
        let session_id = session_id.into();
        Ok(Self {
            inner: Arc::new(Mutex::new(JournalWriter::open(path, session_id)?)),
        })
    }

    pub fn append(&self, event: SessionEvent) -> Result<JournalEnvelope, JournalError> {
        self.inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?
            .append(event)
    }

    /// Evaluate an idempotency decision and append under one writer lock.
    ///
    /// This is crate-private because only journal-backed stores may define a
    /// content-bound exact-replay decision. Public append semantics stay
    /// unconditional.
    pub(crate) fn append_conditionally<F>(
        &self,
        event: SessionEvent,
        should_append: F,
    ) -> Result<Option<JournalEnvelope>, JournalError>
    where
        F: FnOnce(&ReducedSessionState, &str) -> Result<bool, JournalError>,
    {
        let mut writer = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?;
        if !should_append(&writer.state, &writer.session_id)? {
            return Ok(None);
        }
        writer.append(event).map(Some)
    }

    /// Build and append an event from the exact committed state under one
    /// uninterrupted writer-authority operation.
    ///
    /// The closure may return `None` for an exact idempotent replay. Even that
    /// path validates the retained snapshot authority before returning.
    pub(crate) fn append_from_committed_authority<F>(
        &self,
        build_event: F,
    ) -> Result<Option<JournalEnvelope>, JournalError>
    where
        F: FnOnce(
            &ReducedSessionState,
            &JournalSnapshotAuthority,
        ) -> Result<Option<SessionEvent>, JournalError>,
    {
        let mut writer = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?;
        let committed = writer.committed_authority()?;
        let snapshot = SessionSnapshot::new(writer.session_id.clone(), committed.state.clone())?;
        let binding = SnapshotAuthorityBinding::new(&snapshot);
        let binding_value =
            serde_json::to_value(&binding).map_err(|source| JournalError::Json {
                context: "encoding locked snapshot authority binding",
                source,
            })?;
        let binding_digest = state_payload_digest(&binding_value)?;
        let generation_value = serde_json::json!({
            "domain": "wayland-core:journal-authority-generation:v1",
            "session_id": writer.session_id,
            "journal_schema_version": SESSION_JOURNAL_SCHEMA_VERSION,
            "snapshot_schema_version": snapshot.schema_version,
            "cursor": snapshot.cursor,
            "cursor_checksum": snapshot.cursor_checksum,
            "state_digest": snapshot.state_digest,
            "binding_digest": binding_digest,
            "base_snapshot_digest": committed
                .base_snapshot
                .as_ref()
                .map(|base| base.state_digest.as_str()),
        });
        let authority = JournalSnapshotAuthority {
            session_id: writer.session_id.clone(),
            binding_schema_version: binding.schema_version,
            snapshot_schema_version: snapshot.schema_version,
            cursor: snapshot.cursor,
            cursor_checksum: snapshot.cursor_checksum,
            state_digest: snapshot.state_digest,
            binding_digest,
            durable_authority_generation: state_payload_digest(&generation_value)?,
        };
        let Some(event) = build_event(&committed.state, &authority)? else {
            return Ok(None);
        };
        writer.append(event).map(Some)
    }

    pub fn state(&self) -> Result<ReducedSessionState, JournalError> {
        self.inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)
            .map(|writer| writer.state.clone())
    }

    /// Snapshot the reduced state and committed entries from one locked writer.
    ///
    /// Reading the already-open data file prevents a pathname replacement from
    /// supplying head evidence for a different authority. The writer validates
    /// that the parsed head still matches its reduced state before returning
    /// either value.
    pub(crate) fn committed_authority(&self) -> Result<CommittedJournalAuthority, JournalError> {
        self.inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?
            .committed_authority()
    }

    /// Compatibility projection for callers that need only committed entries.
    /// The entries still come from the locked writer authority, never by
    /// reopening its mutable pathname.
    #[cfg(test)]
    pub(crate) fn committed_entries(&self) -> Result<Vec<JournalEnvelope>, JournalError> {
        self.committed_authority().map(|authority| {
            debug_assert_eq!(
                authority.state.last_seq,
                authority.entries.last().map(|entry| entry.seq)
            );
            authority.entries
        })
    }

    /// Stable session identity used when deriving durable effect keys.
    pub fn session_id(&self) -> Result<String, JournalError> {
        self.inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)
            .map(|writer| writer.session_id.clone())
    }

    /// Persist a private content-addressed preimage used by filesystem-effect
    /// recovery. The journal stores only this digest; raw file contents never
    /// enter an event frame.
    pub(crate) fn store_effect_checkpoint(
        &self,
        digest: &str,
        contents: &[u8],
    ) -> Result<(), JournalError> {
        if contents.len() as u64 > MAX_EFFECT_CHECKPOINT_BYTES {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint exceeds {MAX_EFFECT_CHECKPOINT_BYTES} bytes"
            )));
        }
        if !valid_sha256_hex(digest) || sha256_hex(contents) != digest {
            return Err(JournalError::InvalidTransition(
                "filesystem effect checkpoint digest mismatch".to_string(),
            ));
        }
        let path = self.effect_checkpoint_path(digest)?;
        let directory = path.parent().expect("checkpoint path has a parent");
        std::fs::create_dir_all(directory).map_err(|source| JournalError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let directory_metadata =
            std::fs::symlink_metadata(directory).map_err(|source| JournalError::Io {
                path: directory.to_path_buf(),
                source,
            })?;
        if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint directory is not a private directory: {}",
                directory.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            use std::os::unix::fs::PermissionsExt as _;
            if directory_metadata.uid() != self.journal_owner_uid()? {
                return Err(JournalError::InvalidTransition(format!(
                    "filesystem effect checkpoint directory has the wrong owner: {}",
                    directory.display()
                )));
            }
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).map_err(
                |source| JournalError::Io {
                    path: directory.to_path_buf(),
                    source,
                },
            )?;
        }

        remove_stale_checkpoint_temps(directory, digest, path.exists())?;

        if path.exists() {
            self.load_effect_checkpoint(digest)?;
            return Ok(());
        }
        let session_bytes = checkpoint_directory_bytes(directory)?;
        if session_bytes.saturating_add(contents.len() as u64) > MAX_EFFECT_CHECKPOINT_SESSION_BYTES
        {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoints exceed the {MAX_EFFECT_CHECKPOINT_SESSION_BYTES}-byte session quota"
            )));
        }

        let temporary = directory.join(format!(
            ".{digest}.{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|source| JournalError::Io {
                path: temporary.clone(),
                source,
            })?;
        let publication = (|| {
            file.write_all(contents)?;
            file.sync_all()?;
            match std::fs::hard_link(&temporary, &path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        })();
        let _ = std::fs::remove_file(&temporary);
        publication.map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
        self.load_effect_checkpoint(digest)?;
        #[cfg(unix)]
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| JournalError::Io {
                path: directory.to_path_buf(),
                source,
            })?;
        Ok(())
    }

    pub(crate) fn load_effect_checkpoint(&self, digest: &str) -> Result<Vec<u8>, JournalError> {
        if !valid_sha256_hex(digest) {
            return Err(JournalError::InvalidTransition(
                "invalid filesystem effect checkpoint digest".to_string(),
            ));
        }
        let path = self.effect_checkpoint_path(digest)?;
        let mut metadata = std::fs::symlink_metadata(&path).map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint is not a regular file: {}",
                path.display()
            )));
        }
        if metadata.len() > MAX_EFFECT_CHECKPOINT_BYTES {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint exceeds {MAX_EFFECT_CHECKPOINT_BYTES} bytes: {}",
                path.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            if metadata.nlink() > 1 {
                remove_stale_checkpoint_temps(
                    path.parent().expect("checkpoint path has a parent"),
                    digest,
                    true,
                )?;
                metadata = std::fs::symlink_metadata(&path).map_err(|source| JournalError::Io {
                    path: path.clone(),
                    source,
                })?;
            }
            if metadata.nlink() != 1
                || metadata.permissions().mode() & 0o077 != 0
                || metadata.uid() != self.journal_owner_uid()?
            {
                return Err(JournalError::InvalidTransition(format!(
                    "filesystem effect checkpoint has unsafe links or permissions: {}",
                    path.display()
                )));
            }
        }
        let contents = std::fs::read(&path).map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
        if sha256_hex(&contents) != digest {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint content digest mismatch: {}",
                path.display()
            )));
        }
        Ok(contents)
    }

    fn effect_checkpoint_path(&self, digest: &str) -> Result<PathBuf, JournalError> {
        let journal_path = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?
            .path
            .clone();
        Ok(effect_checkpoint_directory_for(&journal_path)?.join(digest))
    }

    #[cfg(unix)]
    fn journal_owner_uid(&self) -> Result<u32, JournalError> {
        use std::os::unix::fs::MetadataExt as _;

        let writer = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?;
        writer
            .file
            .metadata()
            .map(|metadata| metadata.uid())
            .map_err(|source| JournalError::Io {
                path: writer.path.clone(),
                source,
            })
    }

    /// Atomically publish the current reduced state and replace the redundant
    /// log prefix with its final checksum-linked envelope.
    ///
    /// The writer lease remains held throughout. Publishing the snapshot first
    /// means a crash observes either snapshot + full log or snapshot + anchor;
    /// both are complete authorities. The retained anchor makes a missing
    /// snapshot detectable whenever compaction removed a non-genesis prefix.
    pub fn compact(&self) -> Result<SessionSnapshot, JournalError> {
        self.inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?
            .compact()
    }

    /// Publish a snapshot of the exact live writer state under its writer lease.
    ///
    /// Snapshot bytes are never accepted from callers. A retained binding frame
    /// is fsynced before publication so recovery can distinguish a complete
    /// authority pair from a substituted or torn snapshot.
    pub fn publish_snapshot(&self) -> Result<SessionSnapshot, JournalError> {
        self.inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?
            .publish_snapshot()
    }

    /// Replay and verify all complete records. An unterminated final fragment is
    /// ignored; opening the writer heals that fragment before the next append.
    pub fn replay(path: impl AsRef<Path>) -> Result<Vec<JournalEnvelope>, JournalError> {
        let path = path.as_ref();
        let bytes = read_journal_if_present(path)?;
        let parsed = parse_complete_frames(path, &bytes)?;
        let snapshot = snapshot::load_snapshot_if_present(snapshot_path_for(path))?;
        recover_storage(&parsed.entries, &parsed.bindings, snapshot.as_ref(), None)?;
        verify_snapshot_authority_head_readonly(path, &parsed.bindings, snapshot.as_ref())?;
        Ok(parsed.entries)
    }

    /// Recover the complete committed state from a full log or a validated
    /// companion snapshot plus compacted suffix.
    pub fn recovered_state(path: impl AsRef<Path>) -> Result<ReducedSessionState, JournalError> {
        let path = path.as_ref();
        let bytes = read_journal_if_present(path)?;
        let parsed = parse_complete_frames(path, &bytes)?;
        let snapshot = snapshot::load_snapshot_if_present(snapshot_path_for(path))?;
        let recovery = recover_storage(&parsed.entries, &parsed.bindings, snapshot.as_ref(), None)?;
        verify_snapshot_authority_head_readonly(path, &parsed.bindings, snapshot.as_ref())?;
        Ok(recovery.state)
    }

    pub fn lease_owner(path: impl AsRef<Path>) -> Result<LeaseOwner, JournalError> {
        lease::inspect(&lease::normalized_path(path.as_ref())?)
    }

    pub(crate) fn acquire_storage_lease(
        path: impl AsRef<Path>,
        session_id: &str,
    ) -> Result<SessionStorageLease, JournalError> {
        SessionStorageLease::acquire(path.as_ref(), session_id)
    }
}

/// Write self-consistent snapshot bytes for offline fixtures and inspection.
///
/// This compatibility API cannot establish durable recovery authority. Use
/// [`SessionJournal::publish_snapshot`] while holding the journal writer lease
/// when publishing a recovery snapshot.
#[deprecated(note = "use SessionJournal::publish_snapshot for durable recovery authority")]
pub fn write_snapshot(
    path: impl AsRef<Path>,
    snapshot: &SessionSnapshot,
) -> Result<(), JournalError> {
    snapshot::write_snapshot(path, snapshot).map(|_| ())
}

/// Reduce a suffix from self-consistent snapshot data for offline use.
///
/// This function does not grant durable authority, but it enforces the same
/// schema-history boundary as authoritative recovery.
pub fn replay_from_snapshot(
    snapshot: &SessionSnapshot,
    suffix: &[JournalEnvelope],
) -> Result<ReducedSessionState, JournalError> {
    snapshot.validate()?;
    let mut previous_schema = Some(snapshot.schema_version);
    for envelope in suffix {
        reject_schema_regression(previous_schema, envelope.schema_version)?;
        previous_schema = Some(envelope.schema_version);
    }
    snapshot::replay_from_snapshot(snapshot, suffix)
}

impl SessionStorageLease {
    fn acquire(path: &Path, session_id: &str) -> Result<Self, JournalError> {
        let journal_path = lease::normalized_path(path)?;
        let lease = WriterLease::acquire(&journal_path, session_id)?;
        let journal_file = match lease::open_existing_read_write_nofollow(&journal_path) {
            Ok(file) => {
                lease::lock_data_file(&file, &journal_path)?;
                lease::ensure_path_identity(&file, &journal_path)?;
                lease::reject_multiple_links(&file, &journal_path)?;
                Some(file)
            }
            Err(JournalError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        lease.validate_current_path()?;
        if let Some(file) = journal_file.as_ref() {
            lease::ensure_path_identity(file, &journal_path)?;
        }
        Ok(Self {
            journal_path,
            session_id: session_id.to_owned(),
            _journal_file: journal_file,
            _lease: lease,
        })
    }

    pub(crate) fn remove_files(
        &self,
        session_path: &Path,
        wal_path: &Path,
    ) -> Result<(), JournalError> {
        self.validate_retirement_paths(session_path, wal_path)?;
        self.validate_journal_retirement_authority()?;
        let session = CapturedRetirementFile::capture(session_path, None)?;
        let wal = CapturedRetirementFile::capture(wal_path, None)?;
        let snapshot =
            CapturedRetirementFile::capture(&snapshot_path_for(&self.journal_path), None)?;
        let journal =
            CapturedRetirementFile::capture(&self.journal_path, self._journal_file.as_ref())?;
        let authority_head = CapturedRetirementFile::capture(
            &snapshot::snapshot_authority_head_path(&self.journal_path),
            None,
        )?;

        // Attempt every artifact so one undeletable file does not strand other
        // plaintext. The caller retains index authority if any unlink or
        // directory sync fails, making every residual file discoverable.
        let mut first_error = None;
        if let Err(error) = remove_effect_checkpoint_directory(&self.journal_path) {
            first_error = Some(error);
        }
        for captured in [&session, &wal, &snapshot, &journal] {
            if let Err(error) = captured.remove()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if first_error.is_none() {
            if let Err(error) = authority_head.remove() {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn validate_journal_retirement_authority(&self) -> Result<(), JournalError> {
        match self._journal_file.as_ref() {
            Some(file) => lease::ensure_path_identity(file, &self.journal_path),
            None => require_path_absent(&self.journal_path),
        }
    }

    fn validate_retirement_paths(
        &self,
        session_path: &Path,
        wal_path: &Path,
    ) -> Result<(), JournalError> {
        let journal_parent = self.journal_path.parent().ok_or_else(|| {
            JournalError::InvalidTransition("session journal has no parent".to_owned())
        })?;
        let session_parent = canonical_existing_parent(session_path)?;
        let wal_parent = canonical_existing_parent(wal_path)?;
        let expected_journal_name = format!("{}.journal", self.session_id);
        let expected_session_suffix = format!("_{}.json", self.session_id);
        let session_name = session_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if self.journal_path.file_name().and_then(|name| name.to_str())
            != Some(expected_journal_name.as_str())
            || session_parent != journal_parent
            || wal_parent != journal_parent
            || !session_name.ends_with(&expected_session_suffix)
            || wal_path != session_path.with_extension("wal")
        {
            return Err(JournalError::InvalidTransition(
                "session retirement paths do not match the leased journal authority".to_owned(),
            ));
        }
        Ok(())
    }
}

fn canonical_existing_parent(path: &Path) -> Result<PathBuf, JournalError> {
    let parent = path.parent().ok_or_else(|| {
        JournalError::InvalidTransition("session retirement path has no parent".to_owned())
    })?;
    std::fs::canonicalize(parent).map_err(|source| JournalError::Io {
        path: parent.to_path_buf(),
        source,
    })
}

fn require_path_absent(path: &Path) -> Result<(), JournalError> {
    match std::fs::symlink_metadata(path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(JournalError::Io {
            path: path.to_path_buf(),
            source,
        }),
        Ok(_) => Err(JournalError::PathIdentityMismatch {
            path: path.to_path_buf(),
        }),
    }
}

enum CapturedRetirementFile {
    Missing(PathBuf),
    Present { path: PathBuf, file: File },
}

impl CapturedRetirementFile {
    fn capture(path: &Path, expected: Option<&File>) -> Result<Self, JournalError> {
        let file = match lease::open_existing_nofollow(path) {
            Ok(file) => file,
            Err(JournalError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound && expected.is_none() =>
            {
                return Ok(Self::Missing(path.to_path_buf()));
            }
            Err(JournalError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Err(JournalError::PathIdentityMismatch {
                    path: path.to_path_buf(),
                });
            }
            Err(error) => return Err(error),
        };
        if let Some(expected) = expected {
            lease::ensure_same_identity(expected, &file, path)?;
        }
        lease::ensure_path_identity(&file, path)?;
        Ok(Self::Present {
            path: path.to_path_buf(),
            file,
        })
    }

    fn remove(&self) -> Result<(), JournalError> {
        match self {
            Self::Missing(path) => require_path_absent(path),
            Self::Present { path, file } => {
                lease::ensure_path_identity(file, path)?;
                // Rust's portable filesystem API has no unlink-by-handle
                // primitive. The held session lease and this final identity
                // probe bound the supported race window; direct same-user
                // directory mutation after the probe is outside this portable
                // authority floor.
                std::fs::remove_file(path).map_err(|source| JournalError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
                snapshot::sync_parent_directory(path)
            }
        }
    }
}

fn effect_checkpoint_directory_for(journal_path: &Path) -> Result<PathBuf, JournalError> {
    let file_name = journal_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            JournalError::InvalidTransition(
                "session journal filename is not valid UTF-8".to_string(),
            )
        })?;
    Ok(journal_path.with_file_name(format!(".{file_name}.effects")))
}

fn remove_stale_checkpoint_temps(
    directory: &Path,
    digest: &str,
    published: bool,
) -> Result<(), JournalError> {
    let prefix = format!(".{digest}.");
    let entries = std::fs::read_dir(directory).map_err(|source| JournalError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| JournalError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".tmp") {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.is_dir() {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint temporary path is a directory: {}",
                path.display()
            )));
        }
        if published {
            std::fs::remove_file(&path).map_err(|source| JournalError::Io {
                path: path.clone(),
                source,
            })?;
        } else if metadata.file_type().is_symlink() || metadata.is_file() {
            std::fs::remove_file(&path).map_err(|source| JournalError::Io {
                path: path.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

fn checkpoint_directory_bytes(directory: &Path) -> Result<u64, JournalError> {
    let mut total = 0_u64;
    for entry in std::fs::read_dir(directory).map_err(|source| JournalError::Io {
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| JournalError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint store contains an unsafe entry: {}",
                path.display()
            )));
        }
        total = total.checked_add(metadata.len()).ok_or_else(|| {
            JournalError::InvalidTransition(
                "filesystem effect checkpoint store size overflow".to_string(),
            )
        })?;
    }
    Ok(total)
}

fn remove_effect_checkpoint_directory(journal_path: &Path) -> Result<(), JournalError> {
    let directory = effect_checkpoint_directory_for(journal_path)?;
    let metadata = match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(JournalError::Io {
                path: directory,
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() {
        std::fs::remove_file(&directory).map_err(|source| JournalError::Io {
            path: directory.clone(),
            source,
        })?;
        return snapshot::sync_parent_directory(&directory);
    }
    if !metadata.is_dir() {
        return Err(JournalError::InvalidTransition(format!(
            "filesystem effect checkpoint path is not a directory: {}",
            directory.display()
        )));
    }

    let mut first_error = None;
    for entry in std::fs::read_dir(&directory).map_err(|source| JournalError::Io {
        path: directory.clone(),
        source,
    })? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) => {
                if first_error.is_none() {
                    first_error = Some(JournalError::Io {
                        path: directory.clone(),
                        source,
                    });
                }
                continue;
            }
        };
        let path = entry.path();
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                if first_error.is_none() {
                    first_error = Some(JournalError::InvalidTransition(format!(
                        "filesystem effect checkpoint directory contains a subdirectory: {}",
                        path.display()
                    )));
                }
            }
            Ok(_) => {
                if let Err(source) = std::fs::remove_file(&path)
                    && first_error.is_none()
                {
                    first_error = Some(JournalError::Io { path, source });
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) if first_error.is_none() => {
                first_error = Some(JournalError::Io { path, source });
            }
            Err(_) => {}
        }
    }
    if let Err(source) = std::fs::remove_dir(&directory)
        && first_error.is_none()
    {
        first_error = Some(JournalError::Io {
            path: directory.clone(),
            source,
        });
    }
    if first_error.is_none()
        && let Err(error) = snapshot::sync_parent_directory(&directory)
    {
        first_error = Some(error);
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

impl JournalWriter {
    fn open(path: PathBuf, session_id: String) -> Result<Self, JournalError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| JournalError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let lease = WriterLease::acquire(&path, &session_id)?;
        let mut file = lease::open_or_create_nofollow(&path)?;
        lease::lock_data_file(&file, &path)?;
        lease::ensure_path_identity(&file, &path)?;
        lease::reject_multiple_links(&file, &path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|source| JournalError::Io {
                    path: path.clone(),
                    source,
                })?;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| JournalError::Io {
                path: path.clone(),
                source,
            })?;
        let parsed = parse_complete_frames(&path, &bytes)?;
        // Heal a torn terminal fragment before reconciliation can append a
        // missing authority frame. Truncating afterward would discard that
        // repaired frame and leave the promoted sidecar without journal proof.
        if parsed.valid_len < bytes.len() {
            file.set_len(parsed.valid_len as u64)
                .and_then(|()| file.sync_all())
                .map_err(|source| JournalError::Io {
                    path: path.clone(),
                    source,
                })?;
        }
        let mut snapshot = snapshot::load_snapshot_if_present(snapshot_path_for(&path))?;
        let recovery = recover_storage(
            &parsed.entries,
            &parsed.bindings,
            snapshot.as_ref(),
            Some(&session_id),
        )?;
        snapshot = reconcile_snapshot_authority_head(
            &path,
            &mut file,
            &parsed.bindings,
            snapshot.as_ref(),
            &recovery.state,
            &session_id,
        )?;
        file.seek(SeekFrom::End(0))
            .map_err(|source| JournalError::Io {
                path: path.clone(),
                source,
            })?;
        let legacy_snapshot = snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.schema_version == LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION
        });
        let mut writer = Self {
            path,
            session_id,
            file,
            next_seq: recovery.next_seq,
            previous_checksum: recovery.previous_checksum,
            state: recovery.state,
            last_envelope: recovery.last_envelope,
            base_snapshot: snapshot,
            faulted: false,
            _lease: lease,
        };
        if legacy_snapshot {
            writer.publish_snapshot()?;
        }
        writer._lease.validate_current_path()?;
        lease::ensure_path_identity(&writer.file, &writer.path)?;
        Ok(writer)
    }

    fn append(&mut self, event: SessionEvent) -> Result<JournalEnvelope, JournalError> {
        if self.faulted {
            return Err(JournalError::WriterFaulted);
        }
        let envelope = JournalEnvelope::create(
            self.session_id.clone(),
            self.next_seq,
            self.previous_checksum.clone(),
            event,
        )?;
        let candidate_state = reduce(self.state.clone(), &envelope)?;
        let body = serde_json::to_vec(&envelope).map_err(|source| JournalError::Json {
            context: "encoding journal envelope",
            source,
        })?;
        let frame = encode_frame(&body)?;
        if let Err(error) = lease::ensure_path_identity(&self.file, &self.path) {
            self.faulted = true;
            return Err(error);
        }
        if let Err(source) = self
            .file
            .seek(SeekFrom::End(0))
            .and_then(|_| self.file.write_all(&frame))
            .and_then(|()| self.file.sync_all())
        {
            self.faulted = true;
            return Err(JournalError::Io {
                path: self.path.clone(),
                source,
            });
        }
        if let Err(error) = lease::ensure_path_identity(&self.file, &self.path) {
            self.faulted = true;
            return Err(error);
        }
        self.next_seq += 1;
        self.previous_checksum.clone_from(&envelope.checksum);
        self.state = candidate_state;
        self.last_envelope = Some(envelope.clone());
        Ok(envelope)
    }

    fn committed_authority(&mut self) -> Result<CommittedJournalAuthority, JournalError> {
        if self.faulted {
            return Err(JournalError::WriterFaulted);
        }
        self.ensure_current_path_identity()?;

        let authority = (|| {
            let mut bytes = Vec::new();
            self.file
                .seek(SeekFrom::Start(0))
                .and_then(|_| self.file.read_to_end(&mut bytes))
                .map_err(|source| JournalError::Io {
                    path: self.path.clone(),
                    source,
                })?;
            self.ensure_current_path_identity()?;
            let parsed = parse_complete_frames(&self.path, &bytes)?;
            verify_snapshot_authority_head_readonly(
                &self.path,
                &parsed.bindings,
                self.base_snapshot.as_ref(),
            )?;
            let entries = parsed.entries;
            if let Some(first) = entries.first() {
                verify_chain_from(
                    &entries,
                    first.seq,
                    &first.previous_checksum,
                    &self.session_id,
                )?;
            }

            let head = entries.last();
            let head_matches_state = match (self.state.last_seq, head) {
                (None, None) => {
                    self.state.last_checksum == GENESIS_CHECKSUM && self.last_envelope.is_none()
                }
                (Some(state_seq), Some(head)) => {
                    head.seq == state_seq
                        && head.checksum == self.state.last_checksum
                        && self.last_envelope.as_ref() == Some(head)
                }
                _ => false,
            };
            if !head_matches_state {
                return Err(JournalError::JournalAuthorityMismatch(format!(
                    "state cursor {:?}/{} does not match committed head {:?}/{}",
                    self.state.last_seq,
                    self.state.last_checksum,
                    head.map(|entry| entry.seq),
                    head.map_or(GENESIS_CHECKSUM, |entry| entry.checksum.as_str())
                )));
            }

            Ok(CommittedJournalAuthority {
                state: self.state.clone(),
                entries,
                base_snapshot: self.base_snapshot.clone(),
            })
        })();

        if let Err(source) = self.file.seek(SeekFrom::End(0)) {
            self.faulted = true;
            return Err(JournalError::Io {
                path: self.path.clone(),
                source,
            });
        }
        if authority.is_err() {
            self.faulted = true;
        }
        authority
    }

    fn compact(&mut self) -> Result<SessionSnapshot, JournalError> {
        if self.faulted {
            return Err(JournalError::WriterFaulted);
        }
        if self.state.last_seq.is_some() && self.last_envelope.is_none() {
            return Err(JournalError::SnapshotJournalMismatch(
                "cannot compact a snapshot-only authority without its anchor envelope".to_owned(),
            ));
        }
        lease::reject_multiple_links(&self.file, &self.path)?;
        self.ensure_current_path_identity()?;
        let snapshot = SessionSnapshot::new(self.session_id.clone(), self.state.clone())?;
        let binding = SnapshotAuthorityBinding::new(&snapshot);
        let binding_frame = encode_snapshot_authority_frame(&binding)?;
        let mut replacement = match self.last_envelope.as_ref() {
            Some(anchor) => {
                let body = serde_json::to_vec(anchor).map_err(|source| JournalError::Json {
                    context: "encoding compacted journal anchor",
                    source,
                })?;
                encode_frame(&body)?
            }
            None => Vec::new(),
        };
        replacement.extend_from_slice(&binding_frame);
        if let Err(error) = self.begin_snapshot_authority(&binding) {
            self.faulted = true;
            return Err(error);
        }
        let snapshot_path = snapshot_path_for(&self.path);
        let publication = (|| {
            self.append_authority_frame(&binding_frame)?;
            let snapshot_file = snapshot::write_snapshot(&snapshot_path, &snapshot)?;
            // `persist` is an atomic replacement on supported tempfile platforms.
            // There is deliberately no remove-then-rename fallback: that would
            // create an authority gap on Windows and violate the journal contract.
            let mut file = snapshot::replace_file_atomically(&self.path, &replacement)?;
            snapshot::sync_parent_directory(&self.path)?;
            lease::ensure_path_identity(&file, &self.path)?;
            file.seek(SeekFrom::End(0))
                .map_err(|source| JournalError::Io {
                    path: self.path.clone(),
                    source,
                })?;
            Ok((file, snapshot_file))
        })();
        let snapshot_file = match publication {
            Ok((file, snapshot_file)) => {
                self.file = file;
                snapshot_file
            }
            Err(error) => {
                // Once snapshot publication starts, an error can leave the
                // pathname and this open handle referring to different files.
                // Reopening is the only safe way to recover authority.
                self.faulted = true;
                return Err(error);
            }
        };
        if let Err(error) = finish_snapshot_authority(&self.path, &binding, &snapshot_file) {
            self.faulted = true;
            return Err(error);
        }
        drop(snapshot_file);
        self.base_snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    fn publish_snapshot(&mut self) -> Result<SessionSnapshot, JournalError> {
        if self.faulted {
            return Err(JournalError::WriterFaulted);
        }
        lease::reject_multiple_links(&self.file, &self.path)?;
        self.ensure_current_path_identity()?;
        let snapshot = SessionSnapshot::new(self.session_id.clone(), self.state.clone())?;
        let binding = SnapshotAuthorityBinding::new(&snapshot);
        let head = snapshot::load_snapshot_authority_head(&self.path)?;
        if self.base_snapshot.as_ref() == Some(&snapshot)
            && snapshot::load_snapshot(snapshot_path_for(&self.path))
                .as_ref()
                .is_ok_and(|loaded| loaded == &snapshot)
            && head.as_ref().is_some_and(|head| {
                head.accepted.as_ref() == Some(&binding) && head.pending.is_none()
            })
        {
            return Ok(snapshot);
        }
        let frame = encode_snapshot_authority_frame(&binding)?;
        if let Err(error) = self.begin_snapshot_authority(&binding) {
            self.faulted = true;
            return Err(error);
        }
        let snapshot_file = match self.append_authority_frame(&frame).and_then(|()| {
            let snapshot_file = snapshot::write_snapshot(snapshot_path_for(&self.path), &snapshot)?;
            lease::ensure_path_identity(&self.file, &self.path)?;
            Ok(snapshot_file)
        }) {
            Ok(snapshot_file) => snapshot_file,
            Err(error) => {
                self.faulted = true;
                return Err(error);
            }
        };
        if let Err(error) = finish_snapshot_authority(&self.path, &binding, &snapshot_file) {
            self.faulted = true;
            return Err(error);
        }
        drop(snapshot_file);
        self.base_snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }

    fn begin_snapshot_authority(
        &self,
        binding: &SnapshotAuthorityBinding,
    ) -> Result<(), JournalError> {
        let mut head = snapshot::load_snapshot_authority_head(&self.path)?.unwrap_or_default();
        let accepted_matches_base = match (head.accepted.as_ref(), self.base_snapshot.as_ref()) {
            (Some(accepted), Some(base)) => accepted.matches(base),
            (None, None) => true,
            (None, Some(base)) => base.schema_version == LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION,
            _ => false,
        };
        if !accepted_matches_base {
            return Err(JournalError::SnapshotAuthorityMismatch);
        }
        if head
            .pending
            .as_ref()
            .is_some_and(|pending| pending != binding)
        {
            return Err(JournalError::SnapshotAuthorityMismatch);
        }
        head.pending = Some(binding.clone());
        snapshot::write_snapshot_authority_head(&self.path, &head)
    }

    fn append_authority_frame(&mut self, frame: &[u8]) -> Result<(), JournalError> {
        lease::ensure_path_identity(&self.file, &self.path)?;
        self.file
            .seek(SeekFrom::End(0))
            .and_then(|_| self.file.write_all(frame))
            .and_then(|()| self.file.sync_all())
            .map_err(|source| JournalError::Io {
                path: self.path.clone(),
                source,
            })?;
        lease::ensure_path_identity(&self.file, &self.path)
    }

    fn ensure_current_path_identity(&mut self) -> Result<(), JournalError> {
        if let Err(error) = lease::ensure_path_identity(&self.file, &self.path) {
            self.faulted = true;
            Err(error)
        } else {
            Ok(())
        }
    }
}

fn finish_snapshot_authority(
    journal_path: &Path,
    binding: &SnapshotAuthorityBinding,
    snapshot_file: &File,
) -> Result<(), JournalError> {
    let pending_head = snapshot::load_snapshot_authority_head(journal_path)?
        .ok_or(JournalError::SnapshotAuthorityMismatch)?;
    if pending_head.pending.as_ref() != Some(binding) {
        return Err(JournalError::SnapshotAuthorityMismatch);
    }
    let snapshot_path = snapshot_path_for(journal_path);
    snapshot::validate_snapshot_authority_file(snapshot_file, &snapshot_path)?;
    let mut accepted_head = pending_head.clone();
    accepted_head.accepted = accepted_head.pending.take();
    snapshot::write_snapshot_authority_head(journal_path, &accepted_head)?;
    run_after_snapshot_authority_write_hook(&snapshot_path);
    if let Err(error) = snapshot::validate_snapshot_authority_file(snapshot_file, &snapshot_path) {
        snapshot::write_snapshot_authority_head(journal_path, &pending_head)?;
        return Err(error);
    }
    Ok(())
}

struct StorageRecovery {
    state: ReducedSessionState,
    next_seq: u64,
    previous_checksum: String,
    last_envelope: Option<JournalEnvelope>,
}

fn reconcile_snapshot_authority_head(
    journal_path: &Path,
    journal_file: &mut File,
    bindings: &[SnapshotAuthorityBinding],
    snapshot: Option<&SessionSnapshot>,
    recovered_state: &ReducedSessionState,
    session_id: &str,
) -> Result<Option<SessionSnapshot>, JournalError> {
    let current_snapshot = snapshot.cloned();
    let Some(head) = snapshot::load_snapshot_authority_head(journal_path)? else {
        if let Some(snapshot) = current_snapshot
            .as_ref()
            .filter(|snapshot| snapshot.schema_version == SESSION_SNAPSHOT_SCHEMA_VERSION)
        {
            let binding = SnapshotAuthorityBinding::new(snapshot);
            if !bindings.iter().any(|retained| retained == &binding) {
                return Err(JournalError::SnapshotAuthorityMismatch);
            }
            let head = SnapshotAuthorityHead {
                accepted: Some(binding),
                ..SnapshotAuthorityHead::default()
            };
            snapshot::write_snapshot_authority_head(journal_path, &head)?;
        }
        return Ok(current_snapshot);
    };

    if let Some(pending) = head.pending.clone() {
        let target = SessionSnapshot::new(session_id, recovered_state.clone())?;
        if !pending.matches(&target) {
            return Err(JournalError::SnapshotAuthorityMismatch);
        }
        if !bindings.iter().any(|retained| retained == &pending) {
            let frame = encode_snapshot_authority_frame(&pending)?;
            lease::ensure_path_identity(journal_file, journal_path)?;
            journal_file
                .seek(SeekFrom::End(0))
                .and_then(|_| journal_file.write_all(&frame))
                .and_then(|()| journal_file.sync_all())
                .map_err(|source| JournalError::Io {
                    path: journal_path.to_path_buf(),
                    source,
                })?;
            lease::ensure_path_identity(journal_file, journal_path)?;
        }
        let snapshot_path = snapshot_path_for(journal_path);
        let snapshot_file = snapshot::write_snapshot(&snapshot_path, &target)?;
        finish_snapshot_authority(journal_path, &pending, &snapshot_file)?;
        drop(snapshot_file);
        return Ok(Some(target));
    }

    match (head.accepted.as_ref(), current_snapshot.as_ref()) {
        (Some(accepted), Some(snapshot))
            if accepted.matches(snapshot)
                && bindings.iter().any(|retained| retained == accepted) =>
        {
            Ok(current_snapshot)
        }
        (None, None) => Ok(None),
        (None, Some(snapshot))
            if snapshot.schema_version == LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION =>
        {
            Ok(current_snapshot)
        }
        _ => Err(JournalError::SnapshotAuthorityMismatch),
    }
}

fn verify_snapshot_authority_head_readonly(
    journal_path: &Path,
    bindings: &[SnapshotAuthorityBinding],
    snapshot: Option<&SessionSnapshot>,
) -> Result<(), JournalError> {
    let Some(head) = snapshot::load_snapshot_authority_head(journal_path)? else {
        return Ok(());
    };
    if head.pending.is_some() {
        return Err(JournalError::SnapshotAuthorityMismatch);
    }
    match (head.accepted.as_ref(), snapshot) {
        (Some(accepted), Some(snapshot))
            if accepted.matches(snapshot)
                && bindings.iter().any(|retained| retained == accepted) =>
        {
            Ok(())
        }
        (None, None) => Ok(()),
        (None, Some(snapshot))
            if snapshot.schema_version == LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION =>
        {
            Ok(())
        }
        _ => Err(JournalError::SnapshotAuthorityMismatch),
    }
}

fn recover_storage(
    entries: &[JournalEnvelope],
    bindings: &[SnapshotAuthorityBinding],
    snapshot: Option<&SessionSnapshot>,
    expected_session: Option<&str>,
) -> Result<StorageRecovery, JournalError> {
    if let Some(snapshot) = snapshot {
        snapshot.validate()?;
        if let Some(expected) = expected_session
            && snapshot.session_id != expected
        {
            return Err(JournalError::SessionMismatch {
                expected: expected.to_owned(),
                found: snapshot.session_id.clone(),
            });
        }
    }

    let state = match snapshot {
        Some(snapshot) if snapshot.schema_version == SESSION_SNAPSHOT_SCHEMA_VERSION => {
            recover_bound_snapshot(entries, bindings, snapshot)?
        }
        Some(snapshot) => recover_legacy_snapshot(entries, snapshot)?,
        None => recover_without_snapshot(entries, bindings, expected_session)?,
    };

    if let Some(expected) = expected_session
        && let Some(found) = state.session_id.as_deref()
        && found != expected
    {
        return Err(JournalError::SessionMismatch {
            expected: expected.to_owned(),
            found: found.to_owned(),
        });
    }
    let next_seq = match state.last_seq {
        Some(seq) => seq.checked_add(1).ok_or_else(|| {
            JournalError::InvalidTransition("journal sequence is exhausted".to_owned())
        })?,
        None => 0,
    };
    Ok(StorageRecovery {
        previous_checksum: state.last_checksum.clone(),
        state,
        next_seq,
        last_envelope: entries.last().cloned(),
    })
}

fn recover_without_snapshot(
    entries: &[JournalEnvelope],
    bindings: &[SnapshotAuthorityBinding],
    expected_session: Option<&str>,
) -> Result<ReducedSessionState, JournalError> {
    match entries.first() {
        None if bindings.is_empty() => Ok(ReducedSessionState::default()),
        None => {
            let binding = bindings
                .last()
                .ok_or(JournalError::SnapshotAuthorityMismatch)?;
            if binding.cursor.is_some() || binding.cursor_checksum != GENESIS_CHECKSUM {
                return Err(JournalError::SnapshotAuthorityMismatch);
            }
            let mut state = ReducedSessionState {
                session_id: Some(binding.session_id.clone()),
                ..ReducedSessionState::default()
            };
            if state.digest()? != binding.state_digest {
                return Err(JournalError::SnapshotAuthorityMismatch);
            }
            if let Some(expected) = expected_session
                && binding.session_id != expected
            {
                return Err(JournalError::SessionMismatch {
                    expected: expected.to_owned(),
                    found: binding.session_id.clone(),
                });
            }
            state.last_checksum = GENESIS_CHECKSUM.to_owned();
            Ok(state)
        }
        Some(first) if first.seq == 0 => {
            verify_chain_for_session(entries, expected_session)?;
            replay_state(entries)
        }
        Some(first) => Err(JournalError::CompactedJournalMissingSnapshot {
            first_seq: first.seq,
        }),
    }
}

fn recover_bound_snapshot(
    entries: &[JournalEnvelope],
    bindings: &[SnapshotAuthorityBinding],
    snapshot: &SessionSnapshot,
) -> Result<ReducedSessionState, JournalError> {
    let snapshot = BoundSessionSnapshot::from_retained_binding(snapshot, bindings)?;
    let raw = snapshot.snapshot;
    match (raw.cursor, entries.first()) {
        (None, None) => Ok(raw.state.clone()),
        (Some(_), None) => Err(JournalError::SnapshotJournalMismatch(
            "bound snapshot has a committed cursor but its journal anchor is missing".to_owned(),
        )),
        (None, Some(first)) if first.seq == 0 => {
            verify_chain_for_session(entries, Some(&raw.session_id))?;
            replay_from_bound_snapshot(&snapshot, entries)
        }
        (None, Some(first)) => Err(JournalError::SnapshotJournalMismatch(format!(
            "genesis snapshot cannot seed journal sequence {}",
            first.seq
        ))),
        (Some(cursor), Some(first)) if first.seq <= cursor => {
            verify_chain_from(
                entries,
                first.seq,
                &first.previous_checksum,
                &raw.session_id,
            )?;
            let cursor_index = usize::try_from(cursor - first.seq).map_err(|_| {
                JournalError::SnapshotJournalMismatch(
                    "snapshot cursor offset does not fit this platform".to_owned(),
                )
            })?;
            let anchor = entries.get(cursor_index).ok_or_else(|| {
                JournalError::SnapshotJournalMismatch(format!(
                    "snapshot cursor {cursor} is ahead of a {}-record retained log",
                    entries.len()
                ))
            })?;
            if anchor.checksum != raw.cursor_checksum {
                return Err(JournalError::SnapshotJournalMismatch(format!(
                    "snapshot checksum does not match full-log sequence {cursor}"
                )));
            }
            replay_from_bound_snapshot(&snapshot, &entries[cursor_index + 1..])
        }
        (Some(cursor), Some(first)) if first.seq == cursor.saturating_add(1) => {
            verify_chain_from(entries, first.seq, &raw.cursor_checksum, &raw.session_id)?;
            replay_from_bound_snapshot(&snapshot, entries)
        }
        (Some(cursor), Some(first)) => Err(JournalError::SnapshotJournalMismatch(format!(
            "snapshot cursor {cursor} cannot seed journal sequence {}",
            first.seq
        ))),
    }
}

fn recover_legacy_snapshot(
    entries: &[JournalEnvelope],
    snapshot: &SessionSnapshot,
) -> Result<ReducedSessionState, JournalError> {
    if entries.first().is_some_and(|first| first.seq > 0) {
        // A v4 snapshot has no retained authority binding. Once compaction has
        // discarded the prefix, its state cannot be reconstructed and compared
        // with the log. Migrating it would mint v5 authority for unproved bytes.
        return Err(JournalError::SnapshotAuthorityMismatch);
    }
    match (Some(snapshot), entries.first()) {
        (Some(_), None) => {
            return Err(JournalError::SnapshotJournalMismatch(
                "legacy snapshot has no complete seq-0 journal prefix".to_owned(),
            ));
        }
        (Some(snapshot), Some(first)) if first.seq == 0 => {
            verify_chain_for_session(entries, Some(&snapshot.session_id))?;
            let prefix_len = match snapshot.cursor {
                Some(cursor) => usize::try_from(cursor)
                    .ok()
                    .and_then(|cursor| cursor.checked_add(1))
                    .ok_or_else(|| {
                        JournalError::SnapshotJournalMismatch(
                            "snapshot cursor does not fit this platform".to_owned(),
                        )
                    })?,
                None => 0,
            };
            if entries.len() < prefix_len {
                return Err(JournalError::SnapshotJournalMismatch(format!(
                    "snapshot cursor {:?} is ahead of a {}-record full log",
                    snapshot.cursor,
                    entries.len()
                )));
            }
            if let Some(cursor) = snapshot.cursor
                && entries[prefix_len - 1].checksum != snapshot.cursor_checksum
            {
                return Err(JournalError::SnapshotJournalMismatch(format!(
                    "snapshot checksum does not match full-log sequence {cursor}"
                )));
            }
            let mut prefix_state = replay_state(&entries[..prefix_len])?;
            if prefix_len == 0 {
                prefix_state.session_id = Some(snapshot.session_id.clone());
            }
            let snapshot = BoundSessionSnapshot::from_replayed_prefix(snapshot, &prefix_state)?;
            replay_from_bound_snapshot(&snapshot, &entries[prefix_len..])
        }
        (Some(_), Some(_)) => Err(JournalError::SnapshotAuthorityMismatch),
        _ => unreachable!("legacy recovery always receives a snapshot"),
    }
}

fn replay_from_bound_snapshot(
    snapshot: &BoundSessionSnapshot<'_>,
    suffix: &[JournalEnvelope],
) -> Result<ReducedSessionState, JournalError> {
    let mut previous_schema = Some(snapshot.snapshot.schema_version);
    suffix
        .iter()
        .try_fold(snapshot.snapshot.state.clone(), |state, envelope| {
            reject_schema_regression(previous_schema, envelope.schema_version)?;
            previous_schema = Some(envelope.schema_version);
            reduce(state, envelope)
        })
}

fn verify_chain_for_session(
    entries: &[JournalEnvelope],
    expected_session: Option<&str>,
) -> Result<(), JournalError> {
    let expected_session = expected_session
        .or_else(|| entries.first().map(|entry| entry.session_id.as_str()))
        .unwrap_or_default();
    verify_chain_from(entries, 0, GENESIS_CHECKSUM, expected_session)
}

fn verify_chain_from(
    entries: &[JournalEnvelope],
    first_seq: u64,
    previous_checksum: &str,
    expected_session: &str,
) -> Result<(), JournalError> {
    let mut previous = previous_checksum.to_owned();
    let mut previous_schema = None;
    for (offset, entry) in entries.iter().enumerate() {
        validate_journal_schema_for_reader(entry.schema_version)?;
        enforce_typed_event_schema_boundary(entry)?;
        reject_schema_regression(previous_schema, entry.schema_version)?;
        previous_schema = Some(entry.schema_version);
        if entry.session_id != expected_session {
            return Err(JournalError::SessionMismatch {
                expected: expected_session.to_owned(),
                found: entry.session_id.clone(),
            });
        }
        let expected_seq = first_seq
            .checked_add(u64::try_from(offset).map_err(|_| {
                JournalError::InvalidTransition("journal sequence offset overflow".to_owned())
            })?)
            .ok_or_else(|| {
                JournalError::InvalidTransition("journal sequence is exhausted".to_owned())
            })?;
        if entry.seq != expected_seq {
            return Err(JournalError::SequenceMismatch {
                expected: expected_seq,
                found: entry.seq,
            });
        }
        if entry.previous_checksum != previous {
            return Err(JournalError::PreviousChecksumMismatch { seq: entry.seq });
        }
        if entry.computed_checksum()? != entry.checksum {
            return Err(JournalError::ChecksumMismatch { seq: entry.seq });
        }
        previous.clone_from(&entry.checksum);
    }
    Ok(())
}

pub fn verify_chain(entries: &[JournalEnvelope]) -> Result<(), JournalError> {
    verify_chain_for_session(entries, None)
}

fn encode_frame(body: &[u8]) -> Result<Vec<u8>, JournalError> {
    let length = u32::try_from(body.len()).map_err(|_| {
        JournalError::InvalidTransition(
            "journal envelope exceeds the frame length limit".to_owned(),
        )
    })?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(JournalError::InvalidTransition(
            "journal envelope exceeds the maximum frame size".to_owned(),
        ));
    }
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + body.len() + FRAME_DIGEST_BYTES);
    frame.extend_from_slice(FRAME_MAGIC);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&(!length).to_be_bytes());
    frame.extend_from_slice(body);
    frame.extend_from_slice(&sha256_bytes(body));
    Ok(frame)
}

fn encode_snapshot_authority_frame(
    binding: &SnapshotAuthorityBinding,
) -> Result<Vec<u8>, JournalError> {
    binding.validate()?;
    let body = serde_json::to_vec(binding).map_err(|source| JournalError::Json {
        context: "encoding snapshot authority binding",
        source,
    })?;
    if body.len() > MAX_SNAPSHOT_AUTHORITY_BYTES {
        return Err(JournalError::InvalidTransition(
            "snapshot authority binding exceeds the maximum frame size".to_owned(),
        ));
    }
    let length = u32::try_from(body.len()).map_err(|_| {
        JournalError::InvalidTransition(
            "snapshot authority binding exceeds the frame length limit".to_owned(),
        )
    })?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + body.len() + FRAME_DIGEST_BYTES);
    frame.extend_from_slice(SNAPSHOT_AUTHORITY_MAGIC);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&(!length).to_be_bytes());
    frame.extend_from_slice(&body);
    frame.extend_from_slice(&sha256_bytes(&body));
    Ok(frame)
}

fn parse_complete_frames(path: &Path, bytes: &[u8]) -> Result<ParsedJournal, JournalError> {
    let mut entries = Vec::new();
    let mut bindings = Vec::new();
    let mut offset = 0;
    let mut frame_number = 1;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.len() < FRAME_HEADER_BYTES {
            break;
        }
        let magic = &remaining[..4];
        if magic != FRAME_MAGIC && magic != SNAPSHOT_AUTHORITY_MAGIC {
            return Err(JournalError::InvalidFrameHeader {
                path: path.to_path_buf(),
                frame: frame_number,
            });
        }
        let length = u32::from_be_bytes(remaining[4..8].try_into().map_err(|_| {
            JournalError::InvalidFrameHeader {
                path: path.to_path_buf(),
                frame: frame_number,
            }
        })?);
        let inverse = u32::from_be_bytes(remaining[8..12].try_into().map_err(|_| {
            JournalError::InvalidFrameHeader {
                path: path.to_path_buf(),
                frame: frame_number,
            }
        })?);
        if inverse != !length {
            return Err(JournalError::InvalidFrameHeader {
                path: path.to_path_buf(),
                frame: frame_number,
            });
        }
        let length = length as usize;
        let max_length = if magic == SNAPSHOT_AUTHORITY_MAGIC {
            MAX_SNAPSHOT_AUTHORITY_BYTES
        } else {
            MAX_FRAME_BYTES
        };
        if length > max_length {
            return Err(JournalError::FrameTooLarge {
                path: path.to_path_buf(),
                frame: frame_number,
            });
        }
        let frame_len = FRAME_HEADER_BYTES + length + FRAME_DIGEST_BYTES;
        if remaining.len() < frame_len {
            break;
        }
        let body = &remaining[FRAME_HEADER_BYTES..FRAME_HEADER_BYTES + length];
        let stored_digest = &remaining[FRAME_HEADER_BYTES + length..frame_len];
        if stored_digest != sha256_bytes(body) {
            return Err(JournalError::FrameDigestMismatch {
                path: path.to_path_buf(),
                frame: frame_number,
            });
        }
        if magic == SNAPSHOT_AUTHORITY_MAGIC {
            let binding =
                serde_json::from_slice::<SnapshotAuthorityBinding>(body).map_err(|source| {
                    JournalError::CorruptFrame {
                        path: path.to_path_buf(),
                        frame: frame_number,
                        source,
                    }
                })?;
            binding.validate()?;
            validate_snapshot_binding_position(&entries, &bindings, &binding)?;
            bindings.push(binding);
        } else {
            let schema = serde_json::from_slice::<EnvelopeSchema>(body).map_err(|source| {
                JournalError::CorruptFrame {
                    path: path.to_path_buf(),
                    frame: frame_number,
                    source,
                }
            })?;
            validate_journal_schema_for_reader(schema.schema_version)?;
            reject_schema_regression(
                entries.last().map(|entry| entry.schema_version),
                schema.schema_version,
            )?;
            enforce_event_schema_boundary(body, schema.schema_version)?;
            let entry: JournalEnvelope =
                serde_json::from_slice(body).map_err(|source| JournalError::CorruptFrame {
                    path: path.to_path_buf(),
                    frame: frame_number,
                    source,
                })?;
            reject_unknown_event_fields(body, &entry)?;
            entries.push(entry);
        }
        offset += frame_len;
        frame_number += 1;
    }
    Ok(ParsedJournal {
        entries,
        bindings,
        valid_len: offset,
    })
}

fn reject_unknown_event_fields(body: &[u8], entry: &JournalEnvelope) -> Result<(), JournalError> {
    let raw =
        serde_json::from_slice::<serde_json::Value>(body).map_err(|source| JournalError::Json {
            context: "checking journal event fields",
            source,
        })?;
    let canonical = serde_json::to_value(entry).map_err(|source| JournalError::Json {
        context: "encoding canonical journal event",
        source,
    })?;
    let raw_event = raw.get("event").ok_or_else(|| {
        JournalError::InvalidTransition("journal event must be a JSON object".to_owned())
    })?;
    let canonical_event = canonical.get("event").ok_or_else(|| {
        JournalError::InvalidTransition("canonical journal event must be an object".to_owned())
    })?;
    reject_dropped_typed_fields(raw_event, canonical_event, "journal event")
}

fn validate_journal_schema_for_reader(found: u32) -> Result<(), JournalError> {
    if found == LEGACY_SESSION_JOURNAL_SCHEMA_VERSION || found == SESSION_JOURNAL_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(JournalError::UnsupportedSchema {
            found,
            supported: SESSION_JOURNAL_SCHEMA_VERSION,
        })
    }
}

fn reject_schema_regression(previous: Option<u32>, found: u32) -> Result<(), JournalError> {
    if let Some(previous) = previous
        && previous > found
    {
        Err(JournalError::SchemaRegression { previous, found })
    } else {
        Ok(())
    }
}

fn enforce_event_schema_boundary(body: &[u8], schema_version: u32) -> Result<(), JournalError> {
    if schema_version != LEGACY_SESSION_JOURNAL_SCHEMA_VERSION {
        return Ok(());
    }
    let raw =
        serde_json::from_slice::<serde_json::Value>(body).map_err(|source| JournalError::Json {
            context: "checking legacy journal event boundary",
            source,
        })?;
    let event_type = raw
        .get("event")
        .and_then(|event| event.get("type"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            JournalError::InvalidTransition(
                "legacy journal event is missing a string type discriminator".to_owned(),
            )
        })?;
    if LEGACY_EVENT_TYPES.contains(&event_type) {
        Ok(())
    } else {
        Err(JournalError::EventRequiresSchema {
            event_type: event_type.to_owned(),
            found: schema_version,
            required: SESSION_JOURNAL_SCHEMA_VERSION,
        })
    }
}

fn enforce_typed_event_schema_boundary(envelope: &JournalEnvelope) -> Result<(), JournalError> {
    if envelope.schema_version != LEGACY_SESSION_JOURNAL_SCHEMA_VERSION {
        return Ok(());
    }
    let body = serde_json::to_vec(envelope).map_err(|source| JournalError::Json {
        context: "encoding journal event for schema boundary validation",
        source,
    })?;
    enforce_event_schema_boundary(&body, envelope.schema_version)
}

// Frozen at the v4 boundary. New event variants belong to v5+ and must never
// be added here merely to make an old-version fixture decode.
const LEGACY_EVENT_TYPES: &[&str] = &[
    "session_imported",
    "conversation_message_committed",
    "conversation_state_committed",
    "conversation_recovery_checkpoint_committed",
    "conversation_recovery_checkpoint_committed_v2",
    "turn_started",
    "turn_committed",
    "turn_failed",
    "turn_cancelled",
    "stream_started",
    "stream_batch_committed",
    "stream_finished",
    "provider_attempt_prepared",
    "provider_attempt_prepared_v2",
    "provider_attempt_started",
    "provider_attempt_finished",
    "provider_attempt_finished_v2",
    "provider_attempt_not_started",
    "provider_attempt_not_started_v2",
    "tool_intent_recorded",
    "tool_intent_recorded_v2",
    "tool_execution_started",
    "tool_execution_finished",
    "tool_execution_not_started",
    "tool_execution_unknown",
    "tool_execution_resolved",
    "hook_phase_prepared",
    "hook_phase_started",
    "hook_phase_finished",
    "hook_phase_not_started",
    "hook_phase_not_applicable",
    "hook_phase_abandoned_unknown",
    "approval_requested",
    "approval_resolved",
    "budget_reserved",
    "budget_settled",
    "budget_released",
    "budget_authority_committed",
    "checkpoint_committed",
    "child_prepared",
    "child_started",
    "child_finished",
    "child_not_started",
    "child_declared_v2",
    "child_transitioned_v2",
    "delivery_prepared",
    "delivery_started",
    "delivery_not_started",
    "delivery_finished",
];

fn validate_snapshot_binding_position(
    entries: &[JournalEnvelope],
    bindings: &[SnapshotAuthorityBinding],
    binding: &SnapshotAuthorityBinding,
) -> Result<(), JournalError> {
    let position_matches = match binding.cursor {
        None => entries.is_empty() && binding.cursor_checksum == GENESIS_CHECKSUM,
        Some(cursor) => entries.last().is_some_and(|anchor| {
            anchor.seq == cursor
                && anchor.checksum == binding.cursor_checksum
                && anchor.session_id == binding.session_id
        }),
    };
    if !position_matches {
        return Err(JournalError::InvalidTransition(
            "snapshot authority binding must immediately follow its journal anchor".to_owned(),
        ));
    }
    if let Some(previous) = bindings
        .iter()
        .find(|previous| previous.cursor == binding.cursor)
    {
        if previous == binding {
            // An adjacent retry of the exact receipt is idempotent. Position
            // is checked first so an old receipt cannot be replayed later.
            return Ok(());
        }
        return Err(JournalError::InvalidTransition(format!(
            "conflicting snapshot authority binding for cursor {:?}",
            binding.cursor
        )));
    }
    Ok(())
}

pub(super) fn reject_dropped_typed_fields(
    raw: &serde_json::Value,
    canonical: &serde_json::Value,
    layer: &'static str,
) -> Result<(), JournalError> {
    fn is_null(value: &serde_json::Value) -> bool {
        value.is_null()
    }

    fn is_empty_array(value: &serde_json::Value) -> bool {
        value.as_array().is_some_and(Vec::is_empty)
    }

    fn is_empty_object(value: &serde_json::Value) -> bool {
        value.as_object().is_some_and(serde_json::Map::is_empty)
    }

    fn known_omitted_default(
        raw_root: &serde_json::Value,
        layer: &'static str,
        path: &[String],
        value: &serde_json::Value,
    ) -> bool {
        fn durable_child_record_default(path: &[String], value: &serde_json::Value) -> bool {
            match path {
                [parent, field]
                    if parent == "parent"
                        && matches!(
                            field.as_str(),
                            "turn_id"
                                | "parent_child_id"
                                | "workflow_run_id"
                                | "graph_node_id"
                                | "parent_call_id"
                        ) =>
                {
                    is_null(value)
                }
                [policy, field]
                    if policy == "policy_snapshot" && field == "dangerous_activation_id_digest" =>
                {
                    is_null(value)
                }
                [timestamps, field]
                    if timestamps == "timestamps"
                        && matches!(
                            field.as_str(),
                            "queued_at_unix_ms" | "started_at_unix_ms" | "terminal_at_unix_ms"
                        ) =>
                {
                    is_null(value)
                }
                [field]
                    if matches!(
                        field.as_str(),
                        "provider" | "model" | "result" | "delivery_target" | "retry_of"
                    ) =>
                {
                    is_null(value)
                }
                [field] if field == "applied_events" => is_empty_object(value),
                [result, field] if result == "result" && field == "artifact_digests" => {
                    is_empty_array(value)
                }
                _ => false,
            }
        }

        fn durable_child_transition_default(
            raw_root: &serde_json::Value,
            path: &[String],
            value: &serde_json::Value,
        ) -> bool {
            if !matches!(path, [transition, result, field]
                if transition == "transition"
                    && result == "result"
                    && field == "artifact_digests")
                || !is_empty_array(value)
            {
                return false;
            }
            matches!(
                raw_root
                    .get("transition")
                    .and_then(|transition| transition.get("transition"))
                    .and_then(serde_json::Value::as_str),
                Some("succeed" | "fail" | "succeed_after_recovery" | "fail_after_recovery")
            )
        }

        if layer == "journal event" {
            let event_type = raw_root.get("type").and_then(serde_json::Value::as_str);
            if event_type == Some("child_declared_v2")
                && path.first().is_some_and(|field| field == "record")
                && durable_child_record_default(&path[1..], value)
            {
                return true;
            }
            if event_type == Some("child_transitioned_v2")
                && durable_child_transition_default(raw_root, path, value)
            {
                return true;
            }
            return match (event_type, path) {
                (Some("conversation_recovery_checkpoint_committed_v2"), [field])
                    if field == "consumed_hook_phases" =>
                {
                    is_empty_array(value)
                }
                (Some("tool_intent_recorded_v2"), [field])
                    if matches!(
                        field.as_str(),
                        "retry_of" | "effect_receipt" | "pre_hook_phase_id"
                    ) =>
                {
                    is_null(value)
                }
                (Some("hook_phase_prepared"), [field]) if field == "tool_execution_id" => {
                    is_null(value)
                }
                (Some("hook_phase_started"), [field]) if field == "result_digest" => is_null(value),
                (Some("hook_phase_finished"), [field])
                    if matches!(field.as_str(), "result_digest" | "effective_input_digest") =>
                {
                    is_null(value)
                }
                (Some("budget_authority_committed"), [authority, field])
                    if authority == "authority" && field == "provider_reservations" =>
                {
                    is_empty_object(value)
                }
                (Some("budget_authority_committed"), [authority, field])
                    if authority == "authority" && field == "active_turn" =>
                {
                    is_null(value)
                }
                (Some("budget_authority_committed"), [authority, reservations, _, field])
                    if authority == "authority"
                        && reservations == "provider_reservations"
                        && field == "prior_attempt_ids" =>
                {
                    is_empty_array(value)
                }
                _ => false,
            };
        }

        if layer != "session snapshot" {
            return false;
        }
        match path {
            [state, field] if state == "state" && field == "hook_phases" => is_empty_object(value),
            [state, field] if state == "state" && field == "budget_authority" => is_null(value),
            [state, attempts, _, field]
                if state == "state"
                    && attempts == "provider_attempts"
                    && field == "dispatch_id" =>
            {
                is_null(value)
            }
            [state, tools, _, field]
                if state == "state"
                    && tools == "tools"
                    && matches!(
                        field.as_str(),
                        "retry_of" | "effect_receipt" | "pre_hook_phase_id"
                    ) =>
            {
                is_null(value)
            }
            [state, phases, _, field]
                if state == "state" && phases == "hook_phases" && field == "tool_execution_id" =>
            {
                is_null(value)
            }
            [state, phases, phase_id, phase_state, field]
                if state == "state"
                    && phases == "hook_phases"
                    && phase_state == "state"
                    && is_null(value) =>
            {
                let status = raw_root
                    .get("state")
                    .and_then(|state| state.get("hook_phases"))
                    .and_then(|phases| phases.get(phase_id))
                    .and_then(|phase| phase.get("state"))
                    .and_then(|state| state.get("status"))
                    .and_then(serde_json::Value::as_str);
                matches!(
                    (status, field.as_str()),
                    (Some("started" | "finished"), "result_digest")
                        | (Some("finished"), "effective_input_digest")
                )
            }
            [state, children, _, field]
                if state == "state"
                    && children == "children"
                    && matches!(field.as_str(), "durable" | "durable_declaration_digest") =>
            {
                is_null(value)
            }
            [state, authority, field]
                if state == "state"
                    && authority == "budget_authority"
                    && field == "provider_reservations" =>
            {
                is_empty_object(value)
            }
            [state, authority, field]
                if state == "state"
                    && authority == "budget_authority"
                    && field == "active_turn" =>
            {
                is_null(value)
            }
            [state, authority, reservations, _, field]
                if state == "state"
                    && authority == "budget_authority"
                    && reservations == "provider_reservations"
                    && field == "prior_attempt_ids" =>
            {
                is_empty_array(value)
            }
            _ if path.len() >= 5
                && path[0] == "state"
                && path[1] == "children"
                && path[3] == "durable" =>
            {
                durable_child_record_default(&path[4..], value)
            }
            _ => false,
        }
    }

    fn walk(
        raw_root: &serde_json::Value,
        raw: &serde_json::Value,
        canonical: &serde_json::Value,
        layer: &'static str,
        path: &mut Vec<String>,
    ) -> Result<(), JournalError> {
        match raw {
            serde_json::Value::Object(raw_fields) => {
                let canonical_fields = canonical.as_object();
                for (field, raw_value) in raw_fields {
                    path.push(field.clone());
                    let canonical_value =
                        match canonical_fields.and_then(|fields| fields.get(field)) {
                            Some(value) => value,
                            None if known_omitted_default(raw_root, layer, path, raw_value) => {
                                path.pop();
                                continue;
                            }
                            None => {
                                return Err(JournalError::UnknownCriticalField {
                                    layer,
                                    field: path.join("."),
                                });
                            }
                        };
                    walk(raw_root, raw_value, canonical_value, layer, path)?;
                    path.pop();
                }
            }
            serde_json::Value::Array(raw_values) => {
                let canonical_values = canonical.as_array();
                for (index, raw_value) in raw_values.iter().enumerate() {
                    path.push(format!("[{index}]"));
                    let Some(canonical_value) =
                        canonical_values.and_then(|values| values.get(index))
                    else {
                        return Err(JournalError::UnknownCriticalField {
                            layer,
                            field: path.join("."),
                        });
                    };
                    walk(raw_root, raw_value, canonical_value, layer, path)?;
                    path.pop();
                }
            }
            _ => {}
        }
        Ok(())
    }

    walk(raw, raw, canonical, layer, &mut Vec::new())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha256_bytes(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn valid_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[cfg(test)]
mod fault_tests {
    use super::*;
    use wcore_types::spawner::{
        ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent,
        ChildPolicySnapshot, ChildRecoveryState, ChildRequestEvidence, ChildTimestamps,
        ChildWorkspace, ChildWorkspaceMode, DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord,
        DurableChildResult, DurableChildStatus, DurableChildTransition,
    };

    fn raw_frame(magic: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let length = u32::try_from(body.len()).unwrap();
        let mut frame = Vec::new();
        frame.extend_from_slice(magic);
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&(!length).to_be_bytes());
        frame.extend_from_slice(body);
        frame.extend_from_slice(&sha256_bytes(body));
        frame
    }

    fn durable_child_record(result: Option<DurableChildResult>) -> DurableChildRecord {
        DurableChildRecord {
            schema_version: DURABLE_CHILD_SCHEMA_VERSION,
            declaration_id: "declaration-1".to_owned(),
            child_id: ChildId::new("child-1").unwrap(),
            parent: ChildParent {
                session_id: "s1".to_owned(),
                turn_id: None,
                parent_child_id: None,
                workflow_run_id: None,
                graph_node_id: None,
                parent_call_id: None,
            },
            origin: ChildOrigin::Delegate,
            request: ChildRequestEvidence::redacted(sha256_hex(b"request")),
            policy_snapshot: ChildPolicySnapshot {
                contract_version: "execution-policy/v1".to_owned(),
                exact_digest: sha256_hex(b"policy"),
                posture: "smart".to_owned(),
                approvals: "on_request".to_owned(),
                sandbox: "required".to_owned(),
                source: "local".to_owned(),
                managed_floor_active: false,
                dangerous_activation_id_digest: None,
            },
            provider: None,
            model: None,
            workspace: ChildWorkspace {
                mode: ChildWorkspaceMode::Isolated,
                workspace_id: "workspace-1".to_owned(),
            },
            status: if result.is_some() {
                DurableChildStatus::Succeeded
            } else {
                DurableChildStatus::Prepared
            },
            desired_state: ChildDesiredState::Run,
            recovery: ChildRecoveryState::Clean,
            revision: if result.is_some() { 1 } else { 0 },
            timestamps: ChildTimestamps {
                created_at_unix_ms: 1,
                updated_at_unix_ms: 1,
                queued_at_unix_ms: None,
                started_at_unix_ms: None,
                terminal_at_unix_ms: None,
            },
            result,
            delivery_target: None,
            delivery_state: ChildDeliveryState::NotRequired,
            attempt: 1,
            retry_of: None,
            applied_events: std::collections::BTreeMap::new(),
        }
    }

    fn inject_durable_child_defaults(value: &mut serde_json::Value, result_is_none: bool) {
        value["parent"]["turn_id"] = serde_json::Value::Null;
        value["parent"]["parent_child_id"] = serde_json::Value::Null;
        value["parent"]["workflow_run_id"] = serde_json::Value::Null;
        value["parent"]["graph_node_id"] = serde_json::Value::Null;
        value["parent"]["parent_call_id"] = serde_json::Value::Null;
        value["policy_snapshot"]["dangerous_activation_id_digest"] = serde_json::Value::Null;
        value["provider"] = serde_json::Value::Null;
        value["model"] = serde_json::Value::Null;
        value["timestamps"]["queued_at_unix_ms"] = serde_json::Value::Null;
        value["timestamps"]["started_at_unix_ms"] = serde_json::Value::Null;
        value["timestamps"]["terminal_at_unix_ms"] = serde_json::Value::Null;
        if result_is_none {
            value["result"] = serde_json::Value::Null;
        } else {
            value["result"]["artifact_digests"] = serde_json::json!([]);
        }
        value["delivery_target"] = serde_json::Value::Null;
        value["retry_of"] = serde_json::Value::Null;
        value["applied_events"] = serde_json::json!({});
    }

    fn snapshot_with_hook_state(state: HookPhaseState) -> SessionSnapshot {
        let mut reduced = ReducedSessionState::default();
        reduced.hook_phases.insert(
            "hook-1".to_owned(),
            HookPhaseExecutionState {
                lifecycle_version: HOOK_PHASE_LIFECYCLE_VERSION,
                turn_id: "turn-1".to_owned(),
                provider_call_id: "call-1".to_owned(),
                ordinal: 0,
                phase: ToolHookPhase::PreToolUse,
                tool_execution_id: None,
                input_digest: sha256_hex(b"input"),
                hook_authority_digest: sha256_hex(b"authority"),
                hook_manifest_digest: sha256_hex(b"manifest"),
                hook_slots: Vec::new(),
                state,
            },
        );
        SessionSnapshot::new("s1", reduced).unwrap()
    }

    #[test]
    fn unknown_envelope_event_and_binding_fields_fail_closed() {
        let path = Path::new("strict-fields.journal");
        let envelope = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "hello".to_owned(),
            },
        )
        .unwrap();

        let mut unknown_envelope = serde_json::to_value(&envelope).unwrap();
        unknown_envelope["future_authority"] = serde_json::json!(true);
        let body = serde_json::to_vec(&unknown_envelope).unwrap();
        assert!(matches!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body)),
            Err(JournalError::CorruptFrame { .. })
        ));

        let mut unknown_event = serde_json::to_value(&envelope).unwrap();
        unknown_event["event"]["future_authority"] = serde_json::json!(true);
        let body = serde_json::to_vec(&unknown_event).unwrap();
        assert!(matches!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body)),
            Err(JournalError::UnknownCriticalField {
                layer: "journal event",
                ..
            })
        ));

        let snapshot = SessionSnapshot::new("s1", ReducedSessionState::default()).unwrap();
        let mut unknown_binding =
            serde_json::to_value(SnapshotAuthorityBinding::new(&snapshot)).unwrap();
        unknown_binding["future_authority"] = serde_json::json!(true);
        let body = serde_json::to_vec(&unknown_binding).unwrap();
        assert!(matches!(
            parse_complete_frames(path, &raw_frame(SNAPSHOT_AUTHORITY_MAGIC, &body)),
            Err(JournalError::CorruptFrame { .. })
        ));

        let nested = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::BudgetReserved {
                event_id: "budget-event".to_owned(),
                reservation_id: "reservation".to_owned(),
                owner: BudgetOwner::Session,
                purpose: BudgetPurpose::Conversation,
                amount: BudgetAmount {
                    value: 1,
                    unit: BudgetUnit::Tokens,
                },
            },
        )
        .unwrap();
        let mut unknown_nested_event = serde_json::to_value(&nested).unwrap();
        unknown_nested_event["event"]["amount"]["future_authority"] = serde_json::json!(true);
        let body = serde_json::to_vec(&unknown_nested_event).unwrap();
        assert!(matches!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body)),
            Err(JournalError::UnknownCriticalField {
                layer: "journal event",
                ..
            })
        ));

        let opaque = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::CheckpointCommitted {
                checkpoint_id: "checkpoint".to_owned(),
                purpose: CheckpointPurpose::Recovery,
                origin: CheckpointOrigin::Session,
                state_digest: "digest".to_owned(),
                state: serde_json::json!({"future_payload_field": {"must": "survive"}}),
            },
        )
        .unwrap();
        let body = serde_json::to_vec(&opaque).unwrap();
        let parsed = parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body)).unwrap();
        assert_eq!(parsed.entries, vec![opaque]);
    }

    #[test]
    fn known_explicit_event_defaults_are_wire_compatible_but_unknowns_fail_closed() {
        let path = Path::new("explicit-event-defaults.journal");
        let envelope = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::ToolIntentRecordedV2 {
                tool_execution_id: "tool-1".to_owned(),
                idempotency_key: "key-1".to_owned(),
                retry_of: None,
                provider_call_id: "call-1".to_owned(),
                turn_id: "turn-1".to_owned(),
                ordinal: 0,
                tool: "read".to_owned(),
                requested_input: StoredToolInput::redacted("requested"),
                requested_input_digest: "requested".to_owned(),
                effective_input: StoredToolInput::redacted("effective"),
                effective_input_digest: "effective".to_owned(),
                effect_contract: wcore_types::tool::ToolEffectContract::default(),
                effect_receipt: None,
                pre_hook_phase_id: None,
            },
        )
        .unwrap();
        let mut explicit = serde_json::to_value(&envelope).unwrap();
        explicit["event"]["retry_of"] = serde_json::Value::Null;
        explicit["event"]["effect_receipt"] = serde_json::Value::Null;
        explicit["event"]["pre_hook_phase_id"] = serde_json::Value::Null;
        let body = serde_json::to_vec(&explicit).unwrap();
        assert_eq!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body))
                .unwrap()
                .entries,
            vec![envelope]
        );

        explicit["event"]["future_authority"] = serde_json::Value::Null;
        let body = serde_json::to_vec(&explicit).unwrap();
        assert!(matches!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body)),
            Err(JournalError::UnknownCriticalField {
                layer: "journal event",
                field,
            }) if field == "future_authority"
        ));
    }

    #[test]
    fn known_explicit_snapshot_defaults_are_wire_compatible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("explicit-defaults.snapshot");
        let snapshot = SessionSnapshot::new("s1", ReducedSessionState::default()).unwrap();
        let mut explicit = serde_json::to_value(&snapshot).unwrap();
        explicit["state"]["hook_phases"] = serde_json::json!({});
        explicit["state"]["budget_authority"] = serde_json::Value::Null;
        snapshot::write_private_snapshot_fixture(&path, &serde_json::to_vec(&explicit).unwrap())
            .unwrap();

        assert_eq!(load_snapshot(&path).unwrap(), snapshot);
    }

    #[test]
    fn hook_snapshot_defaults_are_status_aware() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook-defaults.snapshot");

        for field in ["result_digest", "effective_input_digest"] {
            let snapshot = snapshot_with_hook_state(HookPhaseState::Prepared);
            let mut injected = serde_json::to_value(snapshot).unwrap();
            injected["state"]["hook_phases"]["hook-1"]["state"][field] = serde_json::Value::Null;
            snapshot::write_private_snapshot_fixture(
                &path,
                &serde_json::to_vec(&injected).unwrap(),
            )
            .unwrap();
            assert!(matches!(
                load_snapshot(&path),
                Err(JournalError::UnknownCriticalField {
                    layer: "session snapshot",
                    field: found,
                }) if found == format!("state.hook_phases.hook-1.state.{field}")
            ));
        }

        let started = snapshot_with_hook_state(HookPhaseState::Started {
            result_digest: None,
        });
        let mut explicit_started = serde_json::to_value(&started).unwrap();
        explicit_started["state"]["hook_phases"]["hook-1"]["state"]["result_digest"] =
            serde_json::Value::Null;
        snapshot::write_private_snapshot_fixture(
            &path,
            &serde_json::to_vec(&explicit_started).unwrap(),
        )
        .unwrap();
        assert_eq!(load_snapshot(&path).unwrap(), started);

        explicit_started["state"]["hook_phases"]["hook-1"]["state"]["effective_input_digest"] =
            serde_json::Value::Null;
        snapshot::write_private_snapshot_fixture(
            &path,
            &serde_json::to_vec(&explicit_started).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            load_snapshot(&path),
            Err(JournalError::UnknownCriticalField { field, .. })
                if field == "state.hook_phases.hook-1.state.effective_input_digest"
        ));

        let finished = snapshot_with_hook_state(HookPhaseState::Finished {
            result_digest: None,
            effective_input_digest: None,
            outcome_digest: sha256_hex(b"outcome"),
            slot_receipts_digest: sha256_hex(b"receipts"),
            slot_receipts: Vec::new(),
        });
        let mut explicit_finished = serde_json::to_value(&finished).unwrap();
        explicit_finished["state"]["hook_phases"]["hook-1"]["state"]["result_digest"] =
            serde_json::Value::Null;
        explicit_finished["state"]["hook_phases"]["hook-1"]["state"]["effective_input_digest"] =
            serde_json::Value::Null;
        snapshot::write_private_snapshot_fixture(
            &path,
            &serde_json::to_vec(&explicit_finished).unwrap(),
        )
        .unwrap();
        assert_eq!(load_snapshot(&path).unwrap(), finished);
    }

    #[test]
    fn durable_child_nested_defaults_are_wire_compatible() {
        let path = Path::new("durable-child-defaults.journal");
        let declaration = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::ChildDeclaredV2 {
                record: durable_child_record(None),
            },
        )
        .unwrap();
        let mut explicit_declaration = serde_json::to_value(&declaration).unwrap();
        inject_durable_child_defaults(&mut explicit_declaration["event"]["record"], true);
        let body = serde_json::to_vec(&explicit_declaration).unwrap();
        assert_eq!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body))
                .unwrap()
                .entries,
            vec![declaration]
        );

        let result = DurableChildResult {
            exact_digest: sha256_hex(b"result"),
            turns: 1,
            input_tokens: 2,
            output_tokens: 3,
            artifact_digests: Vec::new(),
        };
        let transition = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::ChildTransitionedV2 {
                child_id: ChildId::new("child-1").unwrap(),
                event_id: "event-1".to_owned(),
                expected_revision: 0,
                at_unix_ms: 2,
                transition: DurableChildTransition::Succeed {
                    result: result.clone(),
                },
            },
        )
        .unwrap();
        let mut explicit_transition = serde_json::to_value(&transition).unwrap();
        explicit_transition["event"]["transition"]["result"]["artifact_digests"] =
            serde_json::json!([]);
        let body = serde_json::to_vec(&explicit_transition).unwrap();
        assert_eq!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body))
                .unwrap()
                .entries,
            vec![transition]
        );

        explicit_transition["event"]["transition"]["future_authority"] = serde_json::Value::Null;
        let body = serde_json::to_vec(&explicit_transition).unwrap();
        assert!(matches!(
            parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body)),
            Err(JournalError::CorruptFrame { .. }) | Err(JournalError::UnknownCriticalField { .. })
        ));

        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("durable-child-defaults.snapshot");
        let mut reduced = ReducedSessionState::default();
        reduced.children.insert(
            "child-1".to_owned(),
            ChildState {
                turn_id: String::new(),
                request: serde_json::json!({"exact_digest": sha256_hex(b"request")}),
                result: None,
                not_started_reason: None,
                effect: ExternalEffectState::Completed {
                    outcome: CompletionOutcome::Succeeded,
                },
                durable: Some(durable_child_record(Some(result))),
                durable_declaration_digest: Some(sha256_hex(b"declaration")),
            },
        );
        let snapshot = SessionSnapshot::new("s1", reduced).unwrap();
        let mut explicit_snapshot = serde_json::to_value(&snapshot).unwrap();
        inject_durable_child_defaults(
            &mut explicit_snapshot["state"]["children"]["child-1"]["durable"],
            false,
        );
        snapshot::write_private_snapshot_fixture(
            &snapshot_path,
            &serde_json::to_vec(&explicit_snapshot).unwrap(),
        )
        .unwrap();
        assert_eq!(load_snapshot(&snapshot_path).unwrap(), snapshot);
    }

    #[test]
    fn legacy_and_current_journal_schemas_have_an_explicit_mixed_chain_boundary() {
        assert_eq!(SESSION_JOURNAL_SCHEMA_VERSION, 5);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed-schema.journal");
        let mut legacy = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "legacy".to_owned(),
            },
        )
        .unwrap();
        legacy.schema_version = 4;
        legacy.checksum = legacy.computed_checksum().unwrap();
        let body = serde_json::to_vec(&legacy).unwrap();
        std::fs::write(&path, raw_frame(FRAME_MAGIC, &body)).unwrap();

        assert_eq!(SessionJournal::replay(&path).unwrap(), vec![legacy]);
        let journal = SessionJournal::open(&path, "s1").unwrap();
        journal
            .append(SessionEvent::TurnCancelled {
                turn_id: "t0".to_owned(),
            })
            .unwrap();
        drop(journal);
        assert_eq!(
            SessionJournal::replay(&path)
                .unwrap()
                .iter()
                .map(|entry| entry.schema_version)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
    }

    #[test]
    fn journal_schema_cannot_regress_from_current_to_legacy() {
        let path = Path::new("schema-regression.journal");
        let current = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "current".to_owned(),
            },
        )
        .unwrap();
        let mut legacy = JournalEnvelope::create(
            "s1".to_owned(),
            1,
            current.checksum.clone(),
            SessionEvent::TurnCancelled {
                turn_id: "t0".to_owned(),
            },
        )
        .unwrap();
        legacy.schema_version = LEGACY_SESSION_JOURNAL_SCHEMA_VERSION;
        legacy.checksum = legacy.computed_checksum().unwrap();

        assert!(matches!(
            verify_chain(&[current.clone(), legacy.clone()]),
            Err(JournalError::SchemaRegression {
                previous: SESSION_JOURNAL_SCHEMA_VERSION,
                found: LEGACY_SESSION_JOURNAL_SCHEMA_VERSION,
            })
        ));
        let mut bytes = raw_frame(FRAME_MAGIC, &serde_json::to_vec(&current).unwrap());
        bytes.extend_from_slice(&raw_frame(
            FRAME_MAGIC,
            &serde_json::to_vec(&legacy).unwrap(),
        ));
        assert!(matches!(
            parse_complete_frames(path, &bytes),
            Err(JournalError::SchemaRegression { .. })
        ));
        assert!(matches!(
            replay_state(&[current, legacy]),
            Err(JournalError::SchemaRegression {
                previous: SESSION_JOURNAL_SCHEMA_VERSION,
                found: LEGACY_SESSION_JOURNAL_SCHEMA_VERSION,
            })
        ));

        let mut legacy = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "legacy".to_owned(),
            },
        )
        .unwrap();
        legacy.schema_version = LEGACY_SESSION_JOURNAL_SCHEMA_VERSION;
        legacy.checksum = legacy.computed_checksum().unwrap();
        let current = JournalEnvelope::create(
            "s1".to_owned(),
            1,
            legacy.checksum.clone(),
            SessionEvent::TurnCancelled {
                turn_id: "t0".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(replay_state(&[legacy, current]).unwrap().last_seq, Some(1));
    }

    #[test]
    fn current_genesis_snapshot_cannot_seed_a_legacy_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("genesis-snapshot.journal");
        let journal = SessionJournal::open(&path, "s1").unwrap();
        journal.publish_snapshot().unwrap();
        drop(journal);

        let mut legacy = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "legacy suffix".to_owned(),
            },
        )
        .unwrap();
        legacy.schema_version = LEGACY_SESSION_JOURNAL_SCHEMA_VERSION;
        legacy.checksum = legacy.computed_checksum().unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&raw_frame(
                FRAME_MAGIC,
                &serde_json::to_vec(&legacy).unwrap(),
            ))
            .unwrap();

        assert!(matches!(
            SessionJournal::recovered_state(&path),
            Err(JournalError::SchemaRegression {
                previous: SESSION_JOURNAL_SCHEMA_VERSION,
                found: LEGACY_SESSION_JOURNAL_SCHEMA_VERSION,
            })
        ));
        assert!(matches!(
            SessionJournal::replay(&path),
            Err(JournalError::SchemaRegression {
                previous: SESSION_JOURNAL_SCHEMA_VERSION,
                found: LEGACY_SESSION_JOURNAL_SCHEMA_VERSION,
            })
        ));
    }

    #[test]
    fn offline_snapshot_replay_preserves_forward_only_schema_history() {
        let current = SessionSnapshot::new("s1", ReducedSessionState::default()).unwrap();
        let mut legacy_suffix = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "legacy".to_owned(),
            },
        )
        .unwrap();
        legacy_suffix.schema_version = LEGACY_SESSION_JOURNAL_SCHEMA_VERSION;
        legacy_suffix.checksum = legacy_suffix.computed_checksum().unwrap();
        assert!(matches!(
            replay_from_snapshot(&current, &[legacy_suffix]),
            Err(JournalError::SchemaRegression {
                previous: SESSION_SNAPSHOT_SCHEMA_VERSION,
                found: LEGACY_SESSION_JOURNAL_SCHEMA_VERSION,
            })
        ));

        let mut legacy = current;
        legacy.schema_version = LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION;
        let current_suffix = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "current".to_owned(),
            },
        )
        .unwrap();
        let state = replay_from_snapshot(&legacy, &[current_suffix]).unwrap();
        assert_eq!(state.last_seq, Some(0));
    }

    #[test]
    #[allow(deprecated)]
    fn compatibility_snapshot_writer_cannot_mint_recovery_authority() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("unbound.journal");
        let snapshot_path = snapshot_path_for(&journal_path);
        let snapshot = SessionSnapshot::new("s1", ReducedSessionState::default()).unwrap();
        write_snapshot(&snapshot_path, &snapshot).unwrap();
        assert_eq!(load_snapshot(&snapshot_path).unwrap(), snapshot);
        assert!(matches!(
            SessionJournal::recovered_state(&journal_path),
            Err(JournalError::SnapshotAuthorityMismatch)
                | Err(JournalError::SnapshotJournalMismatch(_))
        ));
    }

    #[test]
    fn legacy_schema_rejects_post_boundary_event_before_event_decode() {
        let path = Path::new("legacy-future-event.journal");
        let body = serde_json::to_vec(&serde_json::json!({
            "schema_version": 4,
            "session_id": "s1",
            "seq": 0,
            "previous_checksum": GENESIS_CHECKSUM,
            "event": {
                "type": "child_transaction_receipt_committed",
                "future_shape": true,
            },
            "checksum": "irrelevant-before-event-decode",
        }))
        .unwrap();
        let error = match parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body)) {
            Ok(_) => panic!("legacy post-boundary event was accepted"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("requires journal schema 5"),
            "legacy post-boundary event was not rejected by the schema boundary: {error}"
        );
    }

    #[test]
    fn snapshot_binding_must_follow_its_anchor_and_be_unique() {
        let path = Path::new("binding-order.journal");
        let envelope = JournalEnvelope::create(
            "s1".to_owned(),
            0,
            GENESIS_CHECKSUM.to_owned(),
            SessionEvent::TurnStarted {
                turn_id: "t0".to_owned(),
                user_message: "hello".to_owned(),
            },
        )
        .unwrap();
        let snapshot = SessionSnapshot::new(
            "s1",
            reduce(ReducedSessionState::default(), &envelope).unwrap(),
        )
        .unwrap();
        let binding = SnapshotAuthorityBinding::new(&snapshot);
        let envelope_frame = raw_frame(FRAME_MAGIC, &serde_json::to_vec(&envelope).unwrap());
        let binding_frame = raw_frame(
            SNAPSHOT_AUTHORITY_MAGIC,
            &serde_json::to_vec(&binding).unwrap(),
        );

        let mut before_anchor = binding_frame.clone();
        before_anchor.extend_from_slice(&envelope_frame);
        assert!(matches!(
            parse_complete_frames(path, &before_anchor),
            Err(JournalError::InvalidTransition(message))
                if message.contains("must immediately follow its journal anchor")
        ));

        let mut duplicate = envelope_frame.clone();
        duplicate.extend_from_slice(&binding_frame);
        duplicate.extend_from_slice(&binding_frame);
        let parsed = parse_complete_frames(path, &duplicate).unwrap();
        assert_eq!(parsed.bindings, vec![binding.clone(), binding.clone()]);

        let mut conflicting_binding = binding;
        conflicting_binding.state_digest = "f".repeat(64);
        let conflicting_frame = raw_frame(
            SNAPSHOT_AUTHORITY_MAGIC,
            &serde_json::to_vec(&conflicting_binding).unwrap(),
        );
        let mut conflicting = envelope_frame;
        conflicting.extend_from_slice(&binding_frame);
        conflicting.extend_from_slice(&conflicting_frame);
        assert!(matches!(
            parse_complete_frames(path, &conflicting),
            Err(JournalError::InvalidTransition(message))
                if message.contains("conflicting snapshot authority binding")
        ));

        let later = JournalEnvelope::create(
            "s1".to_owned(),
            1,
            envelope.checksum.clone(),
            SessionEvent::TurnCancelled {
                turn_id: "t0".to_owned(),
            },
        )
        .unwrap();
        let mut replayed_later = raw_frame(FRAME_MAGIC, &serde_json::to_vec(&envelope).unwrap());
        replayed_later.extend_from_slice(&binding_frame);
        replayed_later.extend_from_slice(&raw_frame(
            FRAME_MAGIC,
            &serde_json::to_vec(&later).unwrap(),
        ));
        replayed_later.extend_from_slice(&binding_frame);
        assert!(matches!(
            parse_complete_frames(path, &replayed_later),
            Err(JournalError::InvalidTransition(message))
                if message.contains("must immediately follow its journal anchor")
        ));
    }

    fn write_valid_journal(path: &Path, turn_id: &str) {
        let journal = SessionJournal::open(path, "s1").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: turn_id.to_owned(),
                user_message: turn_id.to_owned(),
            })
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn readonly_recovery_rejects_symlink_to_valid_journal() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("valid.journal");
        let alias = dir.path().join("alias.journal");
        write_valid_journal(&target, "target-turn");
        symlink(&target, &alias).unwrap();

        assert!(matches!(
            SessionJournal::replay(&alias),
            Err(JournalError::SymbolicLink { path }) if path == alias
        ));
        assert!(matches!(
            SessionJournal::recovered_state(&alias),
            Err(JournalError::SymbolicLink { path }) if path == alias
        ));
    }

    #[cfg(windows)]
    #[test]
    fn readonly_recovery_rejects_symlink_to_valid_journal() {
        use std::os::windows::fs::symlink_file;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("valid.journal");
        let alias = dir.path().join("alias.journal");
        write_valid_journal(&target, "target-turn");
        symlink_file(&target, &alias)
            .unwrap_or_else(|error| panic!("Windows symlink fixture is required: {error}"));

        assert!(matches!(
            SessionJournal::replay(&alias),
            Err(JournalError::SymbolicLink { path }) if path == alias
        ));
        assert!(matches!(
            SessionJournal::recovered_state(&alias),
            Err(JournalError::SymbolicLink { path }) if path == alias
        ));
    }

    #[test]
    fn readonly_recovery_rejects_hard_link_to_valid_journal() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("valid.journal");
        let alias = dir.path().join("alias.journal");
        write_valid_journal(&target, "target-turn");
        std::fs::hard_link(&target, &alias).unwrap();

        assert!(matches!(
            SessionJournal::replay(&alias),
            Err(JournalError::MultipleLinks { path }) if path == alias
        ));
        assert!(matches!(
            SessionJournal::recovered_state(&alias),
            Err(JournalError::MultipleLinks { path }) if path == alias
        ));
    }

    fn assert_readonly_recovery_rejects_path_swap(
        directory: &Path,
        stem: &str,
        read: impl FnOnce(&Path) -> Result<(), JournalError>,
    ) {
        let canonical = directory.join(format!("{stem}-canonical.journal"));
        let displaced = directory.join(format!("{stem}-displaced.journal"));
        let replacement = directory.join(format!("{stem}-replacement.journal"));
        write_valid_journal(&canonical, "original-turn");
        write_valid_journal(&replacement, "replacement-turn");
        let replacement_for_hook = replacement.clone();
        set_after_journal_read_hook(move |path| {
            std::fs::rename(path, displaced).unwrap();
            std::fs::rename(replacement_for_hook, path).unwrap();
        });

        assert!(matches!(
            read(&canonical),
            Err(JournalError::PathIdentityMismatch { path }) if path == canonical
        ));
    }

    #[test]
    fn readonly_recovery_rejects_valid_journal_path_swap_after_read() {
        let dir = tempfile::tempdir().unwrap();
        assert_readonly_recovery_rejects_path_swap(dir.path(), "replay", |path| {
            SessionJournal::replay(path).map(|_| ())
        });
        assert_readonly_recovery_rejects_path_swap(dir.path(), "state", |path| {
            SessionJournal::recovered_state(path).map(|_| ())
        });
    }

    #[test]
    fn canonical_path_replacement_cannot_acknowledge_append_or_snapshot() {
        fn replace_open_journal(
            dir: &Path,
            stem: &str,
        ) -> (SessionJournal, SessionJournal, PathBuf, ReducedSessionState) {
            let target_path = dir.join(format!("{stem}-target.journal"));
            let displaced_path = dir.join(format!("{stem}-displaced.journal"));
            let replacement_path = dir.join(format!("{stem}-replacement.journal"));
            let target = SessionJournal::open(&target_path, "s1").unwrap();
            target
                .append(SessionEvent::TurnStarted {
                    turn_id: "target-turn".to_owned(),
                    user_message: "target".to_owned(),
                })
                .unwrap();
            let replacement = SessionJournal::open(&replacement_path, "s1").unwrap();
            replacement
                .append(SessionEvent::TurnStarted {
                    turn_id: "replacement-turn".to_owned(),
                    user_message: "replacement".to_owned(),
                })
                .unwrap();
            let replacement_state = replacement.state().unwrap();
            std::fs::rename(&target_path, displaced_path).unwrap();
            std::fs::rename(&replacement_path, &target_path).unwrap();
            (target, replacement, target_path, replacement_state)
        }

        let dir = tempfile::tempdir().unwrap();
        let (target, replacement, target_path, replacement_state) =
            replace_open_journal(dir.path(), "append");
        assert!(matches!(
            target.append(SessionEvent::TurnCancelled {
                turn_id: "target-turn".to_owned(),
            }),
            Err(JournalError::PathIdentityMismatch { .. })
        ));
        drop(target);
        drop(replacement);
        assert_eq!(
            SessionJournal::recovered_state(&target_path).unwrap(),
            replacement_state
        );

        let (target, replacement, target_path, replacement_state) =
            replace_open_journal(dir.path(), "snapshot");
        assert!(matches!(
            target.publish_snapshot(),
            Err(JournalError::PathIdentityMismatch { .. })
        ));
        drop(target);
        drop(replacement);
        assert_eq!(
            SessionJournal::recovered_state(&target_path).unwrap(),
            replacement_state
        );
        assert!(!snapshot_path_for(&target_path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn journal_open_and_identity_guards_never_follow_symlink_aliases() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let dir = tempfile::tempdir().unwrap();
        let protected = dir.path().join("protected");
        let journal_path = dir.path().join("session.journal");
        std::fs::write(&protected, b"do not mutate").unwrap();
        std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&protected, &journal_path).unwrap();

        assert!(matches!(
            JournalWriter::open(journal_path.clone(), "s1".to_owned()),
            Err(JournalError::SymbolicLink { .. })
        ));
        assert_eq!(std::fs::read(&protected).unwrap(), b"do not mutate");
        assert_eq!(
            std::fs::metadata(&protected).unwrap().permissions().mode() & 0o777,
            0o640
        );

        std::fs::remove_file(&journal_path).unwrap();
        let journal = SessionJournal::open(&journal_path, "s1").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".to_owned(),
                user_message: "held".to_owned(),
            })
            .unwrap();
        let displaced = dir.path().join("displaced.journal");
        std::fs::rename(&journal_path, &displaced).unwrap();
        symlink(&displaced, &journal_path).unwrap();
        let before = std::fs::read(&displaced).unwrap();

        assert!(matches!(
            journal.publish_snapshot(),
            Err(JournalError::SymbolicLink { .. })
        ));
        assert_eq!(std::fs::read(&displaced).unwrap(), before);
        assert!(!snapshot_path_for(&journal_path).exists());
    }

    #[cfg(unix)]
    #[test]
    fn lease_owner_read_never_follows_replaced_sentinel() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let journal = SessionJournal::open(&journal_path, "s1").unwrap();
        let sentinel = lease::lease_path(&journal_path);
        let displaced = dir.path().join("displaced.writer.lock");
        let protected = dir.path().join("protected");
        std::fs::write(&protected, b"not lease metadata").unwrap();
        std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::rename(&sentinel, &displaced).unwrap();
        symlink(&protected, &sentinel).unwrap();

        assert!(matches!(
            SessionJournal::lease_owner(&journal_path),
            Err(JournalError::SymbolicLink { .. })
        ));
        assert_eq!(std::fs::read(&protected).unwrap(), b"not lease metadata");
        assert_eq!(
            std::fs::metadata(&protected).unwrap().permissions().mode() & 0o777,
            0o640
        );
        drop(journal);
    }

    #[cfg(unix)]
    #[test]
    fn writer_lease_revalidates_path_after_lock_before_mutation() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let displaced = dir.path().join("displaced.writer.lock");
        let replacement = dir.path().join("replacement.writer.lock");
        lease::set_after_lease_lock_hook({
            let displaced = displaced.clone();
            let replacement = replacement.clone();
            move |sentinel| {
                std::fs::rename(sentinel, &displaced).unwrap();
                std::fs::write(&replacement, b"replacement must survive").unwrap();
                std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o640))
                    .unwrap();
                std::fs::rename(&replacement, sentinel).unwrap();
            }
        });

        assert!(matches!(
            SessionJournal::open(&journal_path, "session"),
            Err(JournalError::PathIdentityMismatch { .. })
        ));
        let sentinel = lease::lease_path(&journal_path);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"replacement must survive"
        );
        assert_eq!(
            std::fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn lease_owner_requires_strict_metadata_and_an_active_lock() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let journal = SessionJournal::open(&journal_path, "session").unwrap();
        let owner = SessionJournal::lease_owner(&journal_path).unwrap();
        assert_eq!(owner.session_id, "session");

        let sentinel = lease::lease_path(&journal_path);
        let displaced = dir.path().join("displaced.writer.lock");
        std::fs::rename(&sentinel, &displaced).unwrap();
        let mut value = serde_json::to_value(&owner).unwrap();
        value["untrusted"] = serde_json::json!(true);
        std::fs::write(&sentinel, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            SessionJournal::lease_owner(&journal_path),
            Err(JournalError::Json { .. })
        ));

        std::fs::write(&sentinel, serde_json::to_vec(&owner).unwrap()).unwrap();
        assert!(matches!(
            SessionJournal::lease_owner(&journal_path),
            Err(JournalError::InvalidTransition(message)) if message.contains("not actively owned")
        ));
        drop(journal);
    }

    #[cfg(unix)]
    #[test]
    fn committed_authority_cannot_mix_in_a_pathname_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let displaced_path = dir.path().join("displaced.journal");
        let replacement_path = dir.path().join("replacement.journal");

        let journal = SessionJournal::open(&journal_path, "session").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "original-turn".into(),
                user_message: "original".into(),
            })
            .unwrap();

        let replacement = SessionJournal::open(&replacement_path, "session").unwrap();
        let forged = replacement
            .append(SessionEvent::TurnStarted {
                turn_id: "forged-turn".into(),
                user_message: "forged".into(),
            })
            .unwrap();
        drop(replacement);

        std::fs::rename(&journal_path, &displaced_path).unwrap();
        std::fs::rename(&replacement_path, &journal_path).unwrap();

        assert!(matches!(
            journal.committed_authority(),
            Err(JournalError::PathIdentityMismatch { .. })
        ));
        assert!(matches!(
            journal.committed_authority(),
            Err(JournalError::WriterFaulted)
        ));
        assert_eq!(SessionJournal::replay(&journal_path).unwrap(), vec![forged]);
    }

    #[test]
    fn committed_authority_rejects_a_state_head_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".into(),
                user_message: "hello".into(),
            })
            .unwrap();
        journal.inner.lock().unwrap().state.last_checksum = GENESIS_CHECKSUM.to_owned();

        assert!(matches!(
            journal.committed_authority(),
            Err(JournalError::JournalAuthorityMismatch(_))
        ));
        assert!(matches!(
            journal.append(SessionEvent::TurnCancelled {
                turn_id: "turn".into()
            }),
            Err(JournalError::WriterFaulted)
        ));
    }

    #[test]
    fn effect_checkpoint_round_trips_and_rejects_digest_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);

        journal.store_effect_checkpoint(&digest, contents).unwrap();
        assert_eq!(journal.load_effect_checkpoint(&digest).unwrap(), contents);
        assert!(matches!(
            journal.store_effect_checkpoint(&digest, b"different"),
            Err(JournalError::InvalidTransition(_))
        ));
    }

    #[test]
    fn effect_checkpoint_rejects_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let journal = SessionJournal::open(&journal_path, "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();

        let checkpoint = journal.effect_checkpoint_path(&digest).unwrap();
        std::fs::write(&checkpoint, b"tampered").unwrap();
        assert!(matches!(
            journal.load_effect_checkpoint(&digest),
            Err(JournalError::InvalidTransition(_))
        ));
    }

    #[test]
    fn effect_checkpoint_rejects_oversized_file_before_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();

        let checkpoint = journal.effect_checkpoint_path(&digest).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&checkpoint)
            .unwrap()
            .set_len(MAX_EFFECT_CHECKPOINT_BYTES + 1)
            .unwrap();
        assert!(matches!(
            journal.load_effect_checkpoint(&digest),
            Err(JournalError::InvalidTransition(message)) if message.contains("exceeds")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn effect_checkpoint_repairs_crash_after_publication_link() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();

        let checkpoint = journal.effect_checkpoint_path(&digest).unwrap();
        let crash_link = checkpoint
            .parent()
            .unwrap()
            .join(format!(".{digest}.crash.tmp"));
        std::fs::hard_link(&checkpoint, &crash_link).unwrap();

        assert_eq!(journal.load_effect_checkpoint(&digest).unwrap(), contents);
        assert!(!crash_link.exists());
    }

    #[test]
    fn session_retirement_removes_private_effect_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let journal = SessionJournal::open(&journal_path, "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();
        let checkpoint_directory = effect_checkpoint_directory_for(&journal_path).unwrap();
        assert!(checkpoint_directory.exists());
        drop(journal);

        let lease = SessionJournal::acquire_storage_lease(&journal_path, "session").unwrap();
        lease
            .remove_files(
                &dir.path().join("2026-07-19_session.json"),
                &dir.path().join("2026-07-19_session.wal"),
            )
            .unwrap();
        assert!(!checkpoint_directory.exists());
    }

    #[cfg(unix)]
    #[test]
    fn session_retirement_never_deletes_a_replacement_journal_or_collateral() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let session_path = dir.path().join("2026-07-19_session.json");
        let wal_path = dir.path().join("2026-07-19_session.wal");
        let displaced_path = dir.path().join("displaced.journal");
        let journal = SessionJournal::open(&journal_path, "session").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".to_owned(),
                user_message: "held".to_owned(),
            })
            .unwrap();
        journal.publish_snapshot().unwrap();
        let checkpoint = b"private preimage";
        journal
            .store_effect_checkpoint(&sha256_hex(checkpoint), checkpoint)
            .unwrap();
        std::fs::write(&session_path, b"session").unwrap();
        std::fs::write(&wal_path, b"wal").unwrap();
        let snapshot_path = snapshot_path_for(&journal_path);
        let checkpoint_directory = effect_checkpoint_directory_for(&journal_path).unwrap();
        drop(journal);

        let lease = SessionJournal::acquire_storage_lease(&journal_path, "session").unwrap();
        std::fs::rename(&journal_path, &displaced_path).unwrap();
        std::fs::write(&journal_path, b"replacement must survive").unwrap();

        assert!(matches!(
            lease.remove_files(&session_path, &wal_path),
            Err(JournalError::PathIdentityMismatch { .. })
        ));
        assert_eq!(
            std::fs::read(&journal_path).unwrap(),
            b"replacement must survive"
        );
        assert!(displaced_path.exists());
        assert!(session_path.exists());
        assert!(wal_path.exists());
        assert!(snapshot_path.exists());
        assert!(checkpoint_directory.exists());
    }

    #[test]
    fn session_retirement_rejects_paths_outside_the_leased_session() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let journal = SessionJournal::open(&journal_path, "session").unwrap();
        drop(journal);
        let unrelated = dir.path().join("2026-07-19_unrelated.json");
        let unrelated_wal = unrelated.with_extension("wal");
        std::fs::write(&unrelated, b"unrelated").unwrap();
        std::fs::write(&unrelated_wal, b"unrelated wal").unwrap();

        let lease = SessionJournal::acquire_storage_lease(&journal_path, "session").unwrap();
        assert!(matches!(
            lease.remove_files(&unrelated, &unrelated_wal),
            Err(JournalError::InvalidTransition(_))
        ));
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated");
        assert_eq!(std::fs::read(&unrelated_wal).unwrap(), b"unrelated wal");
    }

    #[cfg(unix)]
    #[test]
    fn effect_checkpoint_rejects_group_or_world_access() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();

        let checkpoint = journal.effect_checkpoint_path(&digest).unwrap();
        std::fs::set_permissions(&checkpoint, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            journal.load_effect_checkpoint(&digest),
            Err(JournalError::InvalidTransition(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn effect_checkpoint_rejects_unowned_hard_link_alias() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();

        let checkpoint = journal.effect_checkpoint_path(&digest).unwrap();
        std::fs::hard_link(&checkpoint, dir.path().join("unowned-alias")).unwrap();
        assert!(matches!(
            journal.load_effect_checkpoint(&digest),
            Err(JournalError::InvalidTransition(message))
                if message.contains("unsafe links or permissions")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn effect_checkpoint_rejects_symlink_replacement() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let contents = b"private preimage";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();

        let checkpoint = journal.effect_checkpoint_path(&digest).unwrap();
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, contents).unwrap();
        std::fs::remove_file(&checkpoint).unwrap();
        symlink(&replacement, &checkpoint).unwrap();
        assert!(matches!(
            journal.load_effect_checkpoint(&digest),
            Err(JournalError::InvalidTransition(message))
                if message.contains("not a regular file")
        ));
    }

    #[test]
    fn effect_checkpoint_store_enforces_session_quota_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let seed = b"seed";
        journal
            .store_effect_checkpoint(&sha256_hex(seed), seed)
            .unwrap();
        let checkpoint_directory = journal
            .effect_checkpoint_path(&sha256_hex(seed))
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let filler = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(checkpoint_directory.join("quota-filler"))
            .unwrap();
        filler.set_len(MAX_EFFECT_CHECKPOINT_SESSION_BYTES).unwrap();

        let next = b"next checkpoint";
        assert!(matches!(
            journal.store_effect_checkpoint(&sha256_hex(next), next),
            Err(JournalError::InvalidTransition(message)) if message.contains("session quota")
        ));
    }

    #[test]
    fn append_io_failure_permanently_faults_writer() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let mut writer = JournalWriter::open(journal_path.clone(), "session".to_owned()).unwrap();
        writer.file = OpenOptions::new().read(true).open(journal_path).unwrap();

        let event = SessionEvent::TurnStarted {
            turn_id: "turn".into(),
            user_message: "hello".into(),
        };
        assert!(matches!(
            writer.append(event.clone()),
            Err(JournalError::Io { .. })
        ));
        assert!(matches!(
            writer.append(event),
            Err(JournalError::WriterFaulted)
        ));
        assert!(matches!(writer.compact(), Err(JournalError::WriterFaulted)));
    }

    #[test]
    fn uncertain_compaction_publication_permanently_faults_writer() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let mut writer = JournalWriter::open(journal_path, "session".to_owned()).unwrap();
        let event = SessionEvent::TurnStarted {
            turn_id: "turn".into(),
            user_message: "hello".into(),
        };
        writer.append(event.clone()).unwrap();

        snapshot::fail_next_replace_after_persist();
        assert!(matches!(writer.compact(), Err(JournalError::Io { .. })));
        assert!(matches!(
            writer.append(event),
            Err(JournalError::WriterFaulted)
        ));
        assert!(matches!(writer.compact(), Err(JournalError::WriterFaulted)));
    }

    #[cfg(unix)]
    #[test]
    fn substituted_snapshot_after_authority_write_before_final_validation_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let snapshot_path = snapshot_path_for(&journal_path);
        let displaced = dir.path().join("displaced.snapshot");
        let replacement = dir.path().join("replacement.snapshot");
        let mut writer = JournalWriter::open(journal_path.clone(), "session".to_owned()).unwrap();
        writer
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".into(),
                user_message: "hello".into(),
            })
            .unwrap();
        let substitute = SessionSnapshot::new("session", ReducedSessionState::default()).unwrap();
        snapshot::write_private_snapshot_fixture(
            &replacement,
            &serde_json::to_vec(&substitute).unwrap(),
        )
        .unwrap();
        let authority_journal_path = journal_path.clone();
        set_after_snapshot_authority_write_hook(move |canonical| {
            let head = snapshot::load_snapshot_authority_head(&authority_journal_path)
                .unwrap()
                .unwrap();
            assert!(head.accepted.is_some());
            assert!(head.pending.is_none());
            std::fs::rename(canonical, displaced).unwrap();
            std::fs::rename(replacement, canonical).unwrap();
        });

        assert!(matches!(
            writer.publish_snapshot(),
            Err(JournalError::PathIdentityMismatch { path }) if path == snapshot_path
        ));
        assert!(matches!(
            writer.append(SessionEvent::TurnCancelled {
                turn_id: "turn".into(),
            }),
            Err(JournalError::WriterFaulted)
        ));
        let head = snapshot::load_snapshot_authority_head(&journal_path)
            .unwrap()
            .unwrap();
        assert!(head.accepted.is_none());
        assert!(head.pending.is_some());
        assert_eq!(snapshot::load_snapshot(&snapshot_path).unwrap(), substitute);
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_privacy_change_after_authority_write_is_rejected() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let snapshot_path = snapshot_path_for(&journal_path);
        let mut writer = JournalWriter::open(journal_path.clone(), "session".to_owned()).unwrap();
        writer
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".into(),
                user_message: "hello".into(),
            })
            .unwrap();
        set_after_snapshot_authority_write_hook(move |canonical| {
            std::fs::set_permissions(canonical, std::fs::Permissions::from_mode(0o640)).unwrap();
        });

        assert!(matches!(
            writer.publish_snapshot(),
            Err(JournalError::SnapshotUnsafePermissions { path }) if path == snapshot_path
        ));
        assert!(matches!(
            writer.append(SessionEvent::TurnCancelled {
                turn_id: "turn".into(),
            }),
            Err(JournalError::WriterFaulted)
        ));
        let head = snapshot::load_snapshot_authority_head(&journal_path)
            .unwrap()
            .unwrap();
        assert!(head.accepted.is_none());
        assert!(head.pending.is_some());
    }
}
