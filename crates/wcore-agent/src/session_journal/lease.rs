use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
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
#[serde(deny_unknown_fields)]
pub struct LeaseOwner {
    pub process_id: u32,
    pub session_id: String,
    pub owner_token: String,
}

#[derive(Debug)]
pub(super) struct WriterLease {
    file: File,
    path: PathBuf,
}

impl WriterLease {
    pub(super) fn acquire(journal_path: &Path, session_id: &str) -> Result<Self, JournalError> {
        let path = lease_path(journal_path);
        let mut file = open_or_create_nofollow(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(JournalError::AlreadyOwned { lease_path: path });
            }
            Err(TryLockError::Error(source)) => return Err(JournalError::Io { path, source }),
        }
        run_after_lease_lock_hook(&path);
        validate_opened_regular_file(&file, &path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|source| JournalError::Io {
                    path: path.clone(),
                    source,
                })?;
        }
        validate_opened_regular_file(&file, &path)?;
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
        validate_opened_regular_file(&file, &path)?;
        Ok(Self { file, path })
    }

    pub(super) fn validate_current_path(&self) -> Result<(), JournalError> {
        validate_opened_regular_file(&self.file, &self.path)
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_LEASE_LOCK_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&Path)>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(super) fn set_after_lease_lock_hook(hook: impl FnOnce(&Path) + 'static) {
    AFTER_LEASE_LOCK_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_after_lease_lock_hook(path: &Path) {
    AFTER_LEASE_LOCK_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(path);
        }
    });
}

#[cfg(not(test))]
fn run_after_lease_lock_hook(_path: &Path) {}

pub(super) fn open_existing_nofollow(path: &Path) -> Result<File, JournalError> {
    open_existing_with_access(path, false)
}

pub(super) fn open_existing_read_write_nofollow(path: &Path) -> Result<File, JournalError> {
    open_existing_with_access(path, true)
}

fn open_existing_with_access(path: &Path, write: bool) -> Result<File, JournalError> {
    reject_symlink_path(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(write);
    configure_reparse_safe(&mut options);
    let file = options.open(path).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    validate_opened_regular_file(&file, path)?;
    Ok(file)
}

pub(super) fn open_or_create_nofollow(path: &Path) -> Result<File, JournalError> {
    const MAX_PATH_RACE_RETRIES: usize = 8;

    for _ in 0..MAX_PATH_RACE_RETRIES {
        reject_symlink_path(path)?;
        let mut existing = OpenOptions::new();
        existing.read(true).write(true);
        configure_reparse_safe(&mut existing);
        match existing.open(path) {
            Ok(file) => {
                validate_opened_regular_file(&file, path)?;
                return Ok(file);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(JournalError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }

        let mut new_file = OpenOptions::new();
        new_file.read(true).write(true).create_new(true);
        configure_reparse_safe(&mut new_file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            new_file.mode(0o600);
        }
        match new_file.open(path) {
            Ok(file) => {
                validate_opened_regular_file(&file, path)?;
                return Ok(file);
            }
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(JournalError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }

    Err(JournalError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "journal authority path changed repeatedly while opening",
        ),
    })
}

fn reject_symlink_path(path: &Path) -> Result<(), JournalError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => reject_link_like_metadata(&metadata, path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(JournalError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn validate_opened_regular_file(file: &File, path: &Path) -> Result<(), JournalError> {
    validate_regular_path(path)?;
    validate_regular_handle(file, path)?;
    reject_multiple_links(file, path)?;
    ensure_path_identity(file, path)
}

fn validate_regular_handle(file: &File, path: &Path) -> Result<(), JournalError> {
    let metadata = file.metadata().map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    reject_link_like_metadata(&metadata, path)?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(JournalError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "opened journal authority is not a regular file",
            ),
        })
    }
}

fn validate_regular_path(path: &Path) -> Result<(), JournalError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    reject_link_like_metadata(&metadata, path)?;
    if !metadata.is_file() {
        return Err(JournalError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "journal authority path is not a regular file",
            ),
        });
    }
    Ok(())
}

fn reject_link_like_metadata(
    metadata: &std::fs::Metadata,
    path: &Path,
) -> Result<(), JournalError> {
    if metadata.file_type().is_symlink() || metadata_is_windows_reparse_point(metadata) {
        Err(JournalError::SymbolicLink {
            path: path.to_path_buf(),
        })
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn metadata_is_windows_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_windows_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn configure_reparse_safe(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(unix)]
fn configure_reparse_safe(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt as _;

    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

#[cfg(not(any(unix, windows)))]
fn configure_reparse_safe(_options: &mut OpenOptions) {}

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

/// Prove that the canonical journal pathname still names the held data file.
///
/// Advisory locks bind an inode/file-id, not a pathname. Without this check an
/// attacker can rename the locked file away, install a replacement at the
/// canonical path, and make an fsync on the displaced handle look successful.
pub(super) fn ensure_path_identity(file: &File, path: &Path) -> Result<(), JournalError> {
    validate_regular_handle(file, path)?;
    reject_multiple_links(file, path)?;
    let path_file = open_identity_probe(path)?;
    ensure_same_identity(file, &path_file, path)?;
    validate_regular_path(path)?;
    validate_regular_handle(file, path)?;
    reject_multiple_links(file, path)?;
    let final_probe = open_identity_probe(path)?;
    ensure_same_identity(file, &final_probe, path)
}

pub(super) fn ensure_same_identity(
    expected: &File,
    observed: &File,
    path: &Path,
) -> Result<(), JournalError> {
    if file_identity(expected, path)? == file_identity(observed, path)? {
        Ok(())
    } else {
        Err(JournalError::PathIdentityMismatch {
            path: path.to_path_buf(),
        })
    }
}

fn open_identity_probe(path: &Path) -> Result<File, JournalError> {
    reject_symlink_path(path)?;
    let mut options = OpenOptions::new();
    // Unix opens are nonblocking, so a raced FIFO is rejected by the regular
    // file check without requiring write access to read-only authority files.
    options.read(true);
    configure_reparse_safe(&mut options);
    let file = options.open(path).map_err(|source| JournalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    validate_regular_path(path)?;
    validate_regular_handle(&file, path)?;
    reject_multiple_links(&file, path)?;
    validate_regular_path(path)?;
    Ok(file)
}

#[cfg(unix)]
fn file_identity(file: &File, path: &Path) -> Result<(u64, u64), JournalError> {
    use std::os::unix::fs::MetadataExt as _;

    file.metadata()
        .map(|metadata| (metadata.dev(), metadata.ino()))
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(windows)]
fn file_identity(file: &File, path: &Path) -> Result<(u32, u64), JournalError> {
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
    Ok((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_file: &File, path: &Path) -> Result<(), JournalError> {
    Err(JournalError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "filesystem identity verification is unavailable on this platform",
        ),
    })
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
    let mut file = open_existing_read_write_nofollow(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    ensure_path_identity(&file, path)?;
    let owner: LeaseOwner =
        serde_json::from_slice(&bytes).map_err(|source| JournalError::Json {
            context: "decoding writer lease",
            source,
        })?;
    if owner.process_id == 0
        || owner.session_id.is_empty()
        || uuid::Uuid::parse_str(&owner.owner_token).is_err()
    {
        return Err(JournalError::InvalidTransition(
            "writer lease contains invalid owner metadata".to_owned(),
        ));
    }
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            Err(JournalError::InvalidTransition(
                "writer lease is not actively owned".to_owned(),
            ))
        }
        Err(TryLockError::WouldBlock) => {
            ensure_path_identity(&file, path)?;
            Ok(owner)
        }
        Err(TryLockError::Error(source)) => Err(JournalError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(super) fn lease_path(journal_path: &Path) -> PathBuf {
    let mut name = journal_path
        .file_name()
        .map_or_else(|| OsString::from("session"), OsString::from);
    name.push(".writer.lock");
    journal_path.with_file_name(name)
}
