use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::JournalError;

pub(super) fn normalized_path(path: &Path) -> Result<PathBuf, JournalError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|source| JournalError::Io {
                path: path.to_path_buf(),
                source,
            })?
    };
    match std::fs::symlink_metadata(&absolute) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(JournalError::SymbolicLink { path: absolute });
        }
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(JournalError::Io {
                path: absolute,
                source,
            });
        }
    }
    let Some(parent) = absolute.parent() else {
        return Ok(absolute);
    };
    std::fs::create_dir_all(parent).map_err(|source| JournalError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|source| JournalError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let Some(file_name) = absolute.file_name() else {
        return Err(JournalError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "session journal path has no file name",
            ),
        });
    };
    Ok(canonical_parent.join(file_name))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseOwner {
    pub process_id: u32,
    pub session_id: String,
    pub owner_token: String,
}

#[derive(Debug)]
pub(super) struct WriterLease {
    file: File,
}

impl WriterLease {
    pub(super) fn acquire(journal_path: &Path, session_id: &str) -> Result<Self, JournalError> {
        let path = lease_path(journal_path);
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
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(JournalError::AlreadyOwned { lease_path: path });
            }
            Err(TryLockError::Error(source)) => return Err(JournalError::Io { path, source }),
        }
        let owner_token = uuid::Uuid::new_v4().to_string();
        let owner = LeaseOwner {
            process_id: std::process::id(),
            session_id: session_id.to_owned(),
            owner_token: owner_token.clone(),
        };
        let bytes = serde_json::to_vec(&owner).map_err(|source| JournalError::Json {
            context: "encoding writer lease",
            source,
        })?;
        if let Err(source) = file
            .set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| file.write_all(&bytes))
            .and_then(|()| file.sync_all())
        {
            let _ = file.unlock();
            return Err(JournalError::Io { path, source });
        }
        Ok(Self { file })
    }
}

pub(super) fn lock_data_file(file: &File, path: &Path) -> Result<(), JournalError> {
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => Err(JournalError::AlreadyOwned {
            lease_path: path.to_path_buf(),
        }),
        Err(TryLockError::Error(source)) => Err(JournalError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(super) fn reject_multiple_links(file: &File, path: &Path) -> Result<(), JournalError> {
    if link_count(file, path)? == 1 {
        Ok(())
    } else {
        Err(JournalError::MultipleLinks {
            path: path.to_path_buf(),
        })
    }
}

#[cfg(unix)]
fn link_count(file: &File, path: &Path) -> Result<u64, JournalError> {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata()
        .map(|metadata| metadata.nlink())
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(windows)]
fn link_count(file: &File, path: &Path) -> Result<u64, JournalError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    // SAFETY: this Windows POD has no invalid bit patterns.
    let mut information = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    // SAFETY: `file` keeps the OS handle valid for the call and `information`
    // is a writable, correctly sized output buffer.
    let succeeded = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if succeeded == 0 {
        return Err(JournalError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    Ok(u64::from(information.nNumberOfLinks))
}

#[cfg(not(any(unix, windows)))]
fn link_count(_file: &File, path: &Path) -> Result<u64, JournalError> {
    Err(JournalError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "filesystem link-count verification is unavailable on this platform",
        ),
    })
}

impl Drop for WriterLease {
    fn drop(&mut self) {
        // The sentinel inode must remain, but stale ownership metadata need
        // not. Scrub it while still holding the advisory lock so a successor
        // never observes a partially cleared owner record.
        let _ = self
            .file
            .set_len(0)
            .and_then(|()| self.file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| self.file.sync_all());
        let _ = self.file.unlock();
    }
}

pub(super) fn inspect(journal_path: &Path) -> Result<LeaseOwner, JournalError> {
    let path = lease_path(journal_path);
    read_owner(&path)
}

fn read_owner(path: &Path) -> Result<LeaseOwner, JournalError> {
    let bytes = std::fs::read(path).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| JournalError::Json {
        context: "decoding writer lease",
        source,
    })
}

fn lease_path(journal_path: &Path) -> PathBuf {
    let mut name = journal_path
        .file_name()
        .map_or_else(|| OsString::from("session"), OsString::from);
    name.push(".writer.lock");
    journal_path.with_file_name(name)
}
