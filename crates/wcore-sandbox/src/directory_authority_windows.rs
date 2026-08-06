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

/// Read-only sibling of [`open_child_directory`], for IDENTITY PROOFS.
///
/// The ONLY difference is the desired access: `FILE_GENERIC_READ | SYNCHRONIZE`
/// instead of the mutating mask's added `FILE_GENERIC_WRITE | DELETE`. The
/// DELETE right is share-arbitrated, so the mutating form is refused with
/// ERROR_SHARING_VIOLATION while any handle on the child omits
/// `FILE_SHARE_DELETE` — a live process whose current directory is that child
/// being the everyday case, not an edge case. Identity is FileId/volume based
/// and needs only read access, so a proof should pay no share-arbitration cost.
pub(super) fn open_child_directory_observational(
    parent: &DirectoryAuthority,
    name: &str,
) -> Result<DirectoryAuthority> {
    let handle = open_relative(
        parent,
        name,
        RelativeKind::Directory,
        RelativeIntent::ReadOnly,
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
/// Open an existing child file with the DELETE right, so it can be removed
/// through the returned authority.
///
/// # Why this open is retried
///
/// `RelativeIntent::Mutate` asks for `DELETE`, and Windows refuses a
/// DELETE-bearing open with ERROR_SHARING_VIOLATION while ANY other handle on
/// the object omits `FILE_SHARE_DELETE` — including a handle held for
/// microseconds by an on-access virus scanner, which is the default posture on
/// both a developer box and a hosted `windows-2022` runner. Measured: the
/// nightly soak went red on `crash_after_descendant_removal_recovers_original_before_reads`
/// and `mid_import_failure_rolls_back_to_original_tree`, both of which reach
/// here through `remove_journal`, and both of which are the CRASH RECOVERY and
/// ROLLBACK paths. A transient scanner handle was making durable recovery fail.
///
/// The retry uses the same bounded schedule as the sibling `remove_descendants`.
/// It does NOT weaken the retained-handle pin: a pin is held for the life of a
/// lease, so it outlasts every backoff step and the open still ends in the same
/// refusal — only ~785ms later.
///
/// WHAT THE RETRY DOES NOT DO, measured. The premise above ("the refusal is
/// transient, so backoff outlasts it") was tested and REFUTED: pinned to 2
/// logical CPUs, 20 iterations of the full suite failed 6/20 without any retry
/// and 5/20 with it — indistinguishable at that sample size. Whatever holds the
/// conflicting handle survives ~785ms of backoff, so it is not a scanner's
/// microsecond handle. The retry is kept because it is cheap and correct for the
/// transient case it does cover; it is NOT a fix for the soak failure, and the
/// refusal that survives it is now named (see `name_share_violation`) precisely
/// so the next occurrence identifies its own call site instead of arriving as a
/// bare `os error 32`.
pub(super) fn open_child_file_for_removal(
    parent: &DirectoryAuthority,
    name: &str,
) -> Result<RegularFileAuthority> {
    retry_while_share_violated(
        || open_child_file_with(parent, name, RelativeIntent::Mutate),
        std::thread::sleep,
    )
    .map_err(|error| {
        if !is_share_violation(&error) {
            return error;
        }
        name_share_violation(
            error,
            &format!(
                "delete-bearing open of child file ({})",
                holders_of_child(parent, name, RelativeKind::File)
            ),
            &parent.display_path.join(name),
        )
    })
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
    // Capture the EXTENDED failure before the classic fallback overwrites the
    // thread's last-error. Without this the reported errno is only the classic
    // attempt's, so a POSIX-semantics rejection (which is a different diagnosis
    // entirely) is indistinguishable from a share-arbitration refusal.
    let extended_error = std::io::Error::last_os_error();

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
        // A sharing violation here MUST stay typed. The delete disposition is
        // one of the operations Windows refuses with ERROR_SHARING_VIOLATION
        // while another handle on the object omits FILE_SHARE_DELETE, and
        // `is_share_violation` matches `SandboxError::Io` on a RAW errno.
        // Stringifying it into `ExecFailed` — which is what this did — made the
        // 32 invisible, so any retry wrapper placed around this call was dead
        // code that could never fire.
        //
        // Context cannot ride along on the same value: `io::Error::new(kind,
        // message)` reports `raw_os_error() == None`, which is exactly what
        // re-hides the errno. So the split is by consequence, not by taste —
        // the retryable case keeps the bare errno because a caller has to act
        // on it, and every other failure keeps the descriptive message because
        // nothing retries it and a human reads it.
        let source = std::io::Error::last_os_error();
        if source.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
            return Err(SandboxError::ShareViolation {
                operation: format!(
                    "delete disposition on retained {kind} (extended attempt: {extended_error}; {})",
                    holders_of_open_object(handle)
                ),
                path: path.to_path_buf(),
                source,
            });
        }
        return Err(SandboxError::ExecFailed(format!(
            "delete retained Windows {kind} {}: {source} (extended attempt: {extended_error})",
            path.display()
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
    // Retried, and this is the FIRST retry this path has ever had.
    //
    // WHY HERE, when retrying elsewhere was measured not to help. The earlier
    // retries were added to the DELETE sites, and the nightly soak then produced
    // no named refusal on any of them — the refusal was never on a delete. Soak
    // 31067604715 named this one instead: the publish rename inside
    // `atomic_write_child`, restoring `...\checkout\keep` during an import
    // rollback.
    //
    // WHY IT SHOULD WORK HERE, when it did not there. That run also showed the
    // destination did not exist (`os error 2` reopening it), so the contended
    // object is the SOURCE — a temporary file this process wrote microseconds
    // earlier and is now unlinking by renaming. A freshly written file is what an
    // on-access scanner opens, and a scanner's handle IS transient, which is the
    // one shape backoff can outlast. The holders that defeated the earlier
    // retries were a lease and a live child process, neither of which clears on
    // its own.
    //
    // The test hook above fires ONCE, outside the loop, so a fault-injection test
    // cannot be silently retried into passing.
    retry_while_share_violated(
        || rename_handle_into(&source.handle, target_parent, name, replace),
        std::thread::sleep,
    )
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
    // Kept before `name` is shadowed by its UTF-16 form: a refusal below has to
    // be able to say WHICH object it was refused on.
    let child_name = name.to_owned();
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
        // The PUBLISH refusal, named. `atomic_write_child` republishes through
        // this rename, and `replace_tree` restores every original file through
        // `atomic_write_child`, so this is the path an import ROLLBACK takes —
        // the one the nightly soak fails on with
        // "durable recovery failed (... os error 32)". Until now it returned a
        // bare errno with no path and no operation, which is why that soak
        // message named no file and why the first round of instrumentation, aimed
        // at the delete sites, produced no named refusal at all.
        //
        // BOTH ENDS ARE REPORTED, because a rename can be refused from either.
        // The first version of this probe asked only about the DESTINATION, on
        // the reasoning that a replace-rename must delete it. Measured on soak
        // run 31067604715, that was the wrong end: the destination reopen failed
        // with os error 2 — the destination did not exist — while the rename was
        // still refused with a sharing violation. A rename also unlinks the
        // SOURCE name, so the source is share-arbitrated too, and here the source
        // is a temporary file this process wrote microseconds earlier, which is
        // exactly what an on-access scanner opens.
        //
        // The source handle is already in hand, so asking it costs one syscall
        // and needs no reopen that could fail the way the destination's did.
        //
        // The extended status is carried too: the classic fallback overwrites it,
        // so without this the reported error is only the second attempt's. That
        // run reported `0xc0000043` (STATUS_SHARING_VIOLATION) for the extended
        // attempt, so BOTH rename classes were refused the same way and the
        // POSIX-semantics fallback is not implicated.
        let error = ntstatus_error(classic);
        if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
            return Err(SandboxError::ShareViolation {
                operation: format!(
                    "handle-relative publish rename (replace={replace}; extended NTSTATUS \
                     {extended:#010x}; source {}; destination {})",
                    holders_of_open_object(source),
                    holders_of_child(target_parent, &child_name, RelativeKind::File)
                ),
                path: target_parent.display_path.join(&child_name),
                source: error,
            });
        }
        return Err(error.into());
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

/// Win32 `ERROR_SHARING_VIOLATION`.
pub(super) const ERROR_SHARING_VIOLATION: i32 = 32;

/// Backoff schedule for a share-arbitration refusal during destructive
/// cleanup, in milliseconds. Seven attempts, ~785 ms of patience in total.
///
/// MEASURED on SeanDesktop (32 CPUs, four concurrent test processes, 24 runs
/// of the swarm output-exhaustion suite): every one of the 8 refusals observed
/// cleared on the FIRST re-attempt 10 ms later — 8 of 8, none needing a second.
/// The tail beyond that first step is headroom for a slower host, not an
/// expectation. Nothing here is unbounded: once the schedule is spent the
/// caller receives the last refusal unchanged.
pub(super) const SHARE_VIOLATION_BACKOFF_MS: &[u64] = &[10, 25, 50, 100, 200, 400];

/// Is this refusal the Windows share-arbitration transient, PROVEN by errno?
///
/// The proof is positive and narrow: only [`SandboxError::Io`] carries a typed
/// `std::io::Error`, and only errno 32 is the sharing violation. Every SECURITY
/// refusal on the cleanup path is a different variant — `PathDenied` for a
/// linked or non-file entry and for a retained directory with outstanding
/// authority handles, `ExecFailed` for a failed disposition — so a refusal can
/// never be retried by accident. No string matching is involved, deliberately:
/// `io::Error`'s Display carries the LOCALIZED system message and would differ
/// on a non-English host.
pub(super) fn is_share_violation(error: &SandboxError) -> bool {
    match error {
        SandboxError::Io(error) => error.raw_os_error() == Some(ERROR_SHARING_VIOLATION),
        // Already named by `name_share_violation`. Both spellings must match or
        // attaching diagnostic context would silently disarm every retry that
        // sits OUTSIDE the site that attached it.
        SandboxError::ShareViolation { .. } => true,
        _ => false,
    }
}

/// Name the object and the operation in a share-arbitration refusal, keeping the
/// errno raw so [`is_share_violation`] still matches.
///
/// Applied at the OUTERMOST edge of each refusal-bearing operation, never inside
/// a retry loop's attempt: the loop's own matcher handles both spellings, and
/// naming on every attempt would only rewrite the same value repeatedly.
/// `FileProcessIdsUsingFileInformation`. Not re-exported by `windows_sys`, so the
/// class number is spelled out; it is stable ABI since Vista.
const FILE_PROCESS_IDS_USING_FILE_INFORMATION: i32 = 47;

/// Identify one holder: PID, image name, and whether it is still running.
///
/// # Why the image name and the liveness matter
///
/// `FileProcessIdsUsingFileInformation` answers "who has this object open". It
/// does NOT answer "who is BLOCKING this open" — those are different questions,
/// and conflating them reads a harmless holder as the culprit. Every handle this
/// crate opens on a file permits deletion, so OUR OWN pid routinely appears in
/// the list without being the blocker. A DIFFERENT process appearing is the
/// interesting case, because a child's current-directory handle omits
/// `FILE_SHARE_DELETE` and a virus scanner's may too.
///
/// Liveness separates the two remedies. A holder still RUNNING must be waited
/// for or reaped. A holder already EXITED whose handle has not yet been torn
/// down can only be outlasted, and tells us the drain reported zero too early.
fn describe_holder(pid: usize) -> String {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    if pid == std::process::id() as usize {
        return format!("{pid} SELF");
    }
    let Ok(raw) = u32::try_from(pid) else {
        return format!("{pid} (pid out of range)");
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, raw) };
    if handle.is_null() {
        // Almost always "the process is gone", which is itself a result: the
        // holder died and its handle outlived it.
        return format!("{pid} (gone or inaccessible)");
    }
    let mut name = [0u16; 260];
    let mut len = name.len() as u32;
    let image =
        if unsafe { QueryFullProcessImageNameW(handle, 0, name.as_mut_ptr(), &mut len) } != 0 {
            String::from_utf16_lossy(&name[..len as usize])
        } else {
            "<unknown image>".to_owned()
        };
    let mut code: u32 = 0;
    let alive = unsafe { GetExitCodeProcess(handle, &mut code) } != 0 && code == STILL_ACTIVE;
    unsafe { CloseHandle(handle) };
    let short = image.rsplit('\\').next().unwrap_or(&image).to_owned();
    format!("{pid} {short} {}", if alive { "RUNNING" } else { "EXITED" })
}

/// `STILL_ACTIVE` is `STATUS_PENDING` reused as a process exit code.
const STILL_ACTIVE: u32 = 259;

/// Name the processes holding a handle on this object, for a refusal that has
/// ALREADY happened.
///
/// # Why this shape, and not handle enumeration
///
/// The refusal being diagnosed reproduces only under 2-core contention inside a
/// full 3244-test run, so any probe that perturbs timing hides the thing it is
/// meant to catch. This is ONE syscall scoped to ONE object, taken only after a
/// terminal failure: it cannot slow a successful path because it never runs on
/// one. The rejected alternatives all fail that constraint — Restart Manager is
/// an RPC round trip that also refuses directories, `handle.exe` spawns a process
/// and enumerates the whole system, and `NtQuerySystemInformation` walks the
/// system-wide handle table behind a global lock.
///
/// Purely diagnostic: every failure to collect the list is reported as text and
/// none is escalated, because this runs while a real error is already in flight
/// and must never replace it.
fn holders_of_open_object(handle: &File) -> String {
    use windows_sys::Wdk::Storage::FileSystem::NtQueryInformationFile;

    let mut buffer = [0u8; 512];
    let mut status_block = zeroed_status_block();
    let status = unsafe {
        NtQueryInformationFile(
            handle.as_raw_handle().cast(),
            &mut status_block,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            FILE_PROCESS_IDS_USING_FILE_INFORMATION,
        )
    };
    if status < 0 {
        return format!("holders unavailable (NTSTATUS {status:#010x})");
    }

    // FILE_PROCESS_IDS_USING_FILE_INFORMATION is `ULONG NumberOfProcessIdsInList`
    // followed by `ULONG_PTR ProcessIdList[]`. The array is pointer-aligned, so
    // the first entry starts one pointer width in, not four bytes in.
    let stride = std::mem::size_of::<usize>();
    let Some(count_bytes) = buffer.get(0..4) else {
        return "holders unavailable (short buffer)".to_owned();
    };
    let count = u32::from_ne_bytes(count_bytes.try_into().expect("4 bytes")) as usize;
    let mut holders = Vec::new();
    for index in 0..count {
        let start = stride + index * stride;
        let Some(entry) = buffer.get(start..start + stride) else {
            holders.push("...truncated".to_owned());
            break;
        };
        let pid = usize::from_ne_bytes(entry.try_into().expect("pointer-width bytes"));
        holders.push(describe_holder(pid));
    }
    // SELF-VALIDATION. The caller passes a handle it is HOLDING OPEN, so this
    // process must appear in any correct answer. When it does not, the result is
    // not "nobody holds it" — it is "this query did not work", and saying the
    // former would be a confident lie.
    //
    // Measured on soak 31070941379: this returned an empty list for the publish
    // rename's source handle while that very handle was open, and the earlier
    // wording reported it as "no holders reported", which reads as evidence of a
    // transient holder. It is not evidence of anything. The same probe returned
    // real pids for a directory handle on another host, so the failure is
    // specific and not yet understood; until it is, it must announce itself.
    let ours = std::process::id() as usize;
    let saw_self = holders.iter().any(|h| h.starts_with(&format!("{ours} ")));
    if !saw_self {
        return format!(
            "holder probe UNRELIABLE (returned {} entries and none is this process, \
             which holds the handle) — draw no conclusion from this",
            holders.len()
        );
    }
    format!("held by [{}]", holders.join(", "))
}

fn name_share_violation(error: SandboxError, operation: &str, path: &Path) -> SandboxError {
    match error {
        SandboxError::Io(source) if source.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => {
            SandboxError::ShareViolation {
                operation: operation.to_owned(),
                path: path.to_path_buf(),
                source,
            }
        }
        other => other,
    }
}

/// The refusal happened at OPEN, so there is no handle to interrogate. Reopen the
/// same child observationally — that open shares delete, so it is not the open
/// that was just refused and it succeeds against the very holder that caused the
/// refusal — then ask the object who holds it.
///
/// Best-effort by construction: if even the observational open is refused, the
/// reason is reported instead of the holders, and the caller's original error is
/// still what propagates.
fn holders_of_child(parent: &DirectoryAuthority, name: &str, kind: RelativeKind) -> String {
    match open_relative(parent, name, kind, RelativeIntent::ReadOnly) {
        Ok(handle) => holders_of_open_object(&handle),
        Err(error) => format!("holders unavailable (diagnostic reopen failed: {error})"),
    }
}

/// Run `attempt`, re-running it on the bounded schedule for as long as it is
/// refused by share arbitration and ONLY for that reason.
///
/// # Why the retry lives here and not at the swarm's cleanup call site
///
/// The obvious place to retry is `dispatch.rs`, around `release_transaction`.
/// It is the wrong place twice over. First, by the time the error arrives
/// there it has been flattened to `SwarmError::WorktreeIo(String)` and the
/// errno is gone, so the retry could only be gated by matching a localized
/// message. Second — and this is the one that bites — `TransactionCleanup::
/// release` closes the transaction lease INSIDE the swarm critical section and
/// drops the swarm sentinel when it returns. Between two attempts up there the
/// root is observably lease-free with its reservation receipt still on disk,
/// which is exactly the state `reclaim_abandoned_transactions` treats as
/// abandoned. A peer PROCESS is not excluded by the in-process reservation
/// registry, so a retry loop at that level would reopen the cross-process
/// reclaim window that ordering was written to close.
///
/// Retrying here keeps both locks held for the whole sequence and keeps the
/// error typed, so neither problem arises.
///
/// `sleep` is injected so the policy is provable without real time.
pub(super) fn retry_while_share_violated<T>(
    mut attempt: impl FnMut() -> Result<T>,
    mut sleep: impl FnMut(std::time::Duration),
) -> Result<T> {
    let mut last = match attempt() {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    for backoff in SHARE_VIOLATION_BACKOFF_MS {
        if !is_share_violation(&last) {
            return Err(last);
        }
        sleep(std::time::Duration::from_millis(*backoff));
        last = match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
    }
    Err(last)
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
            // The delete-bearing open is the one refusal point measured on real
            // hardware: a process that has just been terminated has not yet
            // dropped the handle its current directory held, and Windows refuses
            // a `DELETE` open while any handle omits `FILE_SHARE_DELETE`. The
            // job is drained to zero active processes before cleanup starts (see
            // the AppContainer backend), and instrumentation showed that drain
            // reporting zero in 0 ms on all 147 observations while the refusal
            // still occurred — so the residual holder is outside the job and
            // cannot be waited for directly. It can only be outlasted, and it
            // clears in about 10 ms.
            let handle = retry_while_share_violated(
                || open_relative(authority, &name, kind, RelativeIntent::Mutate),
                std::thread::sleep,
            )
            .map_err(|error| {
                if !is_share_violation(&error) {
                    return error;
                }
                name_share_violation(
                    error,
                    &format!(
                        "delete-bearing open of cleanup entry ({})",
                        holders_of_child(authority, &name, kind)
                    ),
                    &authority.display_path.join(&name),
                )
            })?;
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
    // The SELF-delete is retried, not just the descendants above.
    //
    // `remove_descendants` has been retried since the swarm-cleanup fix, but the
    // directory's own delete disposition was not, and that is the step that
    // survived: measured on SeanDesktop pinned to 2 logical CPUs (which is what
    // the hosted windows-2022 runner has), `malformed_heartbeat_fails_closed_
    // and_preserves_bounded_diagnostic` still failed 2 of 20 runs with
    // "transaction cleanup: worktree io: ... (os error 32)" after the descendant
    // and read-walk fixes, reaching here through `TransactionCleanup::release`
    // -> `remove_transaction_root` -> `remove_open_dir_all`.
    //
    // Emptying a directory and then being refused permission to remove the
    // directory itself is the same transient share arbitration one step later:
    // a scanner that opened the last child can still hold the parent when the
    // disposition is set. This retry is only reachable because the sibling
    // `mark_open_object_for_delete` now returns the sharing violation TYPED —
    // while it was stringified into `ExecFailed`, `is_share_violation` could not
    // see it and this wrapper would have been dead code.
    //
    // The handle is reused rather than reopened: the disposition is set on the
    // handle we already hold, so there is nothing to re-resolve between
    // attempts and no window for the directory to be swapped underneath us.
    if let Err(error) = retry_while_share_violated(
        || delete_open_object(&handle, &display_path, "directory"),
        std::thread::sleep,
    ) {
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
