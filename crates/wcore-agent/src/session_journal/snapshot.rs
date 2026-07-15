use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    JournalEnvelope, JournalError, ReducedSessionState, SESSION_JOURNAL_SCHEMA_VERSION, reduce,
};

#[cfg(test)]
thread_local! {
    static FAIL_REPLACE_AFTER_PERSIST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(super) fn fail_next_replace_after_persist() {
    FAIL_REPLACE_AFTER_PERSIST.with(|fail| fail.set(true));
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[rustfmt::skip]
pub struct SessionSnapshot {
    pub schema_version: u32, pub session_id: String, pub cursor: Option<u64>,
    pub cursor_checksum: String, pub state_digest: String, pub state: ReducedSessionState,
}

impl SessionSnapshot {
    pub fn new(
        session_id: impl Into<String>,
        mut state: ReducedSessionState,
    ) -> Result<Self, JournalError> {
        let session_id = session_id.into();
        if let Some(found) = state.session_id.as_deref()
            && found != session_id
        {
            return Err(JournalError::SessionMismatch {
                expected: session_id,
                found: found.to_owned(),
            });
        }
        state.session_id = Some(session_id.clone());
        let state_digest = state.digest()?;
        Ok(Self {
            schema_version: SESSION_JOURNAL_SCHEMA_VERSION,
            session_id,
            cursor: state.last_seq,
            cursor_checksum: state.last_checksum.clone(),
            state_digest,
            state,
        })
    }

    pub fn validate(&self) -> Result<(), JournalError> {
        if self.schema_version != SESSION_JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchema {
                found: self.schema_version,
                supported: SESSION_JOURNAL_SCHEMA_VERSION,
            });
        }
        if self.state.session_id.as_deref() != Some(self.session_id.as_str()) {
            return Err(JournalError::SessionMismatch {
                expected: self.session_id.clone(),
                found: self.state.session_id.clone().unwrap_or_default(),
            });
        }
        if self.cursor != self.state.last_seq || self.cursor_checksum != self.state.last_checksum {
            return Err(JournalError::SnapshotCursorMismatch);
        }
        if self.state.digest()? != self.state_digest {
            return Err(JournalError::SnapshotDigestMismatch);
        }
        Ok(())
    }
}

pub fn write_snapshot(
    path: impl AsRef<Path>,
    snapshot: &SessionSnapshot,
) -> Result<(), JournalError> {
    snapshot.validate()?;
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| JournalError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut bytes = serde_json::to_vec(snapshot).map_err(|source| JournalError::Json {
        context: "encoding session snapshot",
        source,
    })?;
    bytes.push(b'\n');
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|source| JournalError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    temp.write_all(&bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let persisted = temp.persist(path).map_err(|error| JournalError::Io {
        path: path.to_path_buf(),
        source: error.error,
    })?;
    persisted.sync_all().map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    sync_parent_directory(path)?;
    Ok(())
}

/// Companion snapshot path used by [`super::SessionJournal`].
#[must_use]
pub fn snapshot_path_for(journal_path: impl AsRef<Path>) -> PathBuf {
    let journal_path = journal_path.as_ref();
    let mut name = journal_path
        .file_name()
        .map_or_else(|| "session.journal".into(), std::ffi::OsString::from);
    name.push(".snapshot");
    journal_path.with_file_name(name)
}

pub fn load_snapshot(path: impl AsRef<Path>) -> Result<SessionSnapshot, JournalError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let snapshot =
        serde_json::from_slice::<SessionSnapshot>(&bytes).map_err(|source| JournalError::Json {
            context: "decoding session snapshot",
            source,
        })?;
    snapshot.validate()?;
    Ok(snapshot)
}

pub fn replay_from_snapshot(
    snapshot: &SessionSnapshot,
    suffix: &[JournalEnvelope],
) -> Result<ReducedSessionState, JournalError> {
    snapshot.validate()?;
    suffix.iter().try_fold(snapshot.state.clone(), reduce)
}

pub(super) fn load_snapshot_if_present(
    path: impl AsRef<Path>,
) -> Result<Option<SessionSnapshot>, JournalError> {
    let path = path.as_ref();
    match load_snapshot(path) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(JournalError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn replace_file_atomically(path: &Path, bytes: &[u8]) -> Result<File, JournalError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| JournalError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|source| JournalError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    temp.write_all(bytes)
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let persisted = temp.persist(path).map_err(|error| JournalError::Io {
        path: path.to_path_buf(),
        source: error.error,
    })?;
    #[cfg(test)]
    if FAIL_REPLACE_AFTER_PERSIST.with(|fail| fail.replace(false)) {
        return Err(JournalError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other("injected replacement failure after persist"),
        });
    }
    persisted.sync_all().map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(persisted)
}

pub(super) fn sync_parent_directory(path: &Path) -> Result<(), JournalError> {
    #[cfg(unix)]
    {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| JournalError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    Ok(())
}
