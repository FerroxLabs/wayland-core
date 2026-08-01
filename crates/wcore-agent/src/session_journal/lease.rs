use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::JournalError;

/// Outcome of a non-blocking attempt to take the journal authority lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthorityLock {
    /// This handle now owns the authority lock.
    Acquired,
    /// Another handle on the same file object already owns it.
    Contended,
}

/// Take the journal authority lock without blocking.
///
/// The lock is bound to the underlying file object, not to a pathname, so a
/// hard-link alias of an already-locked journal contends here rather than
/// minting a second writer authority.
#[cfg(unix)]
fn try_lock_authority(file: &File) -> std::io::Result<AuthorityLock> {
    match file.try_lock() {
        Ok(()) => Ok(AuthorityLock::Acquired),
        Err(std::fs::TryLockError::WouldBlock) => Ok(AuthorityLock::Contended),
        Err(std::fs::TryLockError::Error(source)) => Err(source),
    }
}

#[cfg(unix)]
fn unlock_authority(file: &File) -> std::io::Result<()> {
    file.unlock()
}

/// Byte range reserved for the Windows authority lock.
///
/// Unix `flock` is advisory: it excludes competing lock holders without
/// blocking anybody's reads. Windows `LockFileEx` is *mandatory* over the
/// range it covers, and `File::try_lock` covers the whole file - so on Windows
/// a locked journal makes every other handle's read fail with
/// `ERROR_LOCK_VIOLATION` (33), including this crate's own `replay` and
/// `recovered_state` read paths.
///
/// Locking a one-byte sentinel past the largest addressable file offset keeps
/// the exclusion semantics identical (it is still bound to the file object, so
/// hard-link aliases contend) while leaving all real journal bytes readable and
/// appendable.
#[cfg(windows)]
const AUTHORITY_LOCK_OFFSET: u64 = u64::MAX - 1;
#[cfg(windows)]
const AUTHORITY_LOCK_LENGTH: u64 = 1;

#[cfg(windows)]
fn authority_lock_overlapped() -> windows_sys::Win32::System::IO::OVERLAPPED {
    // SAFETY: OVERLAPPED is a plain-old-data struct with no invalid bit
    // patterns; an all-zero value is its documented initial state.
    let mut overlapped =
        unsafe { std::mem::zeroed::<windows_sys::Win32::System::IO::OVERLAPPED>() };
    overlapped.Anonymous.Anonymous.Offset = AUTHORITY_LOCK_OFFSET as u32;
    overlapped.Anonymous.Anonymous.OffsetHigh = (AUTHORITY_LOCK_OFFSET >> 32) as u32;
    overlapped
}

#[cfg(windows)]
fn try_lock_authority(file: &File) -> std::io::Result<AuthorityLock> {
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };

    let mut overlapped = authority_lock_overlapped();
    // SAFETY: `file` keeps the OS handle valid for the call and `overlapped`
    // is a live, correctly initialised OVERLAPPED for the sentinel range.
    let succeeded = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            AUTHORITY_LOCK_LENGTH as u32,
            (AUTHORITY_LOCK_LENGTH >> 32) as u32,
            &mut overlapped,
        )
    };
    if succeeded != 0 {
        return Ok(AuthorityLock::Acquired);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(AuthorityLock::Contended)
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn unlock_authority(file: &File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;

    let mut overlapped = authority_lock_overlapped();
    // SAFETY: `file` keeps the OS handle valid for the call and `overlapped`
    // is a live, correctly initialised OVERLAPPED for the sentinel range.
    let succeeded = unsafe {
        UnlockFileEx(
            file.as_raw_handle(),
            0,
            AUTHORITY_LOCK_LENGTH as u32,
            (AUTHORITY_LOCK_LENGTH >> 32) as u32,
            &mut overlapped,
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn try_lock_authority(_file: &File) -> std::io::Result<AuthorityLock> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "journal authority locking is unavailable on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn unlock_authority(_file: &File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "journal authority locking is unavailable on this platform",
    ))
}

/// Canonicalize a directory into the representation this crate reports.
///
/// `std::fs::canonicalize` returns a verbatim `\\?\` path on Windows. Storing
/// that form makes the writer path (`SessionJournal::open`, which normalizes)
/// and the read-only paths (`replay`, `recovered_state`, which do not) report
/// two different pathnames for the same file, so callers cannot compare or
/// display journal error paths reliably. `dunce::simplified` converts back to
/// the ordinary form whenever that is safe, and deliberately leaves the
/// verbatim prefix in place when it is not - reserved DOS names, over-long
/// components, and non-Unicode paths still need it.
///
/// On Unix this is exactly `std::fs::canonicalize`.
pub(super) fn canonical_simplified_dir(directory: &Path) -> Result<PathBuf, JournalError> {
    let canonical = std::fs::canonicalize(directory).map_err(|source| JournalError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    Ok(dunce::simplified(&canonical).to_path_buf())
}

/// The pathname this crate REPORTS for `path`, resolving nothing on disk.
///
/// [`normalized_path`] is the WRITER-side normalizer: it also creates the parent
/// and rejects a symlinked target, because `SessionJournal::open` is about to
/// take authority over the file. The read-only entry points (`replay`,
/// `recovered_state`) must do neither — but they must report the SAME pathname,
/// or one call names `/private/var/…/s.journal` and the next names `/var/…` for
/// the same file and a caller cannot compare or display journal error paths.
/// That is exactly the hazard [`canonical_simplified_dir`] documents for Windows
/// verbatim prefixes; it was closed there with `dunce` and never closed for the
/// `/var` -> `/private/var` symlink macOS puts in front of every temp path.
///
/// Anything that cannot be resolved is returned unchanged: reading a journal
/// that does not exist yet is a legitimate empty read, not an error, so this
/// must not manufacture one.
pub(super) fn reported_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => return path.to_path_buf(),
        }
    };
    let (Some(parent), Some(file_name)) = (absolute.parent(), absolute.file_name()) else {
        return absolute;
    };
    match canonical_simplified_dir(parent) {
        Ok(canonical_parent) => canonical_parent.join(file_name),
        Err(_) => absolute,
    }
}

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
    let canonical_parent = canonical_simplified_dir(parent)?;
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
        match try_lock_authority(&file) {
            Ok(AuthorityLock::Acquired) => {}
            Ok(AuthorityLock::Contended) => {
                return Err(JournalError::AlreadyOwned { lease_path: path });
            }
            Err(source) => return Err(JournalError::Io { path, source }),
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
            let _ = unlock_authority(&file);
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

// Only the setter is Unix-exclusive: its single caller is a `#[cfg(unix)]`
// test, while the runner below is invoked by the lock path on every target.
#[cfg(all(test, unix))]
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
    match try_lock_authority(file) {
        Ok(AuthorityLock::Acquired) => Ok(()),
        Ok(AuthorityLock::Contended) => Err(JournalError::AlreadyOwned {
            lease_path: path.to_path_buf(),
        }),
        Err(source) => Err(JournalError::Io {
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
        let _ = unlock_authority(&self.file);
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
    match try_lock_authority(&file) {
        Ok(AuthorityLock::Acquired) => {
            let _ = unlock_authority(&file);
            Err(JournalError::InvalidTransition(
                "writer lease is not actively owned".to_owned(),
            ))
        }
        Ok(AuthorityLock::Contended) => {
            ensure_path_identity(&file, path)?;
            Ok(owner)
        }
        Err(source) => Err(JournalError::Io {
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
