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
    FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    FileIdBothDirectoryInformation, NtCreateFile, NtQueryDirectoryFile,
};
use windows_sys::Win32::Foundation::{
    GENERIC_READ, GENERIC_WRITE, HANDLE, RtlNtStatusToDosError, STATUS_BUFFER_OVERFLOW,
    STATUS_BUFFER_TOO_SMALL, STATUS_NO_MORE_FILES, UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

const OBJ_CASE_INSENSITIVE: u32 = 0x40;
pub(super) const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x1;

#[repr(C)]
struct DirectoryCaseSensitiveInfo {
    flags: u32,
}

#[derive(Clone, Copy)]
enum RelativeKind {
    Directory,
    File,
    Any,
}

#[derive(Clone, Copy)]
enum RelativeIntent {
    ReadOnly,
    Mutate,
    Create,
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

pub(super) fn child_names(parent: &DirectoryAuthority) -> Result<Vec<String>> {
    let mut names = Vec::new();
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
        parse_directory_entries(storage.as_ptr().cast(), returned, &mut names)?;
    }

    names.sort();
    names.dedup();
    Ok(names)
}

pub(super) fn open_child_file(
    parent: &DirectoryAuthority,
    name: &str,
) -> Result<RegularFileAuthority> {
    let handle = open_relative(parent, name, RelativeKind::File, RelativeIntent::ReadOnly)?;
    let metadata = handle.metadata()?;
    validate_real_file(Path::new("<retained child>"), &metadata)?;
    let identity = handle_directory_identity(&handle, &metadata)?;
    Ok(RegularFileAuthority {
        handle,
        identity,
        display_path: parent.display_path.join(name),
    })
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

fn rename_handle_into(
    source: &File,
    target_parent: &DirectoryAuthority,
    name: &str,
    replace: bool,
) -> Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FILE_RENAME_INFO_0, FileRenameInfo, SetFileInformationByHandle,
    };

    validate_windows_child_name(name)?;
    let name: Vec<u16> = std::ffi::OsStr::new(name).encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| SandboxError::ExecFailed("Windows path length overflowed".to_owned()))?;
    let bytes = rename_buffer_len(name_bytes)?;
    let mut storage = vec![0_usize; bytes.div_ceil(std::mem::size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous = FILE_RENAME_INFO_0 {
            ReplaceIfExists: u8::from(replace),
        };
        (*info).RootDirectory = target_parent.handle.as_raw_handle().cast();
        (*info).FileNameLength = u32::try_from(name_bytes)
            .map_err(|_| SandboxError::ExecFailed("Windows path is too long".to_owned()))?;
        std::ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());
        if SetFileInformationByHandle(
            source.as_raw_handle().cast(),
            FileRenameInfo,
            info.cast(),
            u32::try_from(bytes).map_err(|_| {
                SandboxError::ExecFailed("Windows rename buffer is too large".to_owned())
            })?,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

pub(super) fn rename_buffer_len(name_bytes: usize) -> Result<usize> {
    std::mem::size_of::<windows_sys::Win32::Storage::FileSystem::FILE_RENAME_INFO>()
        .checked_add(name_bytes)
        .ok_or_else(|| SandboxError::ExecFailed("Windows rename buffer overflowed".to_owned()))
}

pub(super) fn remove_descendants(authority: &DirectoryAuthority) -> Result<()> {
    loop {
        let names = child_names(authority)?;
        if names.is_empty() {
            break;
        }
        for name in names {
            let handle =
                open_relative(authority, &name, RelativeKind::Any, RelativeIntent::Mutate)?;
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
        (RelativeKind::File | RelativeKind::Any, RelativeIntent::Mutate) => {
            FILE_GENERIC_READ | DELETE | SYNCHRONIZE
        }
        (RelativeKind::Any, RelativeIntent::ReadOnly) => FILE_GENERIC_READ | SYNCHRONIZE,
        (RelativeKind::Any, RelativeIntent::Create) => {
            return Err(SandboxError::ExecFailed(
                "Windows cannot create an authority with an unknown type".to_owned(),
            ));
        }
    };
    let type_options = match kind {
        RelativeKind::Directory => FILE_DIRECTORY_FILE,
        RelativeKind::File => FILE_NON_DIRECTORY_FILE,
        RelativeKind::Any => 0,
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
                RelativeKind::File | RelativeKind::Any => FILE_ATTRIBUTE_NORMAL,
            },
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            if matches!(intent, RelativeIntent::Create) {
                FILE_CREATE
            } else {
                FILE_OPEN
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

fn checked_information_length(information: usize, capacity: usize) -> Result<usize> {
    if information > capacity {
        return Err(SandboxError::ExecFailed(format!(
            "Windows reported {information} directory bytes for a {capacity}-byte buffer"
        )));
    }
    Ok(information)
}

fn parse_directory_entries(
    buffer: *const u8,
    returned: usize,
    names: &mut Vec<String>,
) -> Result<()> {
    let header = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    let name_length_offset = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength);
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
        let name_start = offset.checked_add(header).ok_or_else(|| {
            SandboxError::ExecFailed("Windows directory name offset overflowed".to_owned())
        })?;
        let entry_name_end = header.checked_add(name_bytes).ok_or_else(|| {
            SandboxError::ExecFailed("Windows directory name length overflowed".to_owned())
        })?;
        if name_bytes % std::mem::size_of::<u16>() != 0
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
                || next % 8 != 0
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
            names.push(name);
        }
        if next == 0 {
            break;
        }
        offset += next;
    }
    Ok(())
}

fn validate_windows_child_name(name: &str) -> Result<()> {
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
