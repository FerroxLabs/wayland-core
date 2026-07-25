//! Windows implementation behind the portable retained-directory facade.
//!
//! Every child operation is rooted at an already-retained directory handle.
//! Paths stored on authority objects are diagnostic metadata only.

use super::*;

use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};

use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_ID_BOTH_DIR_INFORMATION, FILE_NON_DIRECTORY_FILE,
    FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
    FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS, FILE_SYNCHRONOUS_IO_NONALERT,
    FileIdBothDirectoryInformation, FileRenameInformation, FileRenameInformationEx, NtCreateFile,
    NtQueryDirectoryFile, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{
    GENERIC_READ, GENERIC_WRITE, HANDLE, RtlNtStatusToDosError, STATUS_BUFFER_OVERFLOW,
    STATUS_BUFFER_TOO_SMALL, STATUS_NO_MORE_FILES, STATUS_PENDING, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

const OBJ_CASE_INSENSITIVE: u32 = 0x40;
pub(super) const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x1;

#[repr(C)]
pub(super) struct DirectoryCaseSensitiveInfo {
    pub(super) flags: u32,
}

/// The kind of object a handle-relative open targets.
///
/// There is deliberately NO "unknown" variant. Every relative open in this
/// module knows the object's type before it opens it — the cleanup walk carries
/// the kind out of the directory enumeration — and the kind selects both the
/// access mask and the `FILE_DIRECTORY_FILE` / `FILE_NON_DIRECTORY_FILE` option
/// that makes the KERNEL enforce the type at open. An unknown kind could only be
/// served by the union of two incompatible masks (a directory child requires
/// write because cleanup flushes; a read-only file child forbids it because the
/// open is refused), so its absence is a fail-closed CONSTRUCTION: a future
/// caller that wants one has to add it back deliberately and answer the rights
/// question, rather than silently inheriting an over-broad grant.
#[derive(Clone, Copy)]
enum RelativeKind {
    Directory,
    File,
}

#[derive(Clone, Copy)]
enum RelativeIntent {
    ReadOnly,
    Mutate,
    Create,
    /// Open an EXISTING regular-file advisory-lock target. Surfaces a not-found
    /// error so a caller probing an unheld lock never mutates the directory it
    /// is merely observing.
    LockOpen,
    /// Open-or-create a regular-file advisory-lock target.
    LockOpenOrCreate,
}

pub(super) fn open_directory(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        // DirectoryAuthority is a mutation authority. GENERIC_WRITE is also
        // required for File::sync_all/FlushFileBuffers to provide the Windows
        // durability boundary used after relative create, rename, and delete.
        .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(
            windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS
                | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
        );
    options.open(path)
}

/// Open a directory handle that requests NO delete access, for an authority
/// that only witnesses identity.
///
/// Measured on Windows: a handle carrying the `DELETE` access right blocks
/// `SetCurrentDirectory` into that directory with a sharing violation.
/// `SetCurrentDirectory` deliberately opens its target without sharing delete,
/// so the current directory of a running process cannot be removed underneath
/// it; any outstanding DELETE-bearing handle therefore denies the chdir. MSYS
/// and Cygwin `chdir()` inherit that failure, so every git subcommand flagged
/// NEED_WORK_TREE (`status` among them) dies inside `setup_work_tree()` with
/// `fatal: this operation must be run in a work tree` — while `rev-parse` and
/// `config`, which never chdir, succeed against the same directory. Dropping
/// only the `DELETE` bit makes the chdir succeed; the share mode and the path
/// representation are not involved.
///
/// That is why this function exists: widening it back to the mutating access
/// mode of [`open_directory`] reintroduces a 10-test Windows failure across the
/// swarm dispatch, collision and worker-runtime suites. It mirrors the
/// observational reasoning already recorded for `open_regular_file` below.
///
/// An authority acquired through this open is an IDENTITY WITNESS ONLY.
/// Destructive and relative-child operations on it are outside its contract and
/// fail closed with an OS access error rather than succeeding silently.
///
/// `FILE_FLAG_BACKUP_SEMANTICS` is mandatory to obtain a directory handle at
/// all, and `FILE_FLAG_OPEN_REPARSE_POINT` is preserved so a reparse point at
/// the pathname opens the link itself and is then refused by
/// `validate_real_directory`.
pub(super) fn open_directory_observational(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        .access_mode(GENERIC_READ)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(
            windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS
                | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
        );
    options.open(path)
}

/// Acquire an OS-ENFORCED PIN ON THE RETAINED DIRECTORY'S NAME, held for as
/// long as the returned handle lives.
///
/// WHY THIS EXISTS. `CreateProcess` takes `lpCurrentDirectory` as a PATHNAME,
/// not a HANDLE, and Windows has no `fchdir`, so the Linux mechanism — hand the
/// retained descriptor into the child and chdir to it — has no equivalent here.
/// A path-form bind is only sound if the pathname CANNOT be redirected to a
/// different object while the child runs. This handle is what makes that true.
///
/// HOW IT PINS, and why the retained authority handle does not. Windows share
/// arbitration refuses a new open whose desired access is not permitted by the
/// share mode of every handle already open on the object. Renaming or unlinking
/// an object requires opening it with `DELETE`. The retained authority is opened
/// `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, so it permits that
/// `DELETE` open and pins the OBJECT (the object stays alive and every
/// handle-relative operation keeps reaching it) but NOT the NAME. This lease
/// requests a share-arbitrated access (`GENERIC_READ`) while OMITTING
/// `FILE_SHARE_DELETE`, so while it is held every rename and every unlink of the
/// pinned name is refused by the KERNEL.
///
/// MEASURED ON SEANDESKTOP (NTFS), against the retained OBSERVATIONAL checkout
/// authority the delegated dispatch path actually produces:
/// - external rename of the pinned name: REFUSED, sharing violation;
/// - external unlink of the pinned name: REFUSED, sharing violation;
/// - `CreateProcess(lpCurrentDirectory = display path)`: succeeds, and the file
///   the child creates is visible THROUGH the retained handle — so the child
///   provably operated on the retained object;
/// - after the lease drops, rename and the ordinary destructive cleanup both
///   succeed again, so the pin costs nothing outside the bound execution.
///
/// NO PATHNAME IS RESOLVED HERE. The open is HANDLE-RELATIVE — `RootDirectory`
/// is the retained handle and `ObjectName` is empty — which is the NT "reopen
/// this exact object" form. A pathname-based reopen would be the very
/// re-resolution this lease exists to make safe.
///
/// FAILS CLOSED BY CONSTRUCTION. If the retained authority already holds
/// `DELETE` (an authority opened through [`open_directory`] rather than
/// [`open_directory_observational`]), this open is refused: the lease's share
/// mode would have to permit the `DELETE` the existing handle was granted, and
/// it deliberately does not. The caller must surface that refusal, never spawn
/// unpinned.
pub(super) fn acquire_name_lease(authority: &DirectoryAuthority) -> Result<File> {
    let unicode_name = UNICODE_STRING {
        Length: 0,
        MaximumLength: 0,
        Buffer: std::ptr::null_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: authority.handle.as_raw_handle().cast(),
        ObjectName: &unicode_name,
        Attributes: 0,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut status_block = zeroed_status_block();
    let mut handle: HANDLE = std::ptr::null_mut();
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            // GENERIC_READ is share-arbitrated, which is the whole point: an
            // attributes-only open requests none of read/write/delete, so it
            // neither is checked against nor contributes to share arbitration,
            // and was MEASURED to deliver NO pin at all.
            GENERIC_READ | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            std::ptr::null(),
            FILE_ATTRIBUTE_DIRECTORY,
            // The omission of FILE_SHARE_DELETE IS the pin. Do not widen it.
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | FILE_DIRECTORY_FILE,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        return Err(ntstatus_error(status).into());
    }
    if handle.is_null() {
        return Err(SandboxError::ExecFailed(
            "NtCreateFile succeeded without returning a name-lease handle".to_owned(),
        ));
    }
    // SAFETY: NtCreateFile returned a fresh owned handle on success.
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(super) fn open_regular_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        // RegularFileAuthority::open is observational. Do not make a readable
        // file impossible to open merely because its ACL withholds DELETE.
        .access_mode(GENERIC_READ)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

pub(super) fn identity(handle: &File) -> Result<DirectoryIdentity> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };

    let mut info = std::mem::MaybeUninit::<FILE_ID_INFO>::zeroed();
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle().cast(),
            FileIdInfo,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    Ok(DirectoryIdentity {
        volume: info.VolumeSerialNumber,
        file_id: info.FileId.Identifier,
    })
}

pub(super) fn open_child_directory(
    parent: &DirectoryAuthority,
    name: &str,
) -> Result<DirectoryAuthority> {
    let handle = open_relative(
        parent,
        name,
        RelativeKind::Directory,
        RelativeIntent::Mutate,
    )?;
    let metadata = handle.metadata()?;
    validate_real_directory(Path::new("<retained child>"), &metadata)?;
    let identity = handle_directory_identity(&handle, &metadata)?;
    Ok(directory_authority(parent, name, handle, identity))
}

/// Name-only projection of `child_entries`.
///
/// Kept as a projection rather than a second `NtQueryDirectoryFile` loop so the
/// crate has exactly ONE directory-enumeration implementation.
pub(super) fn child_names(parent: &DirectoryAuthority) -> Result<Vec<String>> {
    Ok(child_entries(parent)?
        .into_iter()
        .map(|entry| entry.name)
        .collect())
}

/// Enumerate the parent's children with the attribute word the kernel reported
/// for each, sorted and deduplicated by name.
pub(super) fn child_entries(parent: &DirectoryAuthority) -> Result<Vec<DirectoryEntry>> {
    let mut entries = Vec::new();
    let mut restart_scan = 1;
    let mut storage = vec![0_usize; 64 * 1024 / std::mem::size_of::<usize>()];

    loop {
        let mut status_block = zeroed_status_block();
        let status = unsafe {
            NtQueryDirectoryFile(
                parent.handle.as_raw_handle().cast(),
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                &mut status_block,
                storage.as_mut_ptr().cast(),
                (storage.len() * std::mem::size_of::<usize>()) as u32,
                FileIdBothDirectoryInformation,
                0,
                std::ptr::null(),
                restart_scan,
            )
        };
        restart_scan = 0;
        if status == STATUS_NO_MORE_FILES {
            break;
        }
        if status == STATUS_BUFFER_TOO_SMALL {
            return Err(SandboxError::ExecFailed(
                "Windows returned an oversized directory entry".to_owned(),
            ));
        }
        if status < 0 && status != STATUS_BUFFER_OVERFLOW {
            return Err(ntstatus_error(status).into());
        }

        let capacity = storage.len() * std::mem::size_of::<usize>();
        let returned = checked_information_length(status_block.Information, capacity)?;
        if returned == 0 {
            if status == STATUS_BUFFER_OVERFLOW {
                return Err(SandboxError::ExecFailed(
                    "Windows directory enumeration overflowed without an entry".to_owned(),
                ));
            }
            break;
        }
        parse_directory_entries(storage.as_ptr().cast(), returned, &mut entries)?;
    }

    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries.dedup_by(|left, right| left.name == right.name);
    Ok(entries)
}

pub(super) fn open_child_file(
    parent: &DirectoryAuthority,
    name: &str,
) -> Result<RegularFileAuthority> {
    open_child_file_with(parent, name, RelativeIntent::ReadOnly)
}

/// Open a regular-file child with the access `remove_child_file` needs to
/// DESTROY it.
///
/// On Windows a file is deleted through a disposition set on the FILE's own
/// handle, so that handle must carry `DELETE`; the read-only profile
/// `open_child_file` uses cannot delete and is refused with os error 5. The
/// mutate profile deliberately withholds the WRITE bit (see the access match),
/// which is what also lets a read-only file be removed.
///
/// Gated to match its only caller's configuration (see the portable wrapper).
#[cfg(any(feature = "live-docker", test))]
pub(super) fn open_child_file_for_removal(
    parent: &DirectoryAuthority,
    name: &str,
) -> Result<RegularFileAuthority> {
    open_child_file_with(parent, name, RelativeIntent::Mutate)
}

fn open_child_file_with(
    parent: &DirectoryAuthority,
    name: &str,
    intent: RelativeIntent,
) -> Result<RegularFileAuthority> {
    let handle = open_relative(parent, name, RelativeKind::File, intent)?;
    let metadata = handle.metadata()?;
    validate_real_file(Path::new("<retained child>"), &metadata)?;
    let identity = handle_directory_identity(&handle, &metadata)?;
    Ok(RegularFileAuthority {
        handle,
        identity,
        display_path: parent.display_path.join(name),
    })
}

/// Open a REGULAR-FILE advisory-lock target beneath a retained directory,
/// handle-relative.
///
/// WHY THIS EXISTS — measured on Windows 11 (10.0.26200.8875, NTFS), do not
/// re-derive: **Windows byte-range locking is UNDEFINED on directory objects.**
/// `LockFileEx(<directory HANDLE>, LOCKFILE_EXCLUSIVE_LOCK, 0, 1, 0, ..)`
/// returns FALSE with `GetLastError() == 87 (ERROR_INVALID_PARAMETER)` for
/// EVERY access mode and share mode tried — `GENERIC_READ|GENERIC_WRITE|DELETE`,
/// `GENERIC_READ|GENERIC_WRITE`, and `GENERIC_READ` alike, all shared
/// read/write/delete — while the same call on a REGULAR-FILE handle succeeds.
/// Rust maps error 87 to [`std::io::ErrorKind::InvalidInput`]. No access-mode
/// change can fix it; the object type is the whole story.
///
/// `fd-lock` (4.0.4, `src/sys/windows/rw_lock.rs::write`) calls `LockFileEx`
/// directly, so EVERY advisory lock in this workspace MUST target a regular
/// file. Retargeting any caller of this primitive at a directory handle does
/// not fail loudly at review time — it silently disables Windows mutual
/// exclusion, which is exactly how the defect this closes survived for the
/// project's entire life.
///
/// The child is resolved through the parent's RETAINED handle
/// (`NtCreateFile` with `RootDirectory = parent.handle`), never by
/// re-resolving a pathname, so a swap of the parent directory between an
/// identity proof and lock acquisition cannot redirect the lock.
///
/// `create = false` opens an existing target only and surfaces a not-found
/// error; `create = true` opens-or-creates it.
pub(super) fn open_child_lock_file(
    parent: &DirectoryAuthority,
    name: &str,
    create: bool,
) -> Result<File> {
    open_relative(
        parent,
        name,
        RelativeKind::File,
        if create {
            RelativeIntent::LockOpenOrCreate
        } else {
            RelativeIntent::LockOpen
        },
    )
}

pub(super) fn create_child_directory(
    parent: &DirectoryAuthority,
    name: &str,
) -> Result<DirectoryAuthority> {
    let handle = open_relative(
        parent,
        name,
        RelativeKind::Directory,
        RelativeIntent::Create,
    )?;
    let result = (|| {
        maybe_fail_created_validation(CreateValidationStage::Metadata)?;
        let metadata = handle.metadata()?;
        maybe_fail_created_validation(CreateValidationStage::Type)?;
        validate_real_directory(Path::new("<retained child>"), &metadata)?;
        maybe_fail_created_validation(CreateValidationStage::Identity)?;
        let identity = handle_directory_identity(&handle, &metadata)?;
        // Publishing a newly-created name is not durable until the directory
        // containing that name has also crossed its flush boundary.
        parent.handle.sync_all()?;
        Ok(directory_authority(
            parent,
            name,
            handle.try_clone()?,
            identity,
        ))
    })();
    match result {
        Ok(authority) => Ok(authority),
        Err(error) => rollback_created_object(parent, name, handle, "directory", error),
    }
}

pub(super) fn create_child_file(
    parent: &DirectoryAuthority,
    name: &str,
    contents: &[u8],
) -> Result<RegularFileAuthority> {
    let mut handle = open_relative(parent, name, RelativeKind::File, RelativeIntent::Create)?;
    let result = (|| {
        handle.write_all(contents)?;
        handle.sync_all()?;
        maybe_fail_created_validation(CreateValidationStage::Metadata)?;
        let metadata = handle.metadata()?;
        maybe_fail_created_validation(CreateValidationStage::Type)?;
        validate_real_file(Path::new("<retained child>"), &metadata)?;
        maybe_fail_created_validation(CreateValidationStage::Identity)?;
        let identity = handle_directory_identity(&handle, &metadata)?;
        // The file data and the parent namespace are separate durability
        // boundaries on Windows; both must be flushed before returning.
        parent.handle.sync_all()?;
        Ok(identity)
    })();
    match result {
        Ok(identity) => Ok(RegularFileAuthority {
            handle,
            identity,
            display_path: parent.display_path.join(name),
        }),
        Err(error) => rollback_created_object(parent, name, handle, "file", error),
    }
}

pub(super) fn bind_command_cwd(
    _authority: &DirectoryAuthority,
    _command: &mut tokio::process::Command,
) -> Result<()> {
    Err(SandboxError::PolicyNotSupported(
        "Windows cannot bind a child working directory to a retained handle without a process-lifetime name lease"
            .to_owned(),
    ))
}

pub(super) fn delete_open_object(handle: &File, path: &Path, kind: &str) -> Result<()> {
    let metadata = handle.metadata()?;
    if is_symlink_or_reparse(&metadata) {
        return Err(SandboxError::PathDenied(format!(
            "refused linked Windows {kind}: {}",
            path.display()
        )));
    }
    mark_open_object_for_delete(handle, path, kind)
}

pub(super) fn mark_open_object_for_delete(handle: &File, path: &Path, kind: &str) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
        FileDispositionInfo, FileDispositionInfoEx, SetFileInformationByHandle,
    };

    let extended = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    if unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle().cast(),
            FileDispositionInfoEx,
            std::ptr::addr_of!(extended).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } != 0
    {
        return Ok(());
    }

    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    if unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle().cast(),
            FileDispositionInfo,
            std::ptr::addr_of!(disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(SandboxError::ExecFailed(format!(
            "delete retained Windows {kind} {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn rollback_created_object<T>(
    parent: &DirectoryAuthority,
    name: &str,
    handle: File,
    kind: &str,
    original: SandboxError,
) -> Result<T> {
    let path = parent.display_path.join(name);
    match mark_open_object_for_delete(&handle, &path, kind) {
        Ok(()) => {
            drop(handle);
            match parent.handle.sync_all() {
                Ok(()) => Err(original),
                Err(sync_error) => Err(SandboxError::ExecFailed(format!(
                    "Windows created {kind} validation failed ({original}); rollback succeeded but parent durability failed ({sync_error})"
                ))),
            }
        }
        Err(cleanup) => Err(SandboxError::ExecFailed(format!(
            "Windows created {kind} validation failed ({original}); exact-handle rollback also failed ({cleanup})"
        ))),
    }
}

pub(super) fn rename_file_into(
    source: &RegularFileAuthority,
    target_parent: &DirectoryAuthority,
    name: &str,
    replace: bool,
) -> Result<()> {
    #[cfg(test)]
    run_before_atomic_file_rename_hook();
    rename_handle_into(&source.handle, target_parent, name, replace)
}

pub(super) fn rename_directory_into(
    source: &DirectoryAuthority,
    target_parent: &DirectoryAuthority,
    name: &str,
    replace: bool,
) -> Result<()> {
    rename_handle_into(&source.handle, target_parent, name, replace)?;
    target_parent.handle.sync_all()?;
    Ok(())
}

/// Rename the exact object behind `source` to `name` beneath the RETAINED
/// `target_parent` handle. The destination parent is named ONLY by that handle;
/// no pathname is ever placed in the rename's name field.
///
/// WHY THE NT CALL AND NOT THE WIN32 WRAPPER. A probe isolating a single
/// variable proved, on this hardware, that `SetFileInformationByHandle` with the
/// Win32 `FileRenameInfo` class REJECTS a Win32 HANDLE in `RootDirectory`:
///
///   PROBE[A RootDirectory=<Win32 HANDLE>, relative name] -> FAILED os error 87
///   PROBE[B RootDirectory=NULL, full destination path]   -> SUCCEEDED
///
/// Same access rights, same buffer, same layout (`sizeof=24`,
/// `offsetof(FileName)=20`). Because this is the crate's ONLY handle-relative
/// rename, that defect silently disabled `atomic_write_child` — and with it the
/// production heartbeat status mirror, the swarm directory-rename API and the
/// authority archive import/rollback path — for the whole life of the Windows
/// port. `NtSetInformationFile` with `FileRenameInformation` genuinely honours a
/// `RootDirectory` handle, which is why it is used here.
///
/// THE FORM PROBE B SHOWED WORKING IS FORBIDDEN HERE. `RootDirectory = NULL`
/// with a full destination PATHNAME in `FileName` re-resolves the destination BY
/// PATHNAME at rename time. That destroys exactly the anti-swap/TOCTOU guarantee
/// the entire retained-handle design exists to provide: an attacker who
/// substitutes the destination directory between the identity proof and the
/// rename would have the rename land in the SUBSTITUTED directory. The
/// destination must stay named by a held handle that can never be re-resolved.
/// Anyone reaching for the pathname form to "make it simpler" would be trading a
/// security property for a syntax preference. If the NT call ever stops working,
/// the correct outcome is a reported failure, never a fallback to the pathname
/// form.
fn rename_handle_into(
    source: &File,
    target_parent: &DirectoryAuthority,
    name: &str,
    replace: bool,
) -> Result<()> {
    validate_windows_child_name(name)?;
    let name: Vec<u16> = std::ffi::OsStr::new(name).encode_wide().collect();
    // `FileNameLength` is the BYTE length of the UTF-16 name, NOT a code-unit
    // count, and the name is NOT NUL-terminated.
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| SandboxError::ExecFailed("Windows path length overflowed".to_owned()))?;
    let bytes = rename_buffer_len(name_bytes)?;
    // The `usize` element type is what supplies the 8-byte alignment the
    // `RootDirectory` HANDLE field requires. A `u8` allocation would leave the
    // handle write below unaligned and unsound.
    let mut storage = vec![0_usize; bytes.div_ceil(std::mem::size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let name_length = u32::try_from(name_bytes)
        .map_err(|_| SandboxError::ExecFailed("Windows path is too long".to_owned()))?;
    let buffer_length = u32::try_from(bytes)
        .map_err(|_| SandboxError::ExecFailed("Windows rename buffer is too large".to_owned()))?;
    let mut status_block = zeroed_status_block();
    // SAFETY: `info` points at `storage`, a live `usize`-aligned allocation of at
    // least `rename_buffer_len(name_bytes)` bytes — the fixed
    // `FILE_RENAME_INFORMATION` header plus exactly `name_bytes` of trailing
    // name — so every field write and the `name.len()`-code-unit copy into the
    // trailing array stay inside that allocation. `name` and `storage` are
    // distinct allocations, so the copy cannot overlap. `storage` outlives both
    // calls below.
    unsafe {
        (*info).RootDirectory = target_parent.handle.as_raw_handle().cast();
        (*info).FileNameLength = name_length;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
    }

    // POSIX SEMANTICS ARE REQUIRED, NOT A NICETY — and this is measured, not
    // assumed. The CLASSIC rename class refuses to replace a destination that
    // any other handle currently has open, even one opened with
    // `FILE_SHARE_DELETE`. Measured on this hardware once the os-87 defect above
    // was repaired: a replace over an existing but UNOPENED destination
    // succeeds, while a replace over a destination a concurrent reader holds
    // open fails with os error 5. That is a production defect, not a test
    // artifact — `atomic_write_child` is the publish path for the polled
    // heartbeat status mirror, whose readers open exactly that file — and it
    // would also diverge from the unix branch, where `renameat` has always had
    // POSIX replace semantics.
    //
    // `FILE_RENAME_INFORMATION` doubles as the extended structure: the anonymous
    // union's `Flags` member is what the EX class reads, and every other field
    // (including `RootDirectory`) is identical, so the held-handle destination
    // guarantee above is UNCHANGED by which class is used.
    //
    // The extended class needs Windows 10 1709 or newer, so fall back to the
    // classic class exactly as the sibling `mark_open_object_for_delete` falls
    // back from `FileDispositionInfoEx`. The fallback is safe to take on ANY
    // failure: it degrades only the replace-over-open-destination behaviour, it
    // cannot mask a real refusal (a name collision or a missing DELETE right
    // fails identically under both classes, and the classic error is the one
    // surfaced), and it never changes how the destination is named.
    let mut flags = FILE_RENAME_POSIX_SEMANTICS;
    if replace {
        flags |= FILE_RENAME_REPLACE_IF_EXISTS;
    }
    // SAFETY: as above — `info` is a live, correctly sized and aligned
    // `FILE_RENAME_INFORMATION`; `status_block` and `storage` outlive the call;
    // `source` is owned by a live `File`. Writing the union's `Flags` member
    // initializes all four of its bytes, so no uninitialized byte reaches the
    // kernel.
    let extended = unsafe {
        (*info).Anonymous.Flags = flags;
        NtSetInformationFile(
            source.as_raw_handle().cast(),
            &mut status_block,
            info.cast(),
            buffer_length,
            FileRenameInformationEx,
        )
    };
    if extended >= 0 {
        return rename_status_completed(extended);
    }

    // SAFETY: as above. `Flags` is zeroed FIRST so the three bytes the extended
    // attempt wrote above `ReplaceIfExists` are cleared; the classic class reads
    // only the `ReplaceIfExists` byte, and both writes are fully initialized.
    let classic = unsafe {
        (*info).Anonymous.Flags = 0;
        (*info).Anonymous.ReplaceIfExists = u8::from(replace);
        NtSetInformationFile(
            source.as_raw_handle().cast(),
            &mut status_block,
            info.cast(),
            buffer_length,
            FileRenameInformation,
        )
    };
    if classic < 0 {
        return Err(ntstatus_error(classic).into());
    }
    rename_status_completed(classic)
}

/// Refuse a NON-NEGATIVE but incomplete rename status.
///
/// STATUS_PENDING is non-negative, so it would otherwise fall through as success
/// while the rename has NOT happened. Every handle this primitive is handed
/// today is opened `FILE_SYNCHRONOUS_IO_NONALERT`, but a future asynchronous
/// handle would silently report a publish that never occurred.
fn rename_status_completed(status: i32) -> Result<()> {
    if status == STATUS_PENDING {
        return Err(SandboxError::ExecFailed(
            "Windows handle-relative rename returned asynchronously; \
             this primitive requires a synchronous handle"
                .to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn rename_buffer_len(name_bytes: usize) -> Result<usize> {
    std::mem::size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(name_bytes)
        .ok_or_else(|| SandboxError::ExecFailed("Windows rename buffer overflowed".to_owned()))
}

pub(super) fn remove_descendants(authority: &DirectoryAuthority) -> Result<()> {
    loop {
        let entries = child_entries(authority)?;
        if entries.is_empty() {
            break;
        }
        for entry in entries {
            // PRECISE KIND, NOT `Any`. No single static access mask can serve
            // both child kinds, measured on this hardware: a DIRECTORY child
            // REQUIRES `FILE_GENERIC_WRITE` (this recursion terminates in
            // `authority.handle.sync_all()`, i.e. `FlushFileBuffers`, which
            // demands write access), while a READ-ONLY FILE child FORBIDS it
            // (the open itself is refused with os error 5). Git writes loose
            // objects and packfiles at mode 444, so read-only children are the
            // NORMAL case in every checkout. Asking for the union produced the
            // second failure and was reverted; the kind is made precise instead.
            //
            // TWO LAYERS, DELIBERATELY. The enumerated attribute is an
            // OBSERVATION taken before the open; the opened handle's metadata
            // below is the TRUTH after it. Passing a precise kind makes the
            // KERNEL refuse the open outright if the object's type changed
            // between enumeration and open (the open carries
            // `FILE_DIRECTORY_FILE` or `FILE_NON_DIRECTORY_FILE`), which is a
            // strengthening — it does NOT replace the post-open type check, the
            // identity read or the reparse refusal, all of which stay below.
            let kind = if entry.is_directory() {
                RelativeKind::Directory
            } else {
                RelativeKind::File
            };
            let name = entry.name;
            let handle = open_relative(authority, &name, kind, RelativeIntent::Mutate)?;
            let metadata = handle.metadata()?;
            if is_symlink_or_reparse(&metadata) {
                return Err(SandboxError::PathDenied(format!(
                    "refused linked Windows cleanup entry: {}",
                    authority.display_path.join(&name).display()
                )));
            }
            let identity = handle_directory_identity(&handle, &metadata)?;
            if metadata.is_dir() {
                let child = directory_authority(authority, &name, handle, identity);
                child.remove_open_dir_all().map_err(|boxed| boxed.0)?;
            } else if metadata.is_file() {
                delete_open_object(&handle, &authority.display_path.join(&name), "file")?;
            } else {
                return Err(SandboxError::PathDenied(format!(
                    "refused non-file Windows cleanup entry: {}",
                    authority.display_path.join(&name).display()
                )));
            }
        }
    }
    authority.handle.sync_all()?;
    Ok(())
}

pub(super) fn remove_open_dir_all(
    authority: DirectoryAuthority,
) -> std::result::Result<(), Box<(SandboxError, DirectoryAuthority)>> {
    if Arc::strong_count(&authority.handle) != 1
        || Arc::strong_count(&authority.display_path) != 1
        || authority.has_outstanding_handle_loans()
    {
        let error = SandboxError::PathDenied(format!(
            "retained Windows directory still has outstanding authority handles: {}",
            authority.display_path.display()
        ));
        return Err(Box::new((error, authority)));
    }
    if let Err(error) = remove_descendants(&authority) {
        return Err(Box::new((error, authority)));
    }
    let identity = authority.identity;
    let handle_loans = authority.handle_loans;
    let display_path = Arc::try_unwrap(authority.display_path).expect("strong count checked");
    let handle = Arc::try_unwrap(authority.handle).expect("strong count checked");
    if let Err(error) = delete_open_object(&handle, &display_path, "directory") {
        return Err(Box::new((
            error,
            DirectoryAuthority {
                handle: Arc::new(handle),
                identity,
                display_path: Arc::new(display_path),
                handle_loans,
            },
        )));
    }
    drop(handle);
    Ok(())
}

pub(super) fn remove_empty_child_directory(parent: &DirectoryAuthority, name: &str) -> Result<()> {
    let child = open_child_directory(parent, name)?;
    if !child_names(&child)?.is_empty() {
        return Err(SandboxError::PathDenied(format!(
            "refused to remove non-empty Windows directory: {}",
            parent.display_path.join(name).display()
        )));
    }
    remove_open_dir_all(child).map_err(|boxed| boxed.0)?;
    parent.handle.sync_all()?;
    Ok(())
}

fn open_relative(
    parent: &DirectoryAuthority,
    name: &str,
    kind: RelativeKind,
    intent: RelativeIntent,
) -> Result<File> {
    validate_windows_child_name(name)?;
    let mut wide: Vec<u16> = std::ffi::OsStr::new(name).encode_wide().collect();
    let byte_len = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| SandboxError::PathDenied("Windows child name is too long".to_owned()))?;
    let unicode_name = UNICODE_STRING {
        Length: byte_len,
        MaximumLength: byte_len,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.handle.as_raw_handle().cast(),
        ObjectName: &unicode_name,
        Attributes: if directory_is_case_sensitive(&parent.handle)? {
            0
        } else {
            OBJ_CASE_INSENSITIVE
        },
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut status_block = zeroed_status_block();
    let mut handle: HANDLE = std::ptr::null_mut();
    let desired_access = match (kind, intent) {
        (RelativeKind::File, RelativeIntent::ReadOnly) => FILE_GENERIC_READ | SYNCHRONIZE,
        (RelativeKind::Directory, RelativeIntent::ReadOnly) => FILE_GENERIC_READ | SYNCHRONIZE,
        (RelativeKind::File, RelativeIntent::Create) => {
            GENERIC_READ | GENERIC_WRITE | DELETE | SYNCHRONIZE
        }
        (RelativeKind::Directory, RelativeIntent::Create | RelativeIntent::Mutate) => {
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE
        }
        // THE ABSENCE OF THE WRITE BIT HERE IS LOAD-BEARING. Measured on this
        // hardware against a mode-444 child: `FILE_GENERIC_READ | DELETE |
        // SYNCHRONIZE` OPENS successfully and the extended disposition (with its
        // ignore-read-only flag) then deletes it, whereas
        // `FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE` FAILS
        // AT THE OPEN with os error 5. Git writes loose objects and packfiles
        // read-only, so this is the common path in every checkout, not an edge
        // case. Adding the write bit here to match the directory arm was
        // attempted, traded one os-5 for another, and was reverted. Do not
        // re-add it: the directory arm needs write because the cleanup recursion
        // flushes the directory handle, and no single mask satisfies both — which
        // is exactly why `remove_descendants` now passes a PRECISE kind.
        (RelativeKind::File, RelativeIntent::Mutate) => FILE_GENERIC_READ | DELETE | SYNCHRONIZE,
        // `LockFileEx` needs read or write access on the handle it locks, and
        // nothing else. The DELETE right is DELIBERATELY WITHHELD: this handle
        // never destroys the sentinel — removal happens through the parent's
        // ordinary descendant cleanup, which opens its own handle — and 20-72
        // proved on this hardware that a retained DELETE right is a real
        // functional hazard on Windows directories (it denies the chdir
        // `SetCurrentDirectory` performs). Least privilege here is a decision,
        // not an oversight.
        (RelativeKind::File, RelativeIntent::LockOpen | RelativeIntent::LockOpenOrCreate) => {
            GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE
        }
        // A lock target is a regular file BY CONSTRUCTION. Silently accepting a
        // directory here is the exact defect this intent exists to close, so
        // refuse explicitly rather than hand the locking layer an object on
        // which byte-range locking is undefined.
        (RelativeKind::Directory, RelativeIntent::LockOpen | RelativeIntent::LockOpenOrCreate) => {
            return Err(SandboxError::ExecFailed(
                "Windows advisory locks require a regular-file target".to_owned(),
            ));
        }
    };
    let type_options = match kind {
        RelativeKind::Directory => FILE_DIRECTORY_FILE,
        RelativeKind::File => FILE_NON_DIRECTORY_FILE,
    };
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &attributes,
            &mut status_block,
            std::ptr::null(),
            match kind {
                RelativeKind::Directory => FILE_ATTRIBUTE_DIRECTORY,
                RelativeKind::File => FILE_ATTRIBUTE_NORMAL,
            },
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            match intent {
                RelativeIntent::Create => FILE_CREATE,
                RelativeIntent::LockOpenOrCreate => FILE_OPEN_IF,
                RelativeIntent::ReadOnly | RelativeIntent::Mutate | RelativeIntent::LockOpen => {
                    FILE_OPEN
                }
            },
            FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | type_options,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        return Err(ntstatus_error(status).into());
    }
    if handle.is_null() {
        return Err(SandboxError::ExecFailed(
            "NtCreateFile succeeded without returning a handle".to_owned(),
        ));
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn directory_is_case_sensitive(handle: &File) -> Result<bool> {
    use windows_sys::Win32::Storage::FileSystem::{
        FileCaseSensitiveInfo, GetFileInformationByHandleEx,
    };

    let mut info = DirectoryCaseSensitiveInfo { flags: 0 };
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle().cast(),
            FileCaseSensitiveInfo,
            std::ptr::addr_of_mut!(info).cast(),
            std::mem::size_of::<DirectoryCaseSensitiveInfo>() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(info.flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0)
}

fn directory_authority(
    parent: &DirectoryAuthority,
    name: &str,
    handle: File,
    identity: DirectoryIdentity,
) -> DirectoryAuthority {
    DirectoryAuthority {
        handle: Arc::new(handle),
        identity,
        display_path: Arc::new(parent.display_path.join(name)),
        handle_loans: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
}

pub(super) fn checked_information_length(information: usize, capacity: usize) -> Result<usize> {
    if information > capacity {
        return Err(SandboxError::ExecFailed(format!(
            "Windows reported {information} directory bytes for a {capacity}-byte buffer"
        )));
    }
    Ok(information)
}

/// One enumerated directory child: its name and the attribute word the kernel
/// reported for it.
///
/// The attributes are an OBSERVATION taken before any handle is opened. They are
/// used only to select the KIND the subsequent open requests; the opened
/// handle's own metadata remains the truth, and both the post-open type check
/// and the reparse refusal in `remove_descendants` still apply.
#[derive(Clone, Debug)]
pub(super) struct DirectoryEntry {
    pub(super) name: String,
    pub(super) attributes: u32,
}

impl DirectoryEntry {
    fn is_directory(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }
}

pub(super) fn parse_directory_entries(
    buffer: *const u8,
    returned: usize,
    entries: &mut Vec<DirectoryEntry>,
) -> Result<()> {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let name_length_offset = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength);
    let attributes_offset = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileAttributes);
    let mut offset = 0_usize;
    loop {
        let remaining = returned.checked_sub(offset).ok_or_else(|| {
            SandboxError::ExecFailed("Windows directory entry offset overflowed".to_owned())
        })?;
        if remaining < header {
            return Err(SandboxError::ExecFailed(
                "Windows returned a truncated directory entry".to_owned(),
            ));
        }
        let entry = unsafe { buffer.add(offset) };
        let next = unsafe { entry.cast::<u32>().read_unaligned() } as usize;
        let name_bytes =
            unsafe { entry.add(name_length_offset).cast::<u32>().read_unaligned() } as usize;
        // IN-BOUNDS BY A GUARD THAT ALREADY EXISTS. `attributes_offset` is a
        // FIXED offset within the fixed part of the record (x64: 56) and is
        // STRICTLY LESS than `header`, which is `offset_of!(.., FileName)` (x64:
        // 104). The `remaining < header` refusal above has already proven that
        // at least `header` bytes of this record are present, so a 4-byte read
        // at `attributes_offset` lands strictly inside a region that guard
        // covers. This read therefore adds NO new bounds risk and the overflow
        // guard below needs no change. Read unaligned, exactly as
        // `NextEntryOffset` and `FileNameLength` above are, because a
        // fabricated buffer must never make us construct a misaligned
        // reference.
        let attributes = unsafe { entry.add(attributes_offset).cast::<u32>().read_unaligned() };
        let name_start = offset.checked_add(header).ok_or_else(|| {
            SandboxError::ExecFailed("Windows directory name offset overflowed".to_owned())
        })?;
        let entry_name_end = header.checked_add(name_bytes).ok_or_else(|| {
            SandboxError::ExecFailed("Windows directory name length overflowed".to_owned())
        })?;
        if !name_bytes.is_multiple_of(std::mem::size_of::<u16>())
            || name_bytes
                > returned.checked_sub(name_start).ok_or_else(|| {
                    SandboxError::ExecFailed("Windows directory name offset overflowed".to_owned())
                })?
        {
            return Err(SandboxError::ExecFailed(
                "Windows returned an invalid directory entry name".to_owned(),
            ));
        }
        if next != 0
            && (next < header
                || next > remaining
                || !next.is_multiple_of(8)
                || entry_name_end > next
                || offset.checked_add(next).is_none())
        {
            return Err(SandboxError::ExecFailed(
                "Windows returned an invalid directory entry offset".to_owned(),
            ));
        }
        // The kernel normally aligns entries, but hostile/fabricated buffers
        // must never make us create a misaligned u16 slice. Copy each code unit
        // with read_unaligned before decoding.
        let wide = (0..name_bytes / std::mem::size_of::<u16>())
            .map(|index| unsafe {
                buffer
                    .add(name_start + index * std::mem::size_of::<u16>())
                    .cast::<u16>()
                    .read_unaligned()
            })
            .collect::<Vec<_>>();
        let name = String::from_utf16(&wide).map_err(|_| {
            SandboxError::PathDenied("authority child name is not valid Unicode".to_owned())
        })?;
        if name != "." && name != ".." {
            validate_windows_child_name(&name)?;
            entries.push(DirectoryEntry { name, attributes });
        }
        if next == 0 {
            break;
        }
        offset += next;
    }
    Ok(())
}

pub(super) fn validate_windows_child_name(name: &str) -> Result<()> {
    validate_child_name(name)?;
    if name.chars().any(|character| {
        character <= '\u{1f}'
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
    }) || name.ends_with('.')
        || name.ends_with(' ')
    {
        return Err(SandboxError::PathDenied(format!(
            "Windows authority child has ambiguous namespace syntax: {name:?}"
        )));
    }

    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CLOCK$"
            | "CONIN$"
            | "CONOUT$"
            | "COM¹"
            | "COM²"
            | "COM³"
            | "LPT¹"
            | "LPT²"
            | "LPT³"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        return Err(SandboxError::PathDenied(format!(
            "Windows authority child uses a reserved device name: {name:?}"
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CreateValidationStage {
    Metadata,
    Type,
    Identity,
}

#[cfg(test)]
thread_local! {
    pub(super) static CREATE_VALIDATION_FAILURE: std::cell::Cell<Option<CreateValidationStage>> =
        const { std::cell::Cell::new(None) };
    static BEFORE_ATOMIC_FILE_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn set_before_atomic_file_rename_hook(hook: Option<Box<dyn FnOnce()>>) {
    BEFORE_ATOMIC_FILE_RENAME.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
fn run_before_atomic_file_rename_hook() {
    if let Some(hook) = BEFORE_ATOMIC_FILE_RENAME.with(|slot| slot.borrow_mut().take()) {
        hook();
    }
}

fn maybe_fail_created_validation(stage: CreateValidationStage) -> Result<()> {
    #[cfg(test)]
    if CREATE_VALIDATION_FAILURE.with(|failure| failure.get() == Some(stage)) {
        return Err(SandboxError::ExecFailed(format!(
            "injected Windows {stage:?} validation failure"
        )));
    }
    #[cfg(not(test))]
    let _ = stage;
    Ok(())
}

fn ntstatus_error(status: i32) -> std::io::Error {
    let code = unsafe { RtlNtStatusToDosError(status) };
    std::io::Error::from_raw_os_error(code as i32)
}

fn zeroed_status_block() -> IO_STATUS_BLOCK {
    unsafe { std::mem::zeroed() }
}

// The Windows retained-handle proof is mounted as `directory_authority::tests`
// (see `directory_authority.rs`) so its identities are `directory_authority::
// tests::windows_*`; it is not a child module of `windows` here.
