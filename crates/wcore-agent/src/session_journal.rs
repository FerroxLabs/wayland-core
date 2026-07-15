//! Crash-safe, append-only session execution journal.
//!
//! Records form a versioned SHA-256 chain independent of session snapshots.
//! Started external effects recover as [`ExternalEffectState::Unknown`], never
//! as safe to repeat.

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
pub use reducer::{provider_request_digest, reduce, replay_state, state_payload_digest};
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
