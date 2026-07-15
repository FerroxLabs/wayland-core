//! Crash-safe, append-only session execution journal.
//!
//! Records form a versioned SHA-256 chain independent of session snapshots.
//! Started external effects recover as [`ExternalEffectState::Unknown`], never
//! as safe to repeat.

use std::collections::BTreeMap;
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
mod snapshot;
pub use snapshot::*;

pub const SESSION_JOURNAL_SCHEMA_VERSION: u32 = 3;
pub const GENESIS_CHECKSUM: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
const FRAME_MAGIC: &[u8; 4] = b"WJ01";
const FRAME_HEADER_BYTES: usize = 12;
const FRAME_DIGEST_BYTES: usize = 32;
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[error("snapshot cursor does not match its reduced state")]
    SnapshotCursorMismatch,
    #[error("snapshot and journal do not describe the same authority: {0}")]
    SnapshotJournalMismatch(String),
    #[error("compacted journal begins at sequence {first_seq} but its snapshot is missing")]
    CompactedJournalMissingSnapshot { first_seq: u64 },
    #[error("session journal writer lock is poisoned")]
    WriterPoisoned,
    #[error("session journal writer is faulted after a previous I/O failure")]
    WriterFaulted,
    #[error("session journal writer lease is already held at {lease_path}")]
    AlreadyOwned { lease_path: PathBuf },
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
    faulted: bool,
    _lease: WriterLease,
}

type SharedWriter = Arc<Mutex<JournalWriter>>;

#[derive(Debug, Clone)]
pub struct SessionJournal {
    inner: SharedWriter,
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

    pub fn state(&self) -> Result<ReducedSessionState, JournalError> {
        self.inner
            .lock()
            .map_err(|_| JournalError::WriterPoisoned)
            .map(|writer| writer.state.clone())
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

    /// Replay and verify all complete records. An unterminated final fragment is
    /// ignored; opening the writer heals that fragment before the next append.
    pub fn replay(path: impl AsRef<Path>) -> Result<Vec<JournalEnvelope>, JournalError> {
        let path = path.as_ref();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(JournalError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let (entries, _) = parse_complete_frames(path, &bytes)?;
        let snapshot = snapshot::load_snapshot_if_present(snapshot_path_for(path))?;
        recover_storage(&entries, snapshot.as_ref(), None)?;
        Ok(entries)
    }

    /// Recover the complete committed state from a full log or a validated
    /// companion snapshot plus compacted suffix.
    pub fn recovered_state(path: impl AsRef<Path>) -> Result<ReducedSessionState, JournalError> {
        let path = path.as_ref();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(JournalError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let (entries, _) = parse_complete_frames(path, &bytes)?;
        let snapshot = snapshot::load_snapshot_if_present(snapshot_path_for(path))?;
        recover_storage(&entries, snapshot.as_ref(), None).map(|recovery| recovery.state)
    }

    pub fn lease_owner(path: impl AsRef<Path>) -> Result<LeaseOwner, JournalError> {
        lease::inspect(&lease::normalized_path(path.as_ref())?)
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
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|source| JournalError::Io {
            path: path.clone(),
            source,
        })?;
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
        let (entries, valid_len) = parse_complete_frames(&path, &bytes)?;
        let snapshot = snapshot::load_snapshot_if_present(snapshot_path_for(&path))?;
        let recovery = recover_storage(&entries, snapshot.as_ref(), Some(&session_id))?;
        if valid_len < bytes.len() {
            file.set_len(valid_len as u64)
                .and_then(|()| file.sync_all())
                .map_err(|source| JournalError::Io {
                    path: path.clone(),
                    source,
                })?;
        }
        file.seek(SeekFrom::End(0))
            .map_err(|source| JournalError::Io {
                path: path.clone(),
                source,
            })?;
        Ok(Self {
            path,
            session_id,
            file,
            next_seq: recovery.next_seq,
            previous_checksum: recovery.previous_checksum,
            state: recovery.state,
            last_envelope: recovery.last_envelope,
            faulted: false,
            _lease: lease,
        })
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
        self.next_seq += 1;
        self.previous_checksum.clone_from(&envelope.checksum);
        self.state = candidate_state;
        self.last_envelope = Some(envelope.clone());
        Ok(envelope)
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
        let snapshot = SessionSnapshot::new(self.session_id.clone(), self.state.clone())?;
        let snapshot_path = snapshot_path_for(&self.path);
        write_snapshot(&snapshot_path, &snapshot)?;

        let replacement = match self.last_envelope.as_ref() {
            Some(anchor) => {
                let body = serde_json::to_vec(anchor).map_err(|source| JournalError::Json {
                    context: "encoding compacted journal anchor",
                    source,
                })?;
                encode_frame(&body)?
            }
            None => Vec::new(),
        };
        // `persist` is an atomic replacement on supported tempfile platforms.
        // There is deliberately no remove-then-rename fallback: that would
        // create an authority gap on Windows and violate the journal contract.
        self.file = snapshot::replace_file_atomically(&self.path, &replacement)?;
        snapshot::sync_parent_directory(&self.path)?;
        self.file
            .seek(SeekFrom::End(0))
            .map_err(|source| JournalError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(snapshot)
    }
}

struct StorageRecovery {
    state: ReducedSessionState,
    next_seq: u64,
    previous_checksum: String,
    last_envelope: Option<JournalEnvelope>,
}

fn recover_storage(
    entries: &[JournalEnvelope],
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

    let state = match (snapshot, entries.first()) {
        (None, None) => ReducedSessionState::default(),
        (None, Some(first)) if first.seq == 0 => {
            verify_chain_for_session(entries, expected_session)?;
            replay_state(entries)?
        }
        (None, Some(first)) => {
            return Err(JournalError::CompactedJournalMissingSnapshot {
                first_seq: first.seq,
            });
        }
        (Some(snapshot), None) => snapshot.state.clone(),
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
            if prefix_state != snapshot.state {
                return Err(JournalError::SnapshotJournalMismatch(
                    "snapshot state does not equal its full-log prefix".to_owned(),
                ));
            }
            replay_from_snapshot(snapshot, &entries[prefix_len..])?
        }
        (Some(snapshot), Some(first)) => {
            let suffix_start = snapshot.cursor.map_or(0, |cursor| cursor.saturating_add(1));
            let suffix = if snapshot.cursor == Some(first.seq) {
                if first.session_id != snapshot.session_id {
                    return Err(JournalError::SessionMismatch {
                        expected: snapshot.session_id.clone(),
                        found: first.session_id.clone(),
                    });
                }
                if first.checksum != snapshot.cursor_checksum
                    || first.computed_checksum()? != first.checksum
                {
                    return Err(JournalError::SnapshotJournalMismatch(format!(
                        "compaction anchor does not match snapshot sequence {}",
                        first.seq
                    )));
                }
                verify_chain_from(
                    &entries[1..],
                    suffix_start,
                    &snapshot.cursor_checksum,
                    &snapshot.session_id,
                )?;
                &entries[1..]
            } else if first.seq == suffix_start {
                verify_chain_from(
                    entries,
                    suffix_start,
                    &snapshot.cursor_checksum,
                    &snapshot.session_id,
                )?;
                entries
            } else {
                return Err(JournalError::SnapshotJournalMismatch(format!(
                    "snapshot cursor {:?} cannot seed journal sequence {}",
                    snapshot.cursor, first.seq
                )));
            };
            replay_from_snapshot(snapshot, suffix)?
        }
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
    for (offset, entry) in entries.iter().enumerate() {
        if entry.schema_version != SESSION_JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchema {
                found: entry.schema_version,
                supported: SESSION_JOURNAL_SCHEMA_VERSION,
            });
        }
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

fn parse_complete_frames(
    path: &Path,
    bytes: &[u8],
) -> Result<(Vec<JournalEnvelope>, usize), JournalError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    let mut frame_number = 1;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.len() < FRAME_HEADER_BYTES {
            break;
        }
        if &remaining[..4] != FRAME_MAGIC {
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
        if length > MAX_FRAME_BYTES {
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
        let schema = serde_json::from_slice::<EnvelopeSchema>(body).map_err(|source| {
            JournalError::CorruptFrame {
                path: path.to_path_buf(),
                frame: frame_number,
                source,
            }
        })?;
        if schema.schema_version != SESSION_JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchema {
                found: schema.schema_version,
                supported: SESSION_JOURNAL_SCHEMA_VERSION,
            });
        }
        let entry = serde_json::from_slice(body).map_err(|source| JournalError::CorruptFrame {
            path: path.to_path_buf(),
            frame: frame_number,
            source,
        })?;
        entries.push(entry);
        offset += frame_len;
        frame_number += 1;
    }
    Ok((entries, offset))
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

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

impl ReducedSessionState {
    pub fn digest(&self) -> Result<String, JournalError> {
        let bytes = serde_json::to_vec(self).map_err(|source| JournalError::Json {
            context: "encoding reduced state",
            source,
        })?;
        Ok(sha256_hex(&bytes))
    }
}

pub fn reduce(
    mut state: ReducedSessionState,
    envelope: &JournalEnvelope,
) -> Result<ReducedSessionState, JournalError> {
    let expected_seq = state.last_seq.map_or(0, |seq| seq + 1);
    if envelope.schema_version != SESSION_JOURNAL_SCHEMA_VERSION {
        return Err(JournalError::UnsupportedSchema {
            found: envelope.schema_version,
            supported: SESSION_JOURNAL_SCHEMA_VERSION,
        });
    }
    match state.session_id.as_deref() {
        Some(expected) if expected != envelope.session_id => {
            return Err(JournalError::SessionMismatch {
                expected: expected.to_owned(),
                found: envelope.session_id.clone(),
            });
        }
        None => state.session_id = Some(envelope.session_id.clone()),
        _ => {}
    }
    if envelope.seq != expected_seq {
        return Err(JournalError::SequenceMismatch {
            expected: expected_seq,
            found: envelope.seq,
        });
    }
    if envelope.previous_checksum != state.last_checksum {
        return Err(JournalError::PreviousChecksumMismatch { seq: envelope.seq });
    }
    if envelope.computed_checksum()? != envelope.checksum {
        return Err(JournalError::ChecksumMismatch { seq: envelope.seq });
    }
    apply_event(&mut state, &envelope.event)?;
    state.last_seq = Some(envelope.seq);
    state.last_checksum.clone_from(&envelope.checksum);
    Ok(state)
}

pub fn replay_state(entries: &[JournalEnvelope]) -> Result<ReducedSessionState, JournalError> {
    entries
        .iter()
        .try_fold(ReducedSessionState::default(), reduce)
}

fn duplicate(kind: &str, id: &str) -> JournalError {
    JournalError::InvalidTransition(format!("duplicate {kind} id {id}"))
}

fn missing(kind: &str, id: &str) -> JournalError {
    JournalError::InvalidTransition(format!("unknown {kind} id {id}"))
}

fn required_mut<'a, T>(
    map: &'a mut BTreeMap<String, T>,
    kind: &str,
    id: &str,
) -> Result<&'a mut T, JournalError> {
    map.get_mut(id).ok_or_else(|| missing(kind, id))
}

fn require_prepared(
    effect: &ExternalEffectState,
    kind: &str,
    id: &str,
) -> Result<(), JournalError> {
    if matches!(effect, ExternalEffectState::Prepared) {
        Ok(())
    } else {
        Err(JournalError::InvalidTransition(format!(
            "{kind} {id} was not prepared"
        )))
    }
}

fn require_unknown(effect: &ExternalEffectState, kind: &str, id: &str) -> Result<(), JournalError> {
    if matches!(effect, ExternalEffectState::Unknown) {
        Ok(())
    } else {
        Err(JournalError::InvalidTransition(format!(
            "{kind} {id} has no unresolved started effect"
        )))
    }
}

fn require_active_turn(state: &ReducedSessionState, turn_id: &str) -> Result<(), JournalError> {
    let turn = state
        .turns
        .get(turn_id)
        .ok_or_else(|| missing("turn", turn_id))?;
    if turn.completion.is_some() {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} is terminal"
        )));
    }
    Ok(())
}

fn require_approval_origin_prepared(
    state: &ReducedSessionState,
    origin: &ApprovalOrigin,
) -> Result<(), JournalError> {
    match origin {
        ApprovalOrigin::Turn { turn_id } => require_active_turn(state, turn_id),
        ApprovalOrigin::ProviderAttempt { attempt_id } => {
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            require_active_turn(state, &attempt.turn_id)?;
            require_prepared(&attempt.effect, "provider attempt", attempt_id)
        }
        ApprovalOrigin::ToolExecution { tool_execution_id } => {
            let tool = state
                .tools
                .get(tool_execution_id)
                .ok_or_else(|| missing("tool execution", tool_execution_id))?;
            require_active_turn(state, &tool.turn_id)?;
            require_prepared(&tool.effect, "tool execution", tool_execution_id)
        }
        ApprovalOrigin::Child { child_id } => {
            let child = state
                .children
                .get(child_id)
                .ok_or_else(|| missing("child", child_id))?;
            require_active_turn(state, &child.turn_id)?;
            require_prepared(&child.effect, "child", child_id)
        }
        ApprovalOrigin::Delivery { delivery_id } => {
            let delivery = state
                .deliveries
                .get(delivery_id)
                .ok_or_else(|| missing("delivery", delivery_id))?;
            require_delivery_origin_active(state, &delivery.origin)?;
            require_prepared(&delivery.effect, "delivery", delivery_id)
        }
    }
}

fn require_budget_owner_exists(
    state: &ReducedSessionState,
    owner: &BudgetOwner,
) -> Result<(), JournalError> {
    match owner {
        BudgetOwner::Session => Ok(()),
        BudgetOwner::Turn { turn_id } => require_active_turn(state, turn_id),
        BudgetOwner::ProviderAttempt { attempt_id } => state
            .provider_attempts
            .contains_key(attempt_id)
            .then_some(())
            .ok_or_else(|| missing("provider attempt", attempt_id)),
        BudgetOwner::ToolExecution { tool_execution_id } => state
            .tools
            .contains_key(tool_execution_id)
            .then_some(())
            .ok_or_else(|| missing("tool execution", tool_execution_id)),
        BudgetOwner::Child { child_id } => state
            .children
            .contains_key(child_id)
            .then_some(())
            .ok_or_else(|| missing("child", child_id)),
    }
}

fn require_delivery_origin_active(
    state: &ReducedSessionState,
    origin: &DeliveryOrigin,
) -> Result<(), JournalError> {
    match origin {
        DeliveryOrigin::Turn { turn_id } => require_active_turn(state, turn_id),
        DeliveryOrigin::InboundReply { .. } | DeliveryOrigin::Cron { .. } => Ok(()),
    }
}

pub fn state_payload_digest(value: &serde_json::Value) -> Result<String, JournalError> {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(canonical).collect())
            }
            serde_json::Value::Object(values) => serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            scalar => scalar.clone(),
        }
    }
    let bytes = serde_json::to_vec(&canonical(value)).map_err(|source| JournalError::Json {
        context: "encoding checkpoint state",
        source,
    })?;
    Ok(sha256_hex(&bytes))
}

pub fn provider_request_digest(
    request: &wcore_types::llm::LlmRequest,
) -> Result<String, JournalError> {
    let thinking = request.thinking.as_ref().map(|thinking| match thinking {
        wcore_types::llm::ThinkingConfig::Enabled { budget_tokens } => serde_json::json!({
            "mode": "enabled",
            "budget_tokens": budget_tokens,
        }),
        wcore_types::llm::ThinkingConfig::Disabled => {
            serde_json::json!({ "mode": "disabled" })
        }
    });
    let tools = request
        .tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": &tool.name,
                "description": &tool.description,
                "input_schema": &tool.input_schema,
                "deferred": tool.deferred,
                "server": &tool.server,
            })
        })
        .collect::<Vec<_>>();
    let request_value = serde_json::json!({
        "model": &request.model,
        "system": &request.system,
        "messages": &request.messages,
        "tools": tools,
        "max_tokens": request.max_tokens,
        "thinking": thinking,
        "reasoning_effort": &request.reasoning_effort,
        "cache_tier": request.cache_tier.map(|tier| tier.as_str()),
        "routing_hint": request.routing_hint.as_ref().map(|hint| &hint.0),
        "stop_sequences": &request.stop_sequences,
        "web_search": request.web_search,
        "conversation_id": &request.conversation_id,
        "client_context_tokens": request.client_context_tokens,
        "temperature": request.temperature,
        "omit_max_tokens": request.omit_max_tokens,
    });
    state_payload_digest(&request_value)
}

fn apply_event(state: &mut ReducedSessionState, event: &SessionEvent) -> Result<(), JournalError> {
    match event {
        SessionEvent::SessionImported {
            source_schema_version,
            session,
            session_digest,
        } => {
            let pristine = state.last_seq.is_none()
                && state.imported_baseline.is_none()
                && state.conversation.is_empty()
                && state.turns.is_empty()
                && state.streams.is_empty()
                && state.provider_attempts.is_empty()
                && state.tools.is_empty()
                && state.approvals.is_empty()
                && state.budgets.is_empty()
                && state.budget_event_ids.is_empty()
                && state.checkpoints.is_empty()
                && state.children.is_empty()
                && state.deliveries.is_empty();
            if !pristine {
                return Err(JournalError::InvalidTransition(
                    "session import must be the first event".to_owned(),
                ));
            }
            let object = session.as_object().ok_or_else(|| {
                JournalError::InvalidTransition("imported session must be an object".to_owned())
            })?;
            let imported_id = object
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "imported session id must be a string".to_owned(),
                    )
                })?;
            let expected_id = state.session_id.as_deref().unwrap_or_default();
            if imported_id != expected_id {
                return Err(JournalError::SessionMismatch {
                    expected: expected_id.to_owned(),
                    found: imported_id.to_owned(),
                });
            }
            match object
                .get("schema_version")
                .and_then(serde_json::Value::as_u64)
            {
                Some(version) if version == u64::from(*source_schema_version) => {}
                None if *source_schema_version == 0 => {}
                _ => {
                    return Err(JournalError::InvalidTransition(
                        "imported session schema version mismatch".to_owned(),
                    ));
                }
            }
            let messages = object
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "imported session messages must be an array".to_owned(),
                    )
                })?;
            if messages.iter().any(|message| !message.is_object()) {
                return Err(JournalError::InvalidTransition(
                    "every imported session message must be an object".to_owned(),
                ));
            }
            if state_payload_digest(session)? != *session_digest {
                return Err(JournalError::InvalidTransition(
                    "imported session digest mismatch".to_owned(),
                ));
            }
            state.conversation.clone_from(messages);
            state.imported_baseline = Some(ImportedSessionBaseline {
                source_schema_version: *source_schema_version,
                session_digest: session_digest.clone(),
                imported_message_count: messages.len() as u64,
                session: session.clone(),
            });
        }
        SessionEvent::ConversationMessageCommitted {
            turn_id,
            message_index,
            message,
            message_digest,
        } => {
            let turn = state
                .turns
                .get(turn_id)
                .ok_or_else(|| missing("turn", turn_id))?;
            if turn.completion.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} is terminal"
                )));
            }
            let index = usize::try_from(*message_index).map_err(|_| {
                JournalError::InvalidTransition("conversation message index overflow".to_owned())
            })?;
            if index != state.conversation.len() {
                return Err(JournalError::InvalidTransition(format!(
                    "conversation expected index {}, found {message_index}",
                    state.conversation.len()
                )));
            }
            if !message.is_object() {
                return Err(JournalError::InvalidTransition(
                    "conversation message must be an object".to_owned(),
                ));
            }
            if state_payload_digest(message)? != *message_digest {
                return Err(JournalError::InvalidTransition(
                    "conversation message digest mismatch".to_owned(),
                ));
            }
            state.conversation.push(message.clone());
        }
        SessionEvent::ConversationStateCommitted {
            turn_id,
            messages,
            messages_digest,
        } => {
            let turn = state
                .turns
                .get(turn_id)
                .ok_or_else(|| missing("turn", turn_id))?;
            if turn.completion.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} is terminal"
                )));
            }
            if messages.iter().any(|message| !message.is_object()) {
                return Err(JournalError::InvalidTransition(
                    "every conversation state message must be an object".to_owned(),
                ));
            }
            let payload = serde_json::Value::Array(messages.clone());
            if state_payload_digest(&payload)? != *messages_digest {
                return Err(JournalError::InvalidTransition(
                    "conversation state digest mismatch".to_owned(),
                ));
            }
            state.conversation.clone_from(messages);
        }
        SessionEvent::TurnStarted {
            turn_id,
            user_message,
        } => {
            if state.turns.contains_key(turn_id) {
                return Err(duplicate("turn", turn_id));
            }
            if let Some((active_turn_id, _)) = state
                .turns
                .iter()
                .find(|(_, turn)| turn.completion.is_none())
            {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {active_turn_id} is still active"
                )));
            }
            state.turns.insert(
                turn_id.clone(),
                TurnState {
                    user_message: user_message.clone(),
                    completion: None,
                },
            );
        }
        SessionEvent::TurnCommitted {
            turn_id,
            assistant_message,
        } => {
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Committed {
                assistant_message: assistant_message.clone(),
            });
        }
        SessionEvent::TurnFailed { turn_id, error } => {
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Failed {
                error: error.clone(),
            });
        }
        SessionEvent::TurnCancelled { turn_id } => {
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Cancelled);
        }
        SessionEvent::StreamStarted {
            stream_id,
            attempt_id,
        } => {
            if state.streams.contains_key(stream_id) {
                return Err(duplicate("stream", stream_id));
            }
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            require_unknown(&attempt.effect, "provider attempt", attempt_id)?;
            if state
                .streams
                .values()
                .any(|stream| stream.attempt_id == *attempt_id)
            {
                return Err(JournalError::InvalidTransition(format!(
                    "provider attempt {attempt_id} already has a stream"
                )));
            }
            state.streams.insert(
                stream_id.clone(),
                StreamState {
                    attempt_id: attempt_id.clone(),
                    next_ordinal: 0,
                    batches: Vec::new(),
                    finished: false,
                },
            );
        }
        SessionEvent::StreamBatchCommitted {
            stream_id,
            ordinal,
            events,
        } => {
            let attempt_id = state
                .streams
                .get(stream_id)
                .ok_or_else(|| missing("stream", stream_id))?
                .attempt_id
                .clone();
            let attempt = state
                .provider_attempts
                .get(&attempt_id)
                .ok_or_else(|| missing("provider attempt", &attempt_id))?;
            require_unknown(&attempt.effect, "provider attempt", &attempt_id)?;
            let stream = required_mut(&mut state.streams, "stream", stream_id)?;
            if stream.finished || *ordinal != stream.next_ordinal {
                return Err(JournalError::InvalidTransition(format!(
                    "stream {stream_id} expected batch {}, found {ordinal}",
                    stream.next_ordinal
                )));
            }
            if events.is_empty() {
                return Err(JournalError::InvalidTransition(format!(
                    "stream {stream_id} batch {ordinal} is empty"
                )));
            }
            stream.batches.push(events.clone());
            stream.next_ordinal += 1;
        }
        SessionEvent::StreamFinished { stream_id } => {
            let attempt_id = state
                .streams
                .get(stream_id)
                .ok_or_else(|| missing("stream", stream_id))?
                .attempt_id
                .clone();
            let attempt = state
                .provider_attempts
                .get(&attempt_id)
                .ok_or_else(|| missing("provider attempt", &attempt_id))?;
            require_unknown(&attempt.effect, "provider attempt", &attempt_id)?;
            let stream = required_mut(&mut state.streams, "stream", stream_id)?;
            if stream.finished {
                return Err(duplicate("stream completion", stream_id));
            }
            stream.finished = true;
        }
        SessionEvent::ProviderAttemptPrepared {
            attempt_id,
            turn_id,
            purpose,
            provider,
            model,
            request_digest,
        } => {
            require_active_turn(state, turn_id)?;
            if state.provider_attempts.contains_key(attempt_id) {
                return Err(duplicate("provider attempt", attempt_id));
            }
            state.provider_attempts.insert(
                attempt_id.clone(),
                ProviderAttemptState {
                    turn_id: turn_id.clone(),
                    purpose: *purpose,
                    provider: provider.clone(),
                    model: model.clone(),
                    request_digest: request_digest.clone(),
                    response_digest: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::ProviderAttemptStarted { attempt_id } => {
            let turn_id = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            require_prepared(&attempt.effect, "provider attempt", attempt_id)?;
            attempt.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::ProviderAttemptFinished {
            attempt_id,
            outcome,
            response_digest,
        } => {
            if matches!(outcome, CompletionOutcome::Succeeded) {
                let stream = state
                    .streams
                    .values()
                    .find(|stream| stream.attempt_id == *attempt_id)
                    .ok_or_else(|| {
                        JournalError::InvalidTransition(format!(
                            "successful provider attempt {attempt_id} has no stream"
                        ))
                    })?;
                if !stream.finished {
                    return Err(JournalError::InvalidTransition(format!(
                        "successful provider attempt {attempt_id} has an unfinished stream"
                    )));
                }
            }
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            require_unknown(&attempt.effect, "provider attempt", attempt_id)?;
            attempt.response_digest.clone_from(response_digest);
            attempt.effect = ExternalEffectState::Completed {
                outcome: outcome.clone(),
            };
        }
        SessionEvent::ProviderAttemptNotStarted { attempt_id, reason } => {
            let turn_id = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            require_prepared(&attempt.effect, "provider attempt", attempt_id)?;
            attempt.not_started_reason = Some(reason.clone());
            attempt.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::ToolIntentRecorded {
            tool_execution_id,
            provider_call_id,
            turn_id,
            ordinal,
            tool,
            requested_input,
            requested_input_digest,
            effective_input,
            effective_input_digest,
        } => {
            require_active_turn(state, turn_id)?;
            if state.tools.contains_key(tool_execution_id) {
                return Err(duplicate("tool execution", tool_execution_id));
            }
            if state.tools.values().any(|existing| {
                existing.turn_id == *turn_id && existing.provider_call_id == *provider_call_id
            }) {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} already has provider tool call {provider_call_id}"
                )));
            }
            if state
                .tools
                .values()
                .any(|existing| existing.turn_id == *turn_id && existing.ordinal == *ordinal)
            {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} already has tool execution ordinal {ordinal}"
                )));
            }
            if state_payload_digest(requested_input)? != *requested_input_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} requested input digest mismatch"
                )));
            }
            if state_payload_digest(effective_input)? != *effective_input_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} effective input digest mismatch"
                )));
            }
            state.tools.insert(
                tool_execution_id.clone(),
                ToolState {
                    provider_call_id: provider_call_id.clone(),
                    turn_id: turn_id.clone(),
                    ordinal: *ordinal,
                    tool: tool.clone(),
                    requested_input: requested_input.clone(),
                    requested_input_digest: requested_input_digest.clone(),
                    effective_input: effective_input.clone(),
                    effective_input_digest: effective_input_digest.clone(),
                    result: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::ToolExecutionStarted { tool_execution_id } => {
            let turn_id = state
                .tools
                .get(tool_execution_id)
                .ok_or_else(|| missing("tool execution", tool_execution_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_prepared(&tool.effect, "tool execution", tool_execution_id)?;
            tool.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::ToolExecutionFinished {
            tool_execution_id,
            outcome,
            result,
        } => {
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_unknown(&tool.effect, "tool execution", tool_execution_id)?;
            tool.result = Some(result.clone());
            tool.effect = ExternalEffectState::Completed {
                outcome: outcome.clone(),
            };
        }
        SessionEvent::ToolExecutionNotStarted {
            tool_execution_id,
            reason,
        } => {
            let turn_id = state
                .tools
                .get(tool_execution_id)
                .ok_or_else(|| missing("tool execution", tool_execution_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_prepared(&tool.effect, "tool execution", tool_execution_id)?;
            tool.not_started_reason = Some(reason.clone());
            tool.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::ApprovalRequested {
            approval_id,
            origin,
            intent_digest,
        } => {
            require_approval_origin_prepared(state, origin)?;
            if state.approvals.contains_key(approval_id) {
                return Err(duplicate("approval", approval_id));
            }
            if state
                .approvals
                .values()
                .any(|approval| approval.origin == *origin)
            {
                return Err(JournalError::InvalidTransition(format!(
                    "approval origin {origin:?} already has an approval"
                )));
            }
            state.approvals.insert(
                approval_id.clone(),
                ApprovalState {
                    origin: origin.clone(),
                    intent_digest: intent_digest.clone(),
                    resolution: None,
                },
            );
        }
        SessionEvent::ApprovalResolved {
            approval_id,
            resolution,
        } => {
            let approval = state
                .approvals
                .get(approval_id)
                .ok_or_else(|| missing("approval", approval_id))?;
            if approval.resolution.is_some() {
                return Err(duplicate("approval resolution", approval_id));
            }
            let origin = approval.origin.clone();
            require_approval_origin_prepared(state, &origin)?;
            let approval = required_mut(&mut state.approvals, "approval", approval_id)?;
            approval.resolution = Some(resolution.clone());
        }
        SessionEvent::BudgetReserved {
            event_id,
            reservation_id,
            owner,
            purpose,
            amount,
        } => {
            if state.budget_event_ids.contains_key(event_id) {
                return Err(duplicate("budget event", event_id));
            }
            if state.budgets.contains_key(reservation_id) {
                return Err(duplicate("budget reservation", reservation_id));
            }
            require_budget_owner_exists(state, owner)?;
            if amount.value == 0 {
                return Err(JournalError::InvalidTransition(format!(
                    "budget reservation {reservation_id} amount must be nonzero"
                )));
            }
            state.budgets.insert(
                reservation_id.clone(),
                BudgetState {
                    owner: owner.clone(),
                    purpose: *purpose,
                    reserved: *amount,
                    used: None,
                    released: false,
                    event_ids: vec![event_id.clone()],
                },
            );
            state
                .budget_event_ids
                .insert(event_id.clone(), reservation_id.clone());
        }
        SessionEvent::BudgetSettled {
            event_id,
            reservation_id,
            amount,
        } => {
            if state.budget_event_ids.contains_key(event_id) {
                return Err(duplicate("budget event", event_id));
            }
            let budget = required_mut(&mut state.budgets, "budget reservation", reservation_id)?;
            if budget.used.is_some()
                || budget.released
                || amount.unit != budget.reserved.unit
                || amount.value > budget.reserved.value
            {
                return Err(duplicate("budget settlement", reservation_id));
            }
            budget.used = Some(*amount);
            budget.event_ids.push(event_id.clone());
            state
                .budget_event_ids
                .insert(event_id.clone(), reservation_id.clone());
        }
        SessionEvent::BudgetReleased {
            event_id,
            reservation_id,
        } => {
            if state.budget_event_ids.contains_key(event_id) {
                return Err(duplicate("budget event", event_id));
            }
            let budget = required_mut(&mut state.budgets, "budget reservation", reservation_id)?;
            if budget.used.is_some() || budget.released {
                return Err(duplicate("budget release", reservation_id));
            }
            budget.released = true;
            budget.event_ids.push(event_id.clone());
            state
                .budget_event_ids
                .insert(event_id.clone(), reservation_id.clone());
        }
        SessionEvent::CheckpointCommitted {
            checkpoint_id,
            purpose,
            origin,
            state_digest,
            state: checkpoint,
        } => {
            if let CheckpointOrigin::Turn { turn_id } = origin
                && !state.turns.contains_key(turn_id)
            {
                return Err(missing("turn", turn_id));
            }
            if state_payload_digest(checkpoint)? != *state_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "checkpoint {checkpoint_id} state digest mismatch"
                )));
            }
            if state
                .checkpoints
                .insert(
                    checkpoint_id.clone(),
                    CheckpointState {
                        purpose: *purpose,
                        origin: origin.clone(),
                        state_digest: state_digest.clone(),
                        state: checkpoint.clone(),
                    },
                )
                .is_some()
            {
                return Err(duplicate("checkpoint", checkpoint_id));
            }
        }
        SessionEvent::ChildPrepared {
            child_id,
            turn_id,
            request,
        } => {
            require_active_turn(state, turn_id)?;
            if state.children.contains_key(child_id) {
                return Err(duplicate("child", child_id));
            }
            state.children.insert(
                child_id.clone(),
                ChildState {
                    turn_id: turn_id.clone(),
                    request: request.clone(),
                    result: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::ChildStarted { child_id } => {
            let turn_id = state
                .children
                .get(child_id)
                .ok_or_else(|| missing("child", child_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let child = required_mut(&mut state.children, "child", child_id)?;
            require_prepared(&child.effect, "child", child_id)?;
            child.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::ChildFinished {
            child_id,
            outcome,
            result,
        } => {
            let child = required_mut(&mut state.children, "child", child_id)?;
            require_unknown(&child.effect, "child", child_id)?;
            child.result = Some(result.clone());
            child.effect = ExternalEffectState::Completed {
                outcome: outcome.clone(),
            };
        }
        SessionEvent::ChildNotStarted { child_id, reason } => {
            let turn_id = state
                .children
                .get(child_id)
                .ok_or_else(|| missing("child", child_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let child = required_mut(&mut state.children, "child", child_id)?;
            require_prepared(&child.effect, "child", child_id)?;
            child.not_started_reason = Some(reason.clone());
            child.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::DeliveryPrepared {
            delivery_id,
            origin,
            destination,
            payload,
        } => {
            require_delivery_origin_active(state, origin)?;
            if state.deliveries.contains_key(delivery_id) {
                return Err(duplicate("delivery", delivery_id));
            }
            state.deliveries.insert(
                delivery_id.clone(),
                DeliveryState {
                    origin: origin.clone(),
                    destination: destination.clone(),
                    payload: payload.clone(),
                    completion: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::DeliveryStarted { delivery_id } => {
            let origin = state
                .deliveries
                .get(delivery_id)
                .ok_or_else(|| missing("delivery", delivery_id))?
                .origin
                .clone();
            require_delivery_origin_active(state, &origin)?;
            let delivery = required_mut(&mut state.deliveries, "delivery", delivery_id)?;
            require_prepared(&delivery.effect, "delivery", delivery_id)?;
            delivery.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::DeliveryNotStarted {
            delivery_id,
            reason,
        } => {
            let origin = state
                .deliveries
                .get(delivery_id)
                .ok_or_else(|| missing("delivery", delivery_id))?
                .origin
                .clone();
            require_delivery_origin_active(state, &origin)?;
            let delivery = required_mut(&mut state.deliveries, "delivery", delivery_id)?;
            require_prepared(&delivery.effect, "delivery", delivery_id)?;
            delivery.not_started_reason = Some(reason.clone());
            delivery.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::DeliveryFinished {
            delivery_id,
            completion,
        } => {
            let delivery = required_mut(&mut state.deliveries, "delivery", delivery_id)?;
            require_unknown(&delivery.effect, "delivery", delivery_id)?;
            if delivery.completion.is_some() {
                return Err(duplicate("delivery completion", delivery_id));
            }
            delivery.completion = Some(completion.clone());
            if let DeliveryCompletion::Confirmed { outcome, .. } = completion {
                delivery.effect = ExternalEffectState::Completed {
                    outcome: outcome.clone(),
                };
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod fault_tests {
    use super::*;

    #[test]
    fn append_io_failure_permanently_faults_writer() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("session.journal");
        let mut writer = JournalWriter::open(journal_path, "session".to_owned()).unwrap();
        let read_only_path = dir.path().join("read-only");
        std::fs::write(&read_only_path, []).unwrap();
        writer.file = OpenOptions::new().read(true).open(read_only_path).unwrap();

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
}
