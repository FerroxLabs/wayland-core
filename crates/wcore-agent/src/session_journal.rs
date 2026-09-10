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

#[cfg_attr(test, allow(clippy::type_complexity))]
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
#[cfg_attr(test, allow(clippy::type_complexity))]
mod snapshot;
pub use snapshot::{
    LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION, SESSION_SNAPSHOT_SCHEMA_VERSION, SessionSnapshot,
    load_snapshot, snapshot_path_for, write_private_snapshot_fixture,
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

#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[rustfmt::skip]
pub struct JournalEnvelope {
    pub schema_version: u32, pub session_id: String, pub seq: u64,
    pub previous_checksum: String, pub event: SessionEvent, pub checksum: String,
    /// 23B-H1: the SHA-256 of the checksum material EXACTLY AS IT WAS READ FROM
    /// DISK, populated by `parse_complete_frames` and by nothing else. Never
    /// serialized and never read from a journal. See [`Self::computed_checksum`].
    #[serde(skip)] pub(crate) on_disk_checksum: Option<String>,
}

/// Bookkeeping fields are excluded: two envelopes carrying the same authority
/// are equal whether they were read from disk or built in memory.
impl PartialEq for JournalEnvelope {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.session_id == other.session_id
            && self.seq == other.seq
            && self.previous_checksum == other.previous_checksum
            && self.event == other.event
            && self.checksum == other.checksum
    }
}

#[derive(Serialize)]
#[rustfmt::skip]
struct ChecksumMaterial<'a> {
    schema_version: u32, session_id: &'a str, seq: u64,
    previous_checksum: &'a str, event: &'a SessionEvent,
}

/// The same five members, held as the **verbatim bytes** the producer wrote.
/// Serializing a `RawValue` re-emits its source text unchanged, so encoding
/// this reproduces the producer's checksum material byte for byte.
#[derive(Serialize)]
#[rustfmt::skip]
struct StoredChecksumMaterial<'a> {
    schema_version: &'a serde_json::value::RawValue,
    session_id: &'a serde_json::value::RawValue,
    seq: &'a serde_json::value::RawValue,
    previous_checksum: &'a serde_json::value::RawValue,
    event: &'a serde_json::value::RawValue,
}

#[derive(Deserialize)]
#[rustfmt::skip]
struct StoredEnvelopeFields<'a> {
    #[serde(borrow)] schema_version: &'a serde_json::value::RawValue,
    #[serde(borrow)] session_id: &'a serde_json::value::RawValue,
    #[serde(borrow)] seq: &'a serde_json::value::RawValue,
    #[serde(borrow)] previous_checksum: &'a serde_json::value::RawValue,
    #[serde(borrow)] event: &'a serde_json::value::RawValue,
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
            on_disk_checksum: None,
        };
        envelope.checksum = envelope.computed_checksum()?;
        Ok(envelope)
    }

    /// The checksum this envelope's authority must equal.
    ///
    /// # 23B-H1 — why this hashes the stored bytes rather than a re-encoding
    ///
    /// This used to hash a RE-SERIALIZATION of the decoded event, which made
    /// the integrity check depend on serde encode/decode being a bijection over
    /// `SessionEvent`. It is not one, and the reader says so itself:
    /// `known_omitted_default` blesses fifteen raw encodings as explicit
    /// defaults equivalent to absent — `"retry_of":null`,
    /// `"effect_receipt":null`, `"provider_reservations":{}` and so on. Every
    /// one of those is by definition dropped by the decode, so re-serializing
    /// covered different bytes than the producer hashed and
    /// `verify_chain_from` called corrupt exactly what
    /// `reject_unknown_event_fields` had just declared valid. The session then
    /// became permanently unreachable, because all twelve `session` operator
    /// verbs read through that chain. That is the 23B-01 symptom.
    ///
    /// Hashing the bytes on disk removes the dependency on canonical
    /// re-encoding entirely, and is **strictly tighter**, not looser:
    ///
    /// * every difference the re-encoding caught is still caught — a changed
    ///   value changes the bytes;
    /// * differences the re-encoding NORMALISED AWAY (reordered members,
    ///   inserted whitespace) are now caught, where before they verified clean;
    /// * the only forms newly accepted are raw encodings the reader has already
    ///   declared valid — anything the decode drops that is NOT allowlisted is
    ///   rejected by `reject_unknown_event_fields`, which runs first and fails
    ///   closed with `UnknownCriticalField`.
    ///
    /// In-memory envelopes have no stored bytes and fall back to encoding the
    /// value, which is what the writer is about to put on disk — so `create`
    /// and the read path agree by construction.
    fn computed_checksum(&self) -> Result<String, JournalError> {
        if let Some(on_disk) = &self.on_disk_checksum {
            return Ok(on_disk.clone());
        }
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

    /// Hash the checksum material exactly as `body` carries it.
    fn checksum_of_stored_bytes(body: &[u8]) -> Result<String, JournalError> {
        let stored = serde_json::from_slice::<StoredEnvelopeFields>(body).map_err(|source| {
            JournalError::Json {
                context: "reading stored checksum material",
                source,
            }
        })?;
        let bytes = serde_json::to_vec(&StoredChecksumMaterial {
            schema_version: stored.schema_version,
            session_id: stored.session_id,
            seq: stored.seq,
            previous_checksum: stored.previous_checksum,
            event: stored.event,
        })
        .map_err(|source| JournalError::Json {
            context: "encoding stored checksum material",
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

/// Release the journal data-file lock when the writer dies.
///
/// Mirrors `WriterLease::drop`, which has always done this for the
/// `.writer.lock` sentinel. See [`lease::unlock_data_file`] for why `close(2)`
/// alone is not sufficient.
impl Drop for JournalWriter {
    fn drop(&mut self) {
        lease::unlock_data_file(&self.file);
    }
}

type SharedWriter = Arc<Mutex<JournalWriter>>;

#[derive(Debug, Clone)]
pub struct SessionJournal {
    inner: SharedWriter,
    /// Quota admission for this session's effect-checkpoint directory. Clones
    /// share it and an independent open fails closed, so every in-process store
    /// into that directory uses this one ledger and no other session's store
    /// touches it (wayland#1353). Deliberately NOT the writer lock: appends never
    /// wait on a checkpoint store.
    checkpoint_quota: Arc<Mutex<CheckpointQuota>>,
    /// Temporaries this handle family's stores are writing, so a cleanup never
    /// removes a live one (wayland#1357). Shared by clones, like the quota.
    checkpoint_temporaries: Arc<LiveCheckpointTemporaries>,
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
    pub(crate) storage_identity_digest: String,
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
type JournalPathHook = std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>>;

#[cfg(test)]
thread_local! {
    static AFTER_JOURNAL_READ_HOOK: JournalPathHook = std::cell::RefCell::new(None);
    static AFTER_SNAPSHOT_AUTHORITY_WRITE_HOOK: JournalPathHook = std::cell::RefCell::new(None);
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

// The RUNNER below stays `#[cfg(test)]` because the write path invokes it on
// every target; only this SETTER is Unix-exclusive, because its two callers are
// `#[cfg(unix)]` tests. Gating the setter to match its callers is what keeps
// the Windows leg free of a `dead_code` error without an `#[allow]`.
#[cfg(all(test, unix))]
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

/// Release the journal data-file lock when the storage lease dies.
///
/// Same reasoning as `Drop for JournalWriter`.
impl Drop for SessionStorageLease {
    fn drop(&mut self) {
        if let Some(file) = &self._journal_file {
            lease::unlock_data_file(file);
        }
    }
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
            checkpoint_quota: Arc::default(),
            checkpoint_temporaries: Arc::default(),
        })
    }

    pub fn append(&self, event: SessionEvent) -> Result<JournalEnvelope, JournalError> {
        if matches!(
            &event,
            SessionEvent::ChildTransactionOpened { .. }
                | SessionEvent::ChildTransactionReceiptCommitted { .. }
                | SessionEvent::ChildTransactionLandingPrepared { .. }
                | SessionEvent::ChildTransactionLandingRefAdvanced { .. }
                | SessionEvent::ChildTransactionLandingProjected { .. }
                | SessionEvent::ChildTransactionLanded { .. }
                | SessionEvent::ChildTransactionLandingConflict { .. }
                | SessionEvent::ChildTransactionLandingRecoveryRequired { .. }
                | SessionEvent::ChildTransactionRollbackPrepared { .. }
                | SessionEvent::ChildTransactionRolledBack { .. }
        ) {
            return Err(JournalError::InvalidTransition(
                "child transaction authority events require ChildTransactionStore".to_owned(),
            ));
        }
        // The Goal kernel is the SOLE writer of Goal, task and wait transitions.
        // Refusing them on the public path is what makes that structural rather
        // than a convention: a transition with no attributable kernel append
        // cannot exist, which is the repudiation property (T-22-04).
        if matches!(
            &event,
            SessionEvent::GoalOpened { .. }
                | SessionEvent::GoalIterationStarted { .. }
                | SessionEvent::GoalWaitBegun { .. }
                | SessionEvent::GoalWaitResolved { .. }
                | SessionEvent::GoalRunResumed { .. }
                | SessionEvent::GoalTerminated { .. }
                | SessionEvent::GoalLoopOwnerClaimed { .. }
                | SessionEvent::GoalLoopOwnerFinished { .. }
                | SessionEvent::GoalTaskDeclared { .. }
                | SessionEvent::GoalTaskTransitioned { .. }
        ) {
            return Err(JournalError::InvalidTransition(
                "goal transitions require the goal kernel".to_owned(),
            ));
        }
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

    /// Build an event from the current journal head and append it under one
    /// uninterrupted writer-lock operation.
    ///
    /// Events whose validity is bound to the head they were derived from — a
    /// budget authority carries the prior cursor and the conversation digest it
    /// captured — must never read that head through [`Self::state`] and append
    /// afterwards. Between those two lock acquisitions any other writer may
    /// advance `last_seq`, and the reducer then rejects the append for a
    /// collision the caller had no way to observe. Capturing and appending in
    /// one critical section removes the window rather than retrying after it.
    ///
    /// The builder must not re-enter this journal: the writer lock is held for
    /// its whole execution and is not reentrant.
    pub(crate) fn append_built_from_head<F>(
        &self,
        build_event: F,
    ) -> Result<JournalEnvelope, JournalError>
    where
        F: FnOnce(&ReducedSessionState) -> Result<SessionEvent, JournalError>,
    {
        let mut writer = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?;
        let event = build_event(&writer.state)?;
        writer.append(event)
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
            "storage_identity_digest": storage_identity_digest(&writer.path),
            "base_snapshot_digest": committed
                .base_snapshot
                .as_ref()
                .map(|base| base.state_digest.as_str()),
        });
        let authority = JournalSnapshotAuthority {
            session_id: writer.session_id.clone(),
            storage_identity_digest: storage_identity_digest(&writer.path),
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

    /// Read ONE durable child record from the live writer state.
    ///
    /// Same lock and the same poisoning error as [`Self::state`], but it clones
    /// only the requested record. The durable spawner inspects a child several
    /// times per dispatch, and cloning the whole reduced state for each lookup
    /// made every dispatch cost grow with the number of children already in the
    /// journal (wayland#1301).
    pub(crate) fn durable_child(
        &self,
        child_id: &str,
    ) -> Result<Option<wcore_types::spawner::DurableChildRecord>, JournalError> {
        let writer = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?;
        Ok(writer
            .state
            .children
            .get(child_id)
            .and_then(|child| child.durable.clone()))
    }

    /// Read the same live writer state as `state`, without copying unrelated
    /// conversation and provider payloads for child-supervision queries.
    pub(crate) fn durable_children(
        &self,
    ) -> Result<Vec<wcore_types::spawner::DurableChildRecord>, JournalError> {
        let writer = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?;
        Ok(writer
            .state
            .children
            .values()
            .filter_map(|child| child.durable.clone())
            .collect())
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

    pub(crate) fn storage_identity_digest(&self) -> Result<String, JournalError> {
        let mut writer = self
            .inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)?;
        writer.ensure_current_path_identity()?;
        Ok(storage_identity_digest(&writer.path))
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

        remove_stale_checkpoint_temps(
            directory,
            digest,
            path.exists(),
            &self.checkpoint_temporaries,
            None,
        )?;

        if path.exists() {
            self.load_effect_checkpoint(digest)?;
            return Ok(());
        }
        // The scan and the write it admits are one quota decision (wayland#1353):
        // the admission is taken BEFORE the scan and is decided against it plus
        // every store the scan may not have seen.
        let mut admission = CheckpointAdmission::before_scan(&self.checkpoint_quota);
        let session_bytes = scan_checkpoint_quota(directory)?;
        admission.admit(session_bytes, contents.len() as u64)?;

        let temporary_name = format!(
            ".{digest}.{}.{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let temporary = directory.join(&temporary_name);
        // Live before the file exists, until this store returns (wayland#1357).
        let _live_temporary =
            LiveCheckpointTemporary::register(&self.checkpoint_temporaries, temporary_name);
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
            #[cfg(test)]
            quota_race_gate::reached(quota_race_gate::Point::BeforeLink, directory, None);
            match std::fs::hard_link(&temporary, &path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            }
        })();
        #[cfg(test)]
        quota_race_gate::reached(quota_race_gate::Point::AfterLink, directory, None);
        let _ = std::fs::remove_file(&temporary);
        publication.map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
        // Published: every later scan sees the checkpoint, so the reservation
        // can end. Every earlier return ends it through `Drop`.
        drop(admission);
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
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| JournalError::Io {
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
            // Only the hard-link recheck below reassigns it, and that path is
            // Unix-only — so the mutable binding is shadowed here rather than
            // declared above, where Windows would carry a needless `mut`.
            let mut metadata = metadata;
            if metadata.nlink() > 1 {
                remove_stale_checkpoint_temps(
                    path.parent().expect("checkpoint path has a parent"),
                    digest,
                    true,
                    &self.checkpoint_temporaries,
                    Some(&metadata),
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
        // Report the same pathname `open` reports for this file — see
        // `lease::reported_path`.
        let path = &lease::reported_path(path.as_ref());
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
        // Report the same pathname `open` reports for this file — see
        // `lease::reported_path`.
        let path = &lease::reported_path(path.as_ref());
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

/// The directory spelling every journal entry point reports for files inside
/// `directory`.
///
/// Exposed for this crate's integration tests, alongside
/// [`write_private_snapshot_fixture`]. A fixture built from
/// `tempfile::tempdir()` carries whatever spelling `$TMPDIR` has — macOS routes
/// it through the `/var` -> `/private/var` symlink, Windows can hand back an 8.3
/// short name — while the journal reports the resolved one. Comparing a journal
/// error path against a raw fixture path is therefore comparing two names for
/// one file. Deriving the fixture root from here keeps that comparison EXACT;
/// the alternative, dropping the path equality and matching only the error
/// variant, would retire the guard instead of fixing it.
#[doc(hidden)]
pub fn canonical_journal_root(directory: &Path) -> Result<PathBuf, JournalError> {
    lease::canonical_simplified_dir(directory)
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
    // The published inode is locked by `write_snapshot`. This entry point
    // grants no durable authority, so the guard releases it here rather than
    // leaving it to `close(2)` - see [`lease::unlock_data_file`].
    drop(snapshot::write_snapshot(path, snapshot)?);
    Ok(())
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
        let mut first_error = None;
        let session = capture_retirement_artifact(session_path, None, &mut first_error);
        let wal = capture_retirement_artifact(wal_path, None, &mut first_error);
        let snapshot = capture_retirement_artifact(
            &snapshot_path_for(&self.journal_path),
            None,
            &mut first_error,
        );
        let journal = capture_retirement_artifact(
            &self.journal_path,
            self._journal_file.as_ref(),
            &mut first_error,
        );
        let authority_head = capture_retirement_artifact(
            &snapshot::snapshot_authority_head_path(&self.journal_path),
            None,
            &mut first_error,
        );

        // Attempt every artifact so one undeletable file does not strand other
        // plaintext. The caller retains index authority if any unlink or
        // directory sync fails, making every residual file discoverable.
        if let Err(error) = remove_effect_checkpoint_directory(&self.journal_path) {
            first_error.get_or_insert(error);
        }
        for captured in [&session, &wal, &snapshot, &journal].into_iter().flatten() {
            if let Err(error) = captured.remove()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if first_error.is_none()
            && let Some(authority_head) = authority_head.as_ref()
            && let Err(error) = authority_head.remove()
        {
            first_error = Some(error);
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
        // Both operands must be in the same representation: on Windows a raw
        // `canonicalize` yields a verbatim `\\?\` path while the leased journal
        // path is stored simplified, and comparing the two forms would reject
        // every legitimate retirement.
        let journal_parent = dunce::simplified(journal_parent);
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
    lease::canonical_simplified_dir(parent)
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

fn capture_retirement_artifact(
    path: &Path,
    expected: Option<&File>,
    first_error: &mut Option<JournalError>,
) -> Option<CapturedRetirementFile> {
    match CapturedRetirementFile::capture(path, expected) {
        Ok(captured) => Some(captured),
        Err(error) => {
            first_error.get_or_insert(error);
            None
        }
    }
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

/// In-process quota ledger for one session's effect-checkpoint directory.
///
/// The quota scan reads the directory with no lock held, so by itself it cannot
/// see a store that was admitted and has not yet published (wayland#1353). Each
/// store records `released` before it scans and is then decided against the scan
/// PLUS every reservation still held PLUS every reservation that ended since that
/// record. However a concurrent store's publication interleaves with the scan, it
/// is counted at least once: still reserved, released during the scan, or
/// published before the scan began and so listed by it. It may be counted twice,
/// which can only refuse a store near the quota, never admit one past it. The
/// two critical sections are O(1); neither spans the scan or the write, so the
/// per-store cost the scan carries (wayland#1301) is unchanged.
#[derive(Debug, Default)]
struct CheckpointQuota {
    /// Bytes admitted to stores that have not yet published or failed.
    reserved: u64,
    /// Running total of bytes whose reservation has ended. It wraps; only the
    /// difference between two readings is meaningful.
    released: u64,
}

impl CheckpointQuota {
    /// Every critical section on this ledger is non-panicking integer arithmetic,
    /// so a poisoned lock cannot hold a half-applied update. It is recovered
    /// rather than refused, which would wedge the session's quota for good.
    fn lock(ledger: &Mutex<Self>) -> std::sync::MutexGuard<'_, Self> {
        ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One store's claim on its session's checkpoint quota.
///
/// Dropping it ends the reservation, on success, error return and unwind alike,
/// so a failed or panicking store never leaks quota. A process crash takes the
/// ledger with it, and whatever that store left on disk (a `.{digest}.*.tmp`) is
/// counted by the next scan instead.
struct CheckpointAdmission<'a> {
    ledger: &'a Mutex<CheckpointQuota>,
    released_before_scan: u64,
    reserved: u64,
}

impl<'a> CheckpointAdmission<'a> {
    /// Must be taken before the directory scan that [`Self::admit`] is given.
    fn before_scan(ledger: &'a Mutex<CheckpointQuota>) -> Self {
        let released_before_scan = CheckpointQuota::lock(ledger).released;
        Self {
            ledger,
            released_before_scan,
            reserved: 0,
        }
    }

    /// Reserve `len` bytes, or refuse if the scan plus every store it may have
    /// missed plus `len` exceeds the session quota.
    fn admit(&mut self, scanned: u64, len: u64) -> Result<(), JournalError> {
        let mut ledger = CheckpointQuota::lock(self.ledger);
        let unseen = ledger
            .reserved
            .saturating_add(ledger.released.wrapping_sub(self.released_before_scan));
        if scanned.saturating_add(unseen).saturating_add(len) > MAX_EFFECT_CHECKPOINT_SESSION_BYTES
        {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoints exceed the {MAX_EFFECT_CHECKPOINT_SESSION_BYTES}-byte session quota"
            )));
        }
        ledger.reserved = ledger.reserved.saturating_add(len);
        self.reserved = len;
        Ok(())
    }
}

impl Drop for CheckpointAdmission<'_> {
    fn drop(&mut self) {
        if self.reserved == 0 {
            return;
        }
        #[cfg(test)]
        quota_race_gate::reservation_ended();
        let mut ledger = CheckpointQuota::lock(self.ledger);
        ledger.reserved = ledger.reserved.saturating_sub(self.reserved);
        ledger.released = ledger.released.wrapping_add(self.reserved);
    }
}

/// The session-quota scan exactly as a store takes it.
///
/// Test instrumentation is the only thing between the directory listing and
/// this function's return: under test a store can be parked here, AFTER its
/// listing and BEFORE any statement that follows the scan. That is what lets a
/// test run another store's whole publication inside that gap, so moving the
/// released snapshot to after the scan cannot hide from it (wayland#1353).
///
/// An entry another store removes between the listing and its stat is gone, not
/// an error (wayland#1357), so the directory is listed again. Every rescan is
/// still taken after the admission's released snapshot, so rescanning cannot
/// weaken the quota. The bound only stops a directory that never holds still
/// from spinning; reaching it fails closed, as a vanished entry always did.
fn scan_checkpoint_quota(directory: &Path) -> Result<u64, JournalError> {
    const MAX_SCANS: usize = 8;
    let mut scans = 1;
    let scanned = loop {
        match checkpoint_directory_bytes(directory) {
            Err(JournalError::Io { path, source })
                if source.kind() == std::io::ErrorKind::NotFound
                    && path.as_path() != directory
                    && scans < MAX_SCANS =>
            {
                scans += 1;
            }
            result => break result?,
        }
    };
    #[cfg(test)]
    quota_race_gate::after_quota_scan(directory);
    Ok(scanned)
}

/// TEST-ONLY rendezvous between a checkpoint store's session-quota scan and the
/// quota decision that uses it (wayland#1353).
///
/// While armed, a store into one of the armed checkpoint directories that has
/// just finished its scan waits until `expected` stores (across every armed
/// directory) have reached this point, so all of them scan before any of them is
/// decided. A waiter that gives up after the timeout is counted, so a test can
/// tell a real rendezvous from stores that something else serialized. Stores in
/// any other directory are never gated, and one test at a time may arm it.
#[cfg(test)]
pub(crate) mod quota_race_gate {
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
    use std::time::Duration;

    #[derive(Default)]
    struct Gate {
        armed_for: Vec<PathBuf>,
        expected: usize,
        passed: usize,
        timed_out: usize,
        /// Park mode: only the FIRST store to arrive waits, until released;
        /// every later store passes straight through.
        park_first: bool,
        park_at: Point,
        parked: bool,
        released: bool,
    }

    thread_local! {
        static RESERVATION_ENDED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            std::cell::RefCell::new(None);
    }

    /// Run `hook` the next time a checkpoint reservation ends on this thread.
    pub(crate) fn set_reservation_end_hook(hook: impl FnOnce() + 'static) {
        RESERVATION_ENDED.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    pub(crate) fn reservation_ended() {
        RESERVATION_ENDED.with(|slot| {
            if let Some(hook) = slot.borrow_mut().take() {
                hook();
            }
        });
    }

    /// Where in a checkpoint store a test may park it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) enum Point {
        /// After the session-quota scan, before the quota decision.
        #[default]
        AfterScan,
        /// The temporary is written and synced; its hard link is next.
        BeforeLink,
        /// The hard link was attempted; removing the temporary is next.
        AfterLink,
        /// Stale-temporary cleanup listed a `.{digest}.*.tmp`; its stat is next.
        CleanupListed,
        /// Stale-temporary cleanup stat'd that entry; its removal is next.
        CleanupStatted,
        /// The quota scan listed a `.tmp` entry; its stat is next.
        ScanListed,
    }

    /// Arm park mode for `directory`: the first store that finishes its scan
    /// there waits until [`Armed::release`]; no other store is held.
    pub(crate) fn park_first(directory: &Path) -> Armed {
        park_first_at(directory, Point::AfterScan)
    }

    /// Arm park mode for `directory` at `point`: the first store to reach it
    /// there waits until [`Armed::release`]; no other store is held.
    pub(crate) fn park_first_at(directory: &Path, point: Point) -> Armed {
        let armed = arm(&[directory], 0);
        let (lock, _) = gate();
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        state.park_first = true;
        state.park_at = point;
        drop(state);
        armed
    }

    /// What one armed window observed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Rendezvous {
        pub(crate) passed: usize,
        pub(crate) timed_out: usize,
    }

    /// Exclusive use of the gate for one test; dropping it disarms.
    pub(crate) struct Armed {
        _serial: MutexGuard<'static, ()>,
    }

    fn gate() -> &'static (Mutex<Gate>, Condvar) {
        static GATE: OnceLock<(Mutex<Gate>, Condvar)> = OnceLock::new();
        GATE.get_or_init(|| (Mutex::new(Gate::default()), Condvar::new()))
    }

    pub(crate) fn arm(directories: &[&Path], expected: usize) -> Armed {
        static SERIAL: Mutex<()> = Mutex::new(());
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let (lock, _) = gate();
        *lock.lock().unwrap_or_else(PoisonError::into_inner) = Gate {
            armed_for: directories.iter().map(|path| path.to_path_buf()).collect(),
            expected,
            ..Gate::default()
        };
        Armed { _serial: serial }
    }

    impl Armed {
        /// Park mode: wait until a store is parked. False if none arrived.
        pub(crate) fn wait_until_parked(&self) -> bool {
            let (lock, condvar) = gate();
            let state = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let (state, _) = condvar
                .wait_timeout_while(state, Duration::from_secs(10), |state| !state.parked)
                .unwrap_or_else(PoisonError::into_inner);
            state.parked
        }

        /// Park mode: let the parked store continue.
        pub(crate) fn release(&self) {
            let (lock, condvar) = gate();
            lock.lock().unwrap_or_else(PoisonError::into_inner).released = true;
            condvar.notify_all();
        }

        pub(crate) fn disarm(self) -> Rendezvous {
            let (lock, _) = gate();
            let state = lock.lock().unwrap_or_else(PoisonError::into_inner);
            Rendezvous {
                passed: state.passed,
                timed_out: state.timed_out,
            }
        }
    }

    impl Drop for Armed {
        fn drop(&mut self) {
            let (lock, condvar) = gate();
            *lock.lock().unwrap_or_else(PoisonError::into_inner) = Gate::default();
            condvar.notify_all();
        }
    }

    pub(crate) fn after_quota_scan(directory: &Path) {
        reached(Point::AfterScan, directory, None);
    }

    /// A store in `directory` reached `point`. `name` is the directory entry the
    /// point concerns, where there is one. Park mode holds the first store to
    /// reach the armed point (at [`Point::ScanListed`], the first `.tmp` entry);
    /// the rendezvous applies only after the scan.
    pub(crate) fn reached(point: Point, directory: &Path, name: Option<&std::ffi::OsStr>) {
        let (lock, condvar) = gate();
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if !state
            .armed_for
            .iter()
            .any(|armed| armed.as_path() == directory)
        {
            return;
        }
        if state.park_first {
            if state.parked
                || state.park_at != point
                || (point == Point::ScanListed
                    && !name.is_some_and(|name| name.to_string_lossy().ends_with(".tmp")))
            {
                return;
            }
            state.parked = true;
            state.passed += 1;
            condvar.notify_all();
            let (mut state, wait) = condvar
                .wait_timeout_while(state, Duration::from_secs(10), |state| {
                    !state.armed_for.is_empty() && !state.released
                })
                .unwrap_or_else(PoisonError::into_inner);
            if wait.timed_out() {
                state.timed_out += 1;
            }
            return;
        }
        if point != Point::AfterScan {
            return;
        }
        state.passed += 1;
        condvar.notify_all();
        let (mut state, wait) = condvar
            .wait_timeout_while(state, Duration::from_secs(5), |state| {
                !state.armed_for.is_empty() && state.passed < state.expected
            })
            .unwrap_or_else(PoisonError::into_inner);
        if wait.timed_out() {
            state.timed_out += 1;
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

/// Names of the checkpoint temporaries this journal handle is writing right now.
///
/// Every store into a session's checkpoint directory runs through one handle
/// family, whose clones share this set: the writer lease refuses an independent
/// open of the same journal, in this process or another. So a `.{digest}.*.tmp`
/// whose name is NOT in the set belongs to no live store; it was left by a crash,
/// or its store is already done with it. No process id or file age is consulted
/// (wayland#1357).
type LiveCheckpointTemporaries = Mutex<std::collections::HashSet<String>>;

/// Insert, remove and lookup cannot leave the set half-updated, so a poisoned
/// lock is recovered rather than refused.
fn lock_live_temporaries(
    live: &LiveCheckpointTemporaries,
) -> std::sync::MutexGuard<'_, std::collections::HashSet<String>> {
    live.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One store's temporary, registered as live for as long as the store holds it.
///
/// Registered BEFORE the file is created, so no cleanup can list the file while
/// it is unregistered; unregistered on every exit, unwind included, so a file a
/// failed store leaves behind becomes cleanable.
struct LiveCheckpointTemporary<'a> {
    live: &'a LiveCheckpointTemporaries,
    name: String,
}

impl<'a> LiveCheckpointTemporary<'a> {
    fn register(live: &'a LiveCheckpointTemporaries, name: String) -> Self {
        lock_live_temporaries(live).insert(name.clone());
        Self { live, name }
    }
}

impl Drop for LiveCheckpointTemporary<'_> {
    fn drop(&mut self) {
        lock_live_temporaries(self.live).remove(&self.name);
    }
}

/// Whether `candidate` is another hard link to the file `published` describes.
#[cfg(unix)]
fn is_link_to_published_checkpoint(
    candidate: &std::fs::Metadata,
    published: Option<&std::fs::Metadata>,
) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    published.is_some_and(|published| {
        published.dev() == candidate.dev() && published.ino() == candidate.ino()
    })
}

/// Only Unix exposes file identity through std metadata, and only Unix loads
/// run the redundant-link cleanup, so elsewhere nothing is a redundant link.
#[cfg(not(unix))]
fn is_link_to_published_checkpoint(
    _candidate: &std::fs::Metadata,
    _published: Option<&std::fs::Metadata>,
) -> bool {
    false
}

/// Remove the stale `.{digest}.*.tmp` files of one checkpoint.
///
/// A temporary another LIVE store is writing is left alone, or that store's hard
/// link would fail (wayland#1357). The one exception is a live temporary that is
/// already a second hard link to `published_checkpoint`: removing that name only
/// drops a redundant link, which its store ignores. An entry that is gone by the
/// time it is examined or removed is simply gone.
fn remove_stale_checkpoint_temps(
    directory: &Path,
    digest: &str,
    published: bool,
    live: &LiveCheckpointTemporaries,
    published_checkpoint: Option<&std::fs::Metadata>,
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
        #[cfg(test)]
        quota_race_gate::reached(quota_race_gate::Point::CleanupListed, directory, None);
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(JournalError::Io { path, source }),
        };
        if metadata.is_dir() {
            return Err(JournalError::InvalidTransition(format!(
                "filesystem effect checkpoint temporary path is a directory: {}",
                path.display()
            )));
        }
        if lock_live_temporaries(live).contains(name)
            && !is_link_to_published_checkpoint(&metadata, published_checkpoint)
        {
            continue;
        }
        #[cfg(test)]
        quota_race_gate::reached(quota_race_gate::Point::CleanupStatted, directory, None);
        if published || metadata.file_type().is_symlink() || metadata.is_file() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(JournalError::Io { path, source }),
            }
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
        #[cfg(test)]
        quota_race_gate::reached(
            quota_race_gate::Point::ScanListed,
            directory,
            entry
                .as_ref()
                .ok()
                .map(|entry| entry.file_name())
                .as_deref(),
        );
        let entry = entry.map_err(|source| JournalError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata = checkpoint_entry_metadata(&entry).map_err(|source| JournalError::Io {
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

/// Whether a checkpoint-store entry name is a PUBLISHED checkpoint: exactly a
/// digest as [`valid_sha256_hex`] defines it (64 lowercase hex digits). This is a
/// whole-name match, never a prefix or suffix test, so temporaries
/// (`.{digest}.*.tmp`), leftovers and unrecognised names are all excluded.
fn is_published_checkpoint_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(valid_sha256_hex)
}

/// Size one entry of the private checkpoint store for the session quota.
///
/// A published checkpoint is created only by hard-linking a temporary that was
/// already fully written and synced (`store_effect_checkpoint`), and nothing in
/// this crate rewrites or truncates it afterwards, so the size the directory
/// listing returned for that entry is final. Reading it from the listing avoids
/// opening every published file on every store. On Windows each
/// `symlink_metadata` is a full open, query and close, and the store gains one
/// file per durable child, so those opens made each dispatch cost grow with the
/// number of dispatches before it (wayland#1301).
///
/// Every other entry keeps the per-file `symlink_metadata` open, whose size is
/// current even while another store is still writing that file.
fn checkpoint_entry_metadata(entry: &std::fs::DirEntry) -> std::io::Result<std::fs::Metadata> {
    if is_published_checkpoint_name(&entry.file_name()) {
        return entry.metadata();
    }
    #[cfg(test)]
    checkpoint_sizing_probe::record_opened(&entry.file_name());
    std::fs::symlink_metadata(entry.path())
}

/// TEST-ONLY record of which checkpoint entries were sized through a per-file
/// open, so a test can prove the listing path is taken for published names only.
#[cfg(test)]
mod checkpoint_sizing_probe {
    use std::cell::RefCell;
    use std::ffi::{OsStr, OsString};

    thread_local! {
        static OPENED: RefCell<Vec<OsString>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record_opened(name: &OsStr) {
        OPENED.with(|opened| opened.borrow_mut().push(name.to_os_string()));
    }

    pub(super) fn take_opened() -> Vec<OsString> {
        OPENED.with(|opened| std::mem::take(&mut *opened.borrow_mut()))
    }
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
        #[cfg(feature = "test-utils")]
        if stabilization_crash_cut(&event, &self.state, "before") {
            self.faulted = true;
            return Err(JournalError::Io {
                path: self.path.clone(),
                source: std::io::Error::other("W04 injected cleanup journal failure"),
            });
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
        #[cfg(feature = "test-utils")]
        stabilization_crash_cut(&envelope.event, &self.state, "after");
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
                // The outgoing handle names the pre-compaction inode, which the
                // rename has already displaced. Release its lock rather than
                // letting the handle close: a subprocess forked while it was
                // open still references the locked open file description, and
                // `close(2)` cannot reach a duplicate. `JournalWriter::drop`
                // then owns the incoming handle's lock.
                lease::unlock_data_file(&self.file);
                self.file = file.into_locked_inner();
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
        (Some(_), None) => Err(JournalError::SnapshotJournalMismatch(
            "legacy snapshot has no complete seq-0 journal prefix".to_owned(),
        )),
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
            let mut entry: JournalEnvelope =
                serde_json::from_slice(body).map_err(|source| JournalError::CorruptFrame {
                    path: path.to_path_buf(),
                    frame: frame_number,
                    source,
                })?;
            reject_unknown_event_fields(body, &entry)?;
            entry.on_disk_checksum = Some(JournalEnvelope::checksum_of_stored_bytes(body)?);
            restore_explicit_null_receipt(&mut entry, body)?;
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

/// 23B-H1 value fidelity: make the decoded event say what the stored bytes say.
///
/// `effect_receipt` is the one blessed field where the decode loses
/// information. It is `Option<serde_json::Value>`, so `Some(Value::Null)` and
/// `None` are distinct values that share the encoding `null` — serde maps a
/// JSON null to `None` for any `Option<_>`, and the writer that produced those
/// bytes held `Some(Value::Null)`. Every other blessed encoding is lossless:
/// no `Option<String>` value can be `null`, and an empty map or array decodes
/// to an empty map or array.
///
/// This changes no integrity decision. The checksum is taken from the bytes on
/// disk (see [`JournalEnvelope::computed_checksum`]), so a frame that fails its
/// checksum fails it before and after this runs; all this does is stop replay
/// reporting `None` for a receipt the journal records as present-and-null.
fn restore_explicit_null_receipt(
    entry: &mut JournalEnvelope,
    body: &[u8],
) -> Result<(), JournalError> {
    let raw =
        serde_json::from_slice::<serde_json::Value>(body).map_err(|source| JournalError::Json {
            context: "checking journal event fields",
            source,
        })?;
    if raw.pointer("/event/effect_receipt") != Some(&serde_json::Value::Null) {
        return Ok(());
    }
    let SessionEvent::ToolIntentRecordedV2 { effect_receipt, .. } = &mut entry.event else {
        return Ok(());
    };
    if effect_receipt.is_none() {
        *effect_receipt = Some(serde_json::Value::Null);
    }
    Ok(())
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

fn storage_identity_digest(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"wayland-core:session-journal-storage:v1\0");
    hasher.update(path.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
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

/// Deterministic process-cut rendezvous for the instrumented W04 test host.
/// This symbol and its environment switch do not exist in release builds.
#[cfg(feature = "test-utils")]
pub fn stabilization_test_barrier(cut: &str) -> bool {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    static HIT: AtomicBool = AtomicBool::new(false);
    if std::env::var("WAYLAND_W04_CUT").as_deref() != Ok(cut) {
        return false;
    }
    let Ok(address) = std::env::var("WAYLAND_W04_BARRIER") else {
        return false;
    };
    if HIT.swap(true, Ordering::SeqCst) {
        return false;
    }
    let address: std::net::SocketAddr = address.parse().expect("W04 barrier address");
    assert!(address.ip().is_loopback(), "W04 barrier must be loopback");
    let mut socket =
        std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(20))
            .expect("connect W04 barrier");
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(60)))
        .expect("bound W04 barrier wait");
    writeln!(socket, "{cut}").expect("signal W04 cut");
    let mut action = [0];
    socket
        .read_exact(&mut action)
        .expect("release W04 barrier or kill child");
    action[0] == b'F'
}

#[cfg(feature = "test-utils")]
fn stabilization_crash_cut(event: &SessionEvent, state: &ReducedSessionState, phase: &str) -> bool {
    let label = match (phase, event) {
        ("before", SessionEvent::ToolIntentRecordedV2 { .. }) => "before_intent",
        ("after", SessionEvent::ToolIntentRecordedV2 { .. }) => "intent_before_dispatch",
        ("before", SessionEvent::ToolExecutionFinished { .. }) => "physical_before_receipt",
        ("after", SessionEvent::ToolExecutionFinished { .. }) => "receipt_before_settlement",
        ("after", SessionEvent::BudgetAuthorityCommitted { authority })
            if authority.active_turn.is_none()
                && state.turns.values().any(|turn| turn.completion.is_some()) =>
        {
            "settlement_before_terminal"
        }
        ("before", SessionEvent::TurnCancelled { .. }) => "delete_finalizer",
        _ => return false,
    };
    stabilization_test_barrier(label)
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

        // 23B-H1: a producer stores the hash of the bytes it WRITES. This test
        // previously kept the checksum computed from the clean encoding, which
        // no producer of these bytes could have stored — so the frame verified
        // only because the re-encoding happened to strip the very fields under
        // test, and the assertion below never reached the chain check at all.
        // Re-sealing here is what lets `verify_chain` be asserted, which is the
        // check that used to reject these frames with `ChecksumMismatch`.
        let body = serde_json::to_vec(&explicit).unwrap();
        let sealed = JournalEnvelope::checksum_of_stored_bytes(&body).unwrap();
        assert_ne!(
            sealed, envelope.checksum,
            "the explicit encoding must genuinely hash differently — otherwise \
             this fixture is not exercising the defect"
        );
        explicit["checksum"] = serde_json::Value::String(sealed.clone());
        let body = serde_json::to_vec(&explicit).unwrap();

        // The decode collapses `Some(Value::Null)` to `None` for the one
        // blessed field that is an `Option<serde_json::Value>`; replay must
        // report what the journal records.
        let mut expected = envelope.clone();
        expected.checksum = sealed;
        let SessionEvent::ToolIntentRecordedV2 { effect_receipt, .. } = &mut expected.event else {
            unreachable!("the fixture is a ToolIntentRecordedV2")
        };
        *effect_receipt = Some(serde_json::Value::Null);

        let entries = parse_complete_frames(path, &raw_frame(FRAME_MAGIC, &body))
            .unwrap()
            .entries;
        assert_eq!(entries, vec![expected]);
        verify_chain(&entries)
            .expect("an encoding this reader blesses must also survive its integrity check");

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
        // Every journal entry point reports the RESOLVED spelling of the path it
        // was handed, and `$TMPDIR` reaches us through the `/var` ->
        // `/private/var` symlink on macOS. Derive the fixture root the same way
        // so the equality below compares one file against itself instead of two
        // names for it - see `canonical_journal_root`.
        let root = canonical_journal_root(dir.path()).unwrap();
        let target = root.join("valid.journal");
        let alias = root.join("alias.journal");
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
        // Resolved-spelling fixture root, as in the Unix case above: Windows can
        // hand back an 8.3 short name for `$TMPDIR` while the journal reports the
        // long one.
        let root = canonical_journal_root(dir.path()).unwrap();
        let target = root.join("valid.journal");
        let alias = root.join("alias.journal");
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
        // Resolved-spelling fixture root - see the symlink case above.
        let root = canonical_journal_root(dir.path()).unwrap();
        let target = root.join("valid.journal");
        let alias = root.join("alias.journal");
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
        // Resolved-spelling fixture root - see the symlink case above.
        let root = canonical_journal_root(dir.path()).unwrap();
        assert_readonly_recovery_rejects_path_swap(&root, "replay", |path| {
            SessionJournal::replay(path).map(|_| ())
        });
        assert_readonly_recovery_rejects_path_swap(&root, "state", |path| {
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
    fn stabilization_cleanup_keeps_disk_head_and_budget_refusals() {
        for fault in ["disk", "head", "budget"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.journal");
            let journal = SessionJournal::open(&path, "session").unwrap();
            journal
                .append(SessionEvent::TurnStarted {
                    turn_id: "turn".into(),
                    user_message: "positive".into(),
                })
                .unwrap();
            assert!(crate::recovery::RecoveryPlan::validate_cleanup(&journal).unwrap());
            match fault {
                "disk" => {
                    let mut bytes = std::fs::read(&path).unwrap();
                    bytes[20] ^= 1;
                    std::fs::write(&path, bytes).unwrap();
                }
                "head" => {
                    journal.inner.lock().unwrap().state.last_checksum = GENESIS_CHECKSUM.into()
                }
                "budget" => {
                    let tracker = wcore_budget::BudgetTracker::new(Default::default())
                        .snapshot()
                        .unwrap();
                    let mut value = serde_json::to_value(tracker).unwrap();
                    value["schema_version"] = serde_json::json!(999);
                    let state = journal.state().unwrap();
                    journal.inner.lock().unwrap().state.budget_authority =
                        Some(BudgetAuthorityState {
                            schema_version: BUDGET_AUTHORITY_SCHEMA_VERSION,
                            authority_epoch: 1,
                            prior_cursor: BudgetAuthorityCursor {
                                journal_sequence: state.last_seq,
                                journal_checksum: state.last_checksum,
                            },
                            budget_session_id: "session".into(),
                            provider_tracker: serde_json::from_value(value).unwrap(),
                            provider_reservations: Default::default(),
                            execution_root: wcore_budget::ExecutionBudget::default()
                                .start_root()
                                .snapshot()
                                .unwrap(),
                            active_turn: None,
                            captured_at_unix_millis: 1,
                            wall_clock: BudgetWallClockAuthority::ActiveRuntime,
                            conversation_digest: state_payload_digest(&serde_json::Value::Array(
                                state.conversation,
                            ))
                            .unwrap(),
                        });
                }
                _ => unreachable!(),
            }
            assert!(
                crate::recovery::RecoveryPlan::validate_cleanup(&journal).is_err(),
                "{fault} must still refuse"
            );
        }
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

    /// wayland#1301: the quota scan sizes exact-digest published checkpoints from
    /// the directory listing and every other entry through a per-file open. The
    /// fixture holds a published checkpoint, an in-flight temporary, an
    /// unrecognised name and three near misses of the digest rule, all with
    /// distinct sizes, so a mis-classified or mis-sized entry changes the total.
    #[test]
    fn checkpoint_quota_sizes_only_exact_digest_names_from_the_listing() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let contents = b"published checkpoint";
        let digest = sha256_hex(contents);
        journal.store_effect_checkpoint(&digest, contents).unwrap();
        let directory = journal
            .effect_checkpoint_path(&digest)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();

        let temporary = format!(".{digest}.4242.in-flight.tmp");
        std::fs::write(directory.join(&temporary), vec![0_u8; 777]).unwrap();
        let unknown = "not-a-checkpoint".to_string();
        std::fs::write(directory.join(&unknown), vec![0_u8; 4096]).unwrap();
        // Case variants are exercised by name only in the classifier test below:
        // on a case-insensitive filesystem an uppercase copy would overwrite the
        // published file itself.
        let near_misses = [
            format!("{digest}.tmp"),
            format!("x{digest}"),
            digest[..63].to_string(),
        ];
        for (index, name) in near_misses.iter().enumerate() {
            std::fs::write(directory.join(name), vec![0_u8; 10 + index]).unwrap();
        }

        let _ = checkpoint_sizing_probe::take_opened();
        let total = checkpoint_directory_bytes(&directory).unwrap();
        let mut opened: Vec<String> = checkpoint_sizing_probe::take_opened()
            .into_iter()
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        opened.sort();
        let mut expected_opened = vec![temporary, unknown];
        expected_opened.extend(near_misses.iter().cloned());
        expected_opened.sort();
        assert_eq!(
            opened, expected_opened,
            "only the exact-digest published name may skip the per-file open"
        );

        // The implementation this replaced opened every entry.
        let mut reference = 0_u64;
        for entry in std::fs::read_dir(&directory).unwrap() {
            reference += std::fs::symlink_metadata(entry.unwrap().path())
                .unwrap()
                .len();
        }
        assert_eq!(total, reference);
        assert_eq!(total, contents.len() as u64 + 777 + 4096 + 10 + 11 + 12);
    }

    #[test]
    fn published_checkpoint_names_are_exact_lowercase_digests_only() {
        let digest = sha256_hex(b"any contents");
        assert!(is_published_checkpoint_name(std::ffi::OsStr::new(&digest)));
        for name in [
            digest.to_uppercase(),
            format!("{digest}.tmp"),
            format!(".{digest}.1.tmp"),
            format!("x{digest}"),
            format!("{digest}0"),
            digest[..63].to_string(),
            String::new(),
        ] {
            assert!(
                !is_published_checkpoint_name(std::ffi::OsStr::new(&name)),
                "{name:?} must not be classified as a published checkpoint"
            );
        }
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

    /// Store a small seed checkpoint and return the session's checkpoint
    /// directory, which the seed creates.
    fn seeded_checkpoint_directory(journal: &SessionJournal) -> PathBuf {
        let seed = b"seed";
        let seed_digest = sha256_hex(seed);
        journal.store_effect_checkpoint(&seed_digest, seed).unwrap();
        journal
            .effect_checkpoint_path(&seed_digest)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    /// Fill the checkpoint directory with an ordinary regular file, which the
    /// quota scan counts, until exactly `room` bytes of quota remain. `set_len`
    /// keeps the filler sparse where the filesystem can.
    fn leave_checkpoint_quota_room(directory: &Path, room: u64) {
        let used = checkpoint_directory_bytes(directory).unwrap();
        File::create(directory.join("filler"))
            .unwrap()
            .set_len(MAX_EFFECT_CHECKPOINT_SESSION_BYTES - used - room)
            .unwrap();
    }

    fn is_session_quota_refusal(result: &Result<(), JournalError>) -> bool {
        matches!(result, Err(JournalError::InvalidTransition(message)) if message.contains("session quota"))
    }

    /// wayland#1353: the session-quota scan and the write it admits must be one
    /// decision. Two stores into one checkpoint directory both finish the scan
    /// before either is decided, with room for exactly ONE maximum-size
    /// checkpoint: the directory must stay within the quota, one store must be
    /// accepted and the other refused by the quota.
    #[test]
    fn concurrent_checkpoint_stores_cannot_jointly_exceed_the_session_quota() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        leave_checkpoint_quota_room(
            &directory,
            MAX_EFFECT_CHECKPOINT_BYTES + MAX_EFFECT_CHECKPOINT_BYTES / 2,
        );

        let payload_len = usize::try_from(MAX_EFFECT_CHECKPOINT_BYTES).unwrap();
        let payloads = [vec![0x11_u8; payload_len], vec![0x22_u8; payload_len]];
        let digests: Vec<String> = payloads.iter().map(|payload| sha256_hex(payload)).collect();

        let gate = quota_race_gate::arm(&[&directory], 2);
        let handles: Vec<_> = payloads
            .into_iter()
            .map(|payload| {
                let journal = journal.clone();
                std::thread::spawn(move || {
                    let digest = sha256_hex(&payload);
                    journal.store_effect_checkpoint(&digest, &payload)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let rendezvous = gate.disarm();

        let total = checkpoint_directory_bytes(&directory).unwrap();
        eprintln!(
            "QUOTA_RACE rendezvous={rendezvous:?} total_bytes={total} \
             quota={MAX_EFFECT_CHECKPOINT_SESSION_BYTES} results={results:?}"
        );
        assert!(
            total <= MAX_EFFECT_CHECKPOINT_SESSION_BYTES,
            "two concurrent stores jointly exceeded the session quota: {total} > \
             {MAX_EFFECT_CHECKPOINT_SESSION_BYTES} bytes ({rendezvous:?}, {results:?})"
        );
        assert_eq!(
            rendezvous,
            quota_race_gate::Rendezvous {
                passed: 2,
                timed_out: 0
            },
            "both stores must finish the scan before either is decided, or this test proves nothing"
        );
        let accepted: Vec<usize> = (0..results.len())
            .filter(|index| results[*index].is_ok())
            .collect();
        assert_eq!(accepted.len(), 1, "exactly one store fits: {results:?}");
        assert_eq!(
            results
                .iter()
                .filter(|result| is_session_quota_refusal(result))
                .count(),
            1,
            "the other store must be refused by the session quota: {results:?}"
        );
        let winner = &digests[accepted[0]];
        assert_eq!(
            journal.load_effect_checkpoint(winner).unwrap().len(),
            payload_len
        );
        let loser = &digests[1 - accepted[0]];
        assert!(
            !directory.join(loser).exists(),
            "a refused store must leave no published checkpoint"
        );
    }

    /// wayland#1353: the released-bytes snapshot must be taken BEFORE the scan.
    /// Y finishes its listing and is parked before anything else it does; X then
    /// stores, publishes and ends its reservation completely; only then does Y
    /// decide. Y's listing cannot contain X and X holds no reservation, so the
    /// only thing that can count X for Y is a snapshot Y took before it listed.
    #[test]
    fn a_store_published_after_another_stores_scan_is_still_counted() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        leave_checkpoint_quota_room(
            &directory,
            MAX_EFFECT_CHECKPOINT_BYTES + MAX_EFFECT_CHECKPOINT_BYTES / 2,
        );
        let payload_len = usize::try_from(MAX_EFFECT_CHECKPOINT_BYTES).unwrap();
        let payload_x = vec![0x66_u8; payload_len];
        let payload_y = vec![0x77_u8; payload_len];
        let digest_x = sha256_hex(&payload_x);
        let digest_y = sha256_hex(&payload_y);

        let gate = quota_race_gate::park_first(&directory);
        let store_y = {
            let journal = journal.clone();
            let digest_y = digest_y.clone();
            std::thread::spawn(move || journal.store_effect_checkpoint(&digest_y, &payload_y))
        };
        assert!(
            gate.wait_until_parked(),
            "Y never reached the end of its scan"
        );
        journal
            .store_effect_checkpoint(&digest_x, &payload_x)
            .unwrap();
        assert!(directory.join(&digest_x).exists());
        assert_eq!(
            CheckpointQuota::lock(&journal.checkpoint_quota).reserved,
            0,
            "X must have ended its reservation before Y decides"
        );
        gate.release();
        let result_y = store_y.join().unwrap();
        let observed = gate.disarm();

        let total = checkpoint_directory_bytes(&directory).unwrap();
        assert!(
            total <= MAX_EFFECT_CHECKPOINT_SESSION_BYTES,
            "Y was admitted on a scan that could not see X: {total} > \
             {MAX_EFFECT_CHECKPOINT_SESSION_BYTES} bytes ({result_y:?})"
        );
        assert!(
            is_session_quota_refusal(&result_y),
            "Y must be refused by the session quota: {result_y:?}"
        );
        assert!(!directory.join(&digest_y).exists());
        assert_eq!(observed.timed_out, 0, "Y was never released: {observed:?}");
    }

    /// wayland#1353: a reservation may end only once its checkpoint is reachable
    /// under the published name. Ending it earlier (before the bytes are written,
    /// or after the write but before the hard link) opens a window in which a
    /// store that snapshots after the release and lists before the publication
    /// counts these bytes nowhere. Whether a listing misses them inside that
    /// window depends on directory-listing order, which a test cannot force, so
    /// the ordering itself is pinned: at the instant the reservation ends, the
    /// published checkpoint must already exist.
    #[test]
    fn a_checkpoint_reservation_ends_only_after_its_checkpoint_is_published() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        let payload = b"published before its reservation ends";
        let digest = sha256_hex(payload);
        let published = directory.join(&digest);

        let observed = std::rc::Rc::new(std::cell::Cell::new(None));
        {
            let observed = std::rc::Rc::clone(&observed);
            let published = published.clone();
            quota_race_gate::set_reservation_end_hook(move || {
                observed.set(Some(published.exists()));
            });
        }
        journal.store_effect_checkpoint(&digest, payload).unwrap();
        assert_eq!(
            observed.get(),
            Some(true),
            "the reservation ended before the checkpoint was published"
        );
    }

    /// wayland#1353 c2: the quota ledger belongs to one session's checkpoint
    /// directory. Two sessions, each with room for exactly one maximum-size
    /// checkpoint, store one concurrently. Both must reach the point after the
    /// scan together (nothing shared serializes them) and both must be accepted
    /// (a ledger shared across sessions would count the other's reservation and
    /// refuse one).
    #[test]
    fn checkpoint_stores_in_different_sessions_are_neither_serialized_nor_share_a_quota() {
        let dir = tempfile::tempdir().unwrap();
        let journals: Vec<SessionJournal> = ["a.journal", "b.journal"]
            .into_iter()
            .map(|name| SessionJournal::open(dir.path().join(name), name).unwrap())
            .collect();
        let directories: Vec<PathBuf> = journals.iter().map(seeded_checkpoint_directory).collect();
        assert_ne!(directories[0], directories[1]);
        for directory in &directories {
            leave_checkpoint_quota_room(
                directory,
                MAX_EFFECT_CHECKPOINT_BYTES + MAX_EFFECT_CHECKPOINT_BYTES / 2,
            );
        }

        let payload_len = usize::try_from(MAX_EFFECT_CHECKPOINT_BYTES).unwrap();
        let gate = quota_race_gate::arm(&[&directories[0], &directories[1]], 2);
        let handles: Vec<_> = journals
            .iter()
            .map(|journal| {
                let journal = journal.clone();
                std::thread::spawn(move || {
                    let payload = vec![0x33_u8; payload_len];
                    journal.store_effect_checkpoint(&sha256_hex(&payload), &payload)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let rendezvous = gate.disarm();

        assert_eq!(
            rendezvous,
            quota_race_gate::Rendezvous {
                passed: 2,
                timed_out: 0
            },
            "a store in one session waited for a store in another: {results:?}"
        );
        assert!(
            results.iter().all(Result::is_ok),
            "each session had room for its own checkpoint: {results:?}"
        );
        for (journal, directory) in journals.iter().zip(&directories) {
            assert!(
                checkpoint_directory_bytes(directory).unwrap()
                    <= MAX_EFFECT_CHECKPOINT_SESSION_BYTES
            );
            assert_eq!(CheckpointQuota::lock(&journal.checkpoint_quota).reserved, 0);
        }
    }

    /// wayland#1353 c3: a temporary left by a crashed store is still counted by
    /// the quota scan. It is named for a DIFFERENT digest, so the store's own
    /// stale-temporary cleanup does not remove it first.
    #[test]
    fn crash_left_checkpoint_temporaries_still_count_against_the_session_quota() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        leave_checkpoint_quota_room(&directory, MAX_EFFECT_CHECKPOINT_BYTES);

        let crashed = directory.join(format!(
            ".{}.4242.{}.tmp",
            sha256_hex(b"a store that crashed mid-write"),
            uuid::Uuid::new_v4()
        ));
        File::create(&crashed).unwrap().set_len(1).unwrap();

        let payload = vec![0x44_u8; usize::try_from(MAX_EFFECT_CHECKPOINT_BYTES).unwrap()];
        let digest = sha256_hex(&payload);
        let refused = journal.store_effect_checkpoint(&digest, &payload);
        assert!(
            is_session_quota_refusal(&refused),
            "one crash-left byte over the quota must refuse the store: {refused:?}"
        );
        assert!(
            crashed.exists(),
            "the crash-left temporary must not be removed"
        );
        assert!(!directory.join(&digest).exists());
        assert_eq!(CheckpointQuota::lock(&journal.checkpoint_quota).reserved, 0);

        std::fs::remove_file(&crashed).unwrap();
        journal.store_effect_checkpoint(&digest, &payload).unwrap();
        assert_eq!(
            checkpoint_directory_bytes(&directory).unwrap(),
            MAX_EFFECT_CHECKPOINT_SESSION_BYTES
        );
    }

    /// wayland#1353: a store that published and ended its reservation while
    /// another store was scanning is counted by that other store even though its
    /// (stale) scan missed the file, and it stops counting for any store that
    /// begins afterwards, whose scan does see the file.
    #[test]
    fn checkpoint_admission_counts_a_store_released_during_the_scan() {
        let ledger = Mutex::new(CheckpointQuota::default());
        let cap = MAX_EFFECT_CHECKPOINT_BYTES;
        let before_either = MAX_EFFECT_CHECKPOINT_SESSION_BYTES - cap - cap / 2;

        let mut slow = CheckpointAdmission::before_scan(&ledger);
        let mut fast = CheckpointAdmission::before_scan(&ledger);
        fast.admit(before_either, cap).unwrap();
        drop(fast);
        assert_eq!(CheckpointQuota::lock(&ledger).reserved, 0);
        assert!(
            slow.admit(before_either, cap).is_err(),
            "a store released during the scan must still be counted"
        );
        drop(slow);

        // A store that begins after `fast` published scans it, and must be able
        // to take exactly the room that is left: `fast` is not counted again.
        let mut later = CheckpointAdmission::before_scan(&ledger);
        later
            .admit(
                before_either + cap,
                MAX_EFFECT_CHECKPOINT_SESSION_BYTES - before_either - cap,
            )
            .unwrap();
        drop(later);
        assert_eq!(CheckpointQuota::lock(&ledger).reserved, 0);
    }

    /// wayland#1353 c3: a store that panics after it was admitted must not leak
    /// its reservation and wedge the session's quota. With room for exactly one
    /// maximum-size checkpoint, a leaked reservation would refuse the next one.
    #[test]
    fn panicking_checkpoint_store_releases_its_quota_reservation() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        let room = MAX_EFFECT_CHECKPOINT_BYTES + MAX_EFFECT_CHECKPOINT_BYTES / 2;
        leave_checkpoint_quota_room(&directory, room);

        let panicking = {
            let journal = journal.clone();
            let directory = directory.clone();
            std::thread::spawn(move || {
                let mut admission = CheckpointAdmission::before_scan(&journal.checkpoint_quota);
                admission
                    .admit(
                        checkpoint_directory_bytes(&directory).unwrap(),
                        MAX_EFFECT_CHECKPOINT_BYTES,
                    )
                    .unwrap();
                assert_eq!(
                    CheckpointQuota::lock(&journal.checkpoint_quota).reserved,
                    MAX_EFFECT_CHECKPOINT_BYTES
                );
                panic!("checkpoint store died mid-write");
            })
        };
        assert!(panicking.join().is_err());
        assert_eq!(CheckpointQuota::lock(&journal.checkpoint_quota).reserved, 0);

        let payload = vec![0x55_u8; usize::try_from(MAX_EFFECT_CHECKPOINT_BYTES).unwrap()];
        journal
            .store_effect_checkpoint(&sha256_hex(&payload), &payload)
            .unwrap();
    }

    /// Every `.{digest}.*.tmp` currently in `directory`.
    fn checkpoint_temporaries(directory: &Path, digest: &str) -> Vec<PathBuf> {
        let prefix = format!(".{digest}.");
        let mut found: Vec<PathBuf> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".tmp"))
            })
            .collect();
        found.sort();
        found
    }

    /// wayland#1357 c1: two stores of the SAME checkpoint. The first has written
    /// and synced its temporary and is parked just before its hard link; the
    /// second then runs to completion. The second's stale-temporary cleanup must
    /// not delete the first store's live temporary, or the first store's link
    /// fails NotFound.
    #[test]
    fn a_store_never_deletes_the_live_temporary_of_another_store_of_the_same_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        let payload: &'static [u8] = b"identical preimage stored twice at once";
        let digest = sha256_hex(payload);

        let gate = quota_race_gate::park_first_at(&directory, quota_race_gate::Point::BeforeLink);
        let first = {
            let journal = journal.clone();
            let digest = digest.clone();
            std::thread::spawn(move || journal.store_effect_checkpoint(&digest, payload))
        };
        assert!(
            gate.wait_until_parked(),
            "the first store never reached its link"
        );
        assert_eq!(checkpoint_temporaries(&directory, &digest).len(), 1);

        let second = journal.store_effect_checkpoint(&digest, payload);
        gate.release();
        let first = first.join().unwrap();
        let observed = gate.disarm();

        assert!(second.is_ok(), "{second:?}");
        assert!(
            first.is_ok(),
            "the second store deleted the first store's live temporary: {first:?}"
        );
        assert_eq!(observed.timed_out, 0, "{observed:?}");
        assert_eq!(journal.load_effect_checkpoint(&digest).unwrap(), payload);
        assert!(checkpoint_temporaries(&directory, &digest).is_empty());
    }

    /// wayland#1357 c1: loading a checkpoint that has a crash-left hard link
    /// removes that link, and must not remove the LIVE temporary of another store
    /// of the same checkpoint that has not linked yet.
    #[cfg(unix)]
    #[test]
    fn loading_a_checkpoint_never_deletes_the_live_temporary_of_a_store_not_yet_linked() {
        use std::os::unix::fs::OpenOptionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        let payload: &'static [u8] = b"published while another store is mid-write";
        let digest = sha256_hex(payload);

        let gate = quota_race_gate::park_first_at(&directory, quota_race_gate::Point::BeforeLink);
        let pending = {
            let journal = journal.clone();
            let digest = digest.clone();
            std::thread::spawn(move || journal.store_effect_checkpoint(&digest, payload))
        };
        assert!(
            gate.wait_until_parked(),
            "the pending store never reached its link"
        );

        // The same checkpoint is published beside it, with a crash-left alias.
        let published = directory.join(&digest);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&published)
            .unwrap()
            .write_all(payload)
            .unwrap();
        let crash_alias = directory.join(format!(".{digest}.crash.tmp"));
        std::fs::hard_link(&published, &crash_alias).unwrap();

        let loaded = journal.load_effect_checkpoint(&digest);
        gate.release();
        let pending = pending.join().unwrap();
        let observed = gate.disarm();

        assert_eq!(loaded.unwrap(), payload);
        assert!(
            !crash_alias.exists(),
            "the crash-left alias must still be removed"
        );
        assert!(
            pending.is_ok(),
            "loading deleted a live store's temporary: {pending:?}"
        );
        assert_eq!(observed.timed_out, 0, "{observed:?}");
        assert!(checkpoint_temporaries(&directory, &digest).is_empty());
    }

    /// wayland#1357 c1: a crash-left temporary that something else removes while
    /// this store's cleanup is part-way through it is already gone, not an error:
    /// once between the listing and the stat, once between the stat and the
    /// removal.
    #[test]
    fn a_temporary_removed_during_stale_temporary_cleanup_is_already_gone() {
        for point in [
            quota_race_gate::Point::CleanupListed,
            quota_race_gate::Point::CleanupStatted,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let journal =
                SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
            let directory = seeded_checkpoint_directory(&journal);
            let payload: &'static [u8] = b"stored while its crash-left temporary vanishes";
            let digest = sha256_hex(payload);
            let crashed = directory.join(format!(".{digest}.4242.{}.tmp", uuid::Uuid::new_v4()));
            std::fs::write(&crashed, b"partial").unwrap();

            let gate = quota_race_gate::park_first_at(&directory, point);
            let store = {
                let journal = journal.clone();
                let digest = digest.clone();
                std::thread::spawn(move || journal.store_effect_checkpoint(&digest, payload))
            };
            assert!(
                gate.wait_until_parked(),
                "{point:?}: the store never reached its cleanup"
            );
            std::fs::remove_file(&crashed).unwrap();
            gate.release();
            let result = store.join().unwrap();
            let observed = gate.disarm();

            assert!(
                result.is_ok(),
                "{point:?}: a vanished temporary failed the store: {result:?}"
            );
            assert_eq!(observed.timed_out, 0, "{point:?}: {observed:?}");
            assert_eq!(journal.load_effect_checkpoint(&digest).unwrap(), payload);
        }
    }

    /// wayland#1357 c1: an entry removed between this store's quota listing and
    /// its stat is gone and counts as gone. Room is left so the store fits ONLY
    /// if the vanished 4,096-byte entry is not counted; an error, or a total
    /// that still counts it, both fail the store.
    #[test]
    fn an_entry_removed_during_the_quota_scan_counts_as_gone() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        let other = directory.join(format!(
            ".{}.4242.{}.tmp",
            sha256_hex(b"another checkpoint"),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&other, vec![0_u8; 4096]).unwrap();
        let payload: &'static [u8] = b"stored while another temporary vanishes";
        let digest = sha256_hex(payload);
        leave_checkpoint_quota_room(&directory, payload.len() as u64 - 1);

        let gate = quota_race_gate::park_first_at(&directory, quota_race_gate::Point::ScanListed);
        let store = {
            let journal = journal.clone();
            let digest = digest.clone();
            std::thread::spawn(move || journal.store_effect_checkpoint(&digest, payload))
        };
        assert!(
            gate.wait_until_parked(),
            "the store never listed the other temporary"
        );
        std::fs::remove_file(&other).unwrap();
        gate.release();
        let result = store.join().unwrap();
        let observed = gate.disarm();

        assert!(
            result.is_ok(),
            "an entry removed during the scan failed the store: {result:?}"
        );
        assert_eq!(observed.timed_out, 0, "{observed:?}");
        assert_eq!(journal.load_effect_checkpoint(&digest).unwrap(), payload);
    }

    /// wayland#1357: a reader that loads a checkpoint while the store that just
    /// published it still holds its temporary (by then a second hard link to the
    /// same file) removes that redundant link instead of refusing the checkpoint.
    /// Green before the repair; it pins that the repair still deletes a live
    /// temporary when it is only a link to the published checkpoint.
    #[cfg(unix)]
    #[test]
    fn loading_a_checkpoint_while_its_store_still_links_its_temporary_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let directory = seeded_checkpoint_directory(&journal);
        let payload: &'static [u8] = b"loaded while its own temporary still links it";
        let digest = sha256_hex(payload);

        let gate = quota_race_gate::park_first_at(&directory, quota_race_gate::Point::AfterLink);
        let store = {
            let journal = journal.clone();
            let digest = digest.clone();
            std::thread::spawn(move || journal.store_effect_checkpoint(&digest, payload))
        };
        assert!(gate.wait_until_parked(), "the store never linked");
        let loaded = journal.load_effect_checkpoint(&digest);
        gate.release();
        let stored = store.join().unwrap();
        let observed = gate.disarm();

        assert_eq!(loaded.unwrap(), payload);
        assert!(stored.is_ok(), "{stored:?}");
        assert_eq!(observed.timed_out, 0, "{observed:?}");
        assert!(checkpoint_temporaries(&directory, &digest).is_empty());
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
