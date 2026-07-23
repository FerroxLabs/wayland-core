use super::*;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileDispositionInfo, GetFileInformationByHandle, GetFinalPathNameByHandleW,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, SetFileInformationByHandle,
    VOLUME_NAME_DOS,
};

const MAX_LEASE_BYTES: u64 = 1024 * 1024;
const TEMP_ATTEMPTS: u64 = 64;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    volume_serial: u32,
    index_high: u32,
    index_low: u32,
}

struct TrustedRoot {
    path: PathBuf,
    final_path: PathBuf,
    file: File,
    identity: FileIdentity,
}

pub(super) fn lease_directory() -> Result<PathBuf> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        exec_error("LOCALAPPDATA is required for AppContainer ACL leases".into())
    })?);
    lease_directory_from(local)
}

fn lease_directory_from(local: PathBuf) -> Result<PathBuf> {
    let local_root = open_directory_nofollow(&local, "LOCALAPPDATA")?;
    validate_local_canonical_path(&local_root.final_path)?;

    let mut root = local_root;
    for component in LEASE_DIRECTORY_COMPONENTS {
        root = create_or_open_child_directory(&root, component)?;
    }
    Ok(root.path)
}

pub(super) fn write_new_synced_lease(path: &Path, lease: &LeaseFile) -> Result<()> {
    let root = trusted_root_for(path)?;
    let serialized = serialize(lease)?;
    let mut file = create_new_nofollow(&root, path)?;
    write_and_sync(&mut file, path, serialized.as_bytes())?;
    validate_open_file(&root, path, &file)?;
    sync_root(&root)?;
    Ok(())
}

pub(super) fn rewrite_synced_lease(path: &Path, lease: &LeaseFile) -> Result<()> {
    rewrite_with_hook(path, lease, |_| Ok(()), true)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RewritePhase {
    TempCreated,
    TempSynced,
    Replaced,
}

#[cfg(not(test))]
#[derive(Clone, Copy)]
enum RewritePhase {
    TempCreated,
    TempSynced,
    Replaced,
}

fn rewrite_with_hook(
    path: &Path,
    lease: &LeaseFile,
    mut hook: impl FnMut(RewritePhase) -> Result<()>,
    clean_temp_on_error: bool,
) -> Result<()> {
    let root = trusted_root_for(path)?;
    let existing = open_existing_nofollow(&root, path, GENERIC_READ)?;
    let existing_identity = file_identity(&existing, path)?;
    drop(existing);

    let serialized = serialize(lease)?;
    let (temp_path, mut temp) = create_rewrite_temp(&root, path)?;
    if let Err(error) = hook(RewritePhase::TempCreated) {
        return interrupt_rewrite(&root, &temp_path, error, clean_temp_on_error);
    }
    write_and_sync(&mut temp, &temp_path, serialized.as_bytes())?;
    validate_open_file(&root, &temp_path, &temp)?;
    if let Err(error) = hook(RewritePhase::TempSynced) {
        return interrupt_rewrite(&root, &temp_path, error, clean_temp_on_error);
    }
    drop(temp);

    let current = open_existing_nofollow(&root, path, GENERIC_READ)?;
    if file_identity(&current, path)? != existing_identity {
        return interrupt_rewrite(
            &root,
            &temp_path,
            exec_error(format!(
                "lease identity changed before replace: {}",
                path.display()
            )),
            clean_temp_on_error,
        );
    }
    drop(current);

    let temp_w = widen_path(&temp_path);
    let target_w = widen_path(path);
    if unsafe {
        MoveFileExW(
            temp_w.as_ptr(),
            target_w.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return interrupt_rewrite(
            &root,
            &temp_path,
            last_error("MoveFileExW(AppContainer ACL lease replace)"),
            clean_temp_on_error,
        );
    }

    let replaced = open_existing_nofollow(&root, path, GENERIC_READ)?;
    validate_open_file(&root, path, &replaced)?;
    drop(replaced);
    sync_root(&root)?;
    hook(RewritePhase::Replaced)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn rewrite_synced_lease_with_crash(
    path: &Path,
    lease: &LeaseFile,
    crash_at: RewritePhase,
) -> Result<()> {
    rewrite_with_hook(
        path,
        lease,
        |phase| {
            if phase == crash_at {
                Err(exec_error(format!("injected rewrite crash at {phase:?}")))
            } else {
                Ok(())
            }
        },
        false,
    )
}

pub(super) fn read_validated_lease(path: &Path) -> Result<LeaseFile> {
    let root = trusted_root_for(path)?;
    let mut file = open_existing_nofollow(&root, path, GENERIC_READ)?;
    let metadata = file
        .metadata()
        .map_err(|error| exec_error(format!("stat ACL lease {}: {error}", path.display())))?;
    if metadata.len() == 0 || metadata.len() > MAX_LEASE_BYTES {
        return Err(exec_error(format!(
            "invalid AppContainer ACL lease size {} in {}",
            metadata.len(),
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| exec_error(format!("read ACL lease {}: {error}", path.display())))?;
    validate_open_file(&root, path, &file)?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        exec_error(format!(
            "ACL lease {} is not UTF-8: {error}",
            path.display()
        ))
    })?;
    let lease: LeaseFile = toml::from_str(text).map_err(|error| {
        exec_error(format!(
            "malformed or unknown AppContainer ACL lease {}: {error}",
            path.display()
        ))
    })?;
    lease.validate(path)?;
    Ok(lease)
}

pub(super) fn remove_validated_lease(path: &Path) -> Result<()> {
    let root = trusted_root_for(path)?;
    let file = open_existing_nofollow(&root, path, GENERIC_READ | DELETE)?;
    validate_open_file(&root, path, &file)?;
    delete_open_file(&file, path)?;
    drop(file);
    confirm_path_absent(path)?;
    sync_root(&root)
}

pub(super) fn recover_rewrite_temps(root_path: &Path) -> Result<()> {
    let root = trusted_root(root_path)?;
    let entries = fs::read_dir(root_path).map_err(|error| {
        exec_error(format!(
            "read AppContainer ACL lease directory {}: {error}",
            root_path.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| exec_error(format!("read ACL temp entry: {error}")))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_rewrite_temp_name(name) {
            continue;
        }
        let path = entry.path();
        let file = open_existing_nofollow(&root, &path, GENERIC_READ | DELETE)?;
        validate_open_file(&root, &path, &file)?;
        delete_open_file(&file, &path)?;
        drop(file);
        confirm_path_absent(&path)?;
    }
    sync_root(&root)
}

fn serialize(lease: &LeaseFile) -> Result<String> {
    toml::to_string(lease)
        .map_err(|error| exec_error(format!("serialize AppContainer ACL lease: {error}")))
}

fn write_and_sync(file: &mut File, path: &Path, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes).map_err(|error| {
        exec_error(format!(
            "write AppContainer ACL lease {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        exec_error(format!(
            "fsync AppContainer ACL lease {}: {error}",
            path.display()
        ))
    })
}

fn create_rewrite_temp(root: &TrustedRoot, target: &Path) -> Result<(PathBuf, File)> {
    let target_name = target
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| exec_error(format!("invalid lease filename: {}", target.display())))?;
    let start = TEMP_COUNTER.fetch_add(TEMP_ATTEMPTS, Ordering::Relaxed);
    for offset in 0..TEMP_ATTEMPTS {
        let name = format!(
            "{target_name}.rewrite-{:08x}-{:016x}.tmp",
            std::process::id(),
            start + offset
        );
        let path = root.path.join(name);
        match create_new_nofollow(root, &path) {
            Ok(file) => return Ok((path, file)),
            Err(_) if path.exists() => continue,
            Err(error) => return Err(error),
        }
    }
    Err(exec_error(
        "could not allocate unique ACL lease rewrite temp".into(),
    ))
}

fn interrupt_rewrite(
    root: &TrustedRoot,
    temp_path: &Path,
    error: SandboxError,
    clean_temp: bool,
) -> Result<()> {
    if !clean_temp {
        return Err(error);
    }
    match remove_temp_if_present(root, temp_path) {
        Ok(()) => Err(error),
        Err(cleanup) => Err(exec_error(format!(
            "ACL lease rewrite failed ({error}); temp cleanup also failed ({cleanup})"
        ))),
    }
}

fn remove_temp_if_present(root: &TrustedRoot, path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(exec_error(format!(
                "inspect ACL lease rewrite temp {}: {error}",
                path.display()
            )));
        }
    }
    let file = open_existing_nofollow(root, path, GENERIC_READ | DELETE)?;
    validate_open_file(root, path, &file)?;
    delete_open_file(&file, path)?;
    drop(file);
    confirm_path_absent(path)?;
    sync_root(root)
}

fn is_rewrite_temp_name(name: &str) -> bool {
    let Some((target, suffix)) = name.split_once(".rewrite-") else {
        return false;
    };
    let Some(body) = suffix.strip_suffix(".tmp") else {
        return false;
    };
    let Some((pid, sequence)) = body.split_once('-') else {
        return false;
    };
    target.ends_with(".toml")
        && pid.len() == 8
        && sequence.len() == 16
        && pid.bytes().all(|value| value.is_ascii_hexdigit())
        && sequence.bytes().all(|value| value.is_ascii_hexdigit())
}

fn trusted_root_for(path: &Path) -> Result<TrustedRoot> {
    let parent = path
        .parent()
        .ok_or_else(|| exec_error(format!("lease has no parent: {}", path.display())))?;
    validate_leaf(path)?;
    trusted_root(parent)
}

fn trusted_root(path: &Path) -> Result<TrustedRoot> {
    let root = open_directory_nofollow(path, "AppContainer ACL lease directory")?;
    validate_local_canonical_path(&root.final_path)?;
    let mut expected_suffix = PathBuf::new();
    for component in LEASE_DIRECTORY_COMPONENTS {
        expected_suffix.push(component);
    }
    if !root.final_path.ends_with(&expected_suffix) {
        return Err(exec_error(format!(
            "unexpected AppContainer ACL lease root identity: {}",
            root.final_path.display()
        )));
    }
    Ok(root)
}

fn open_directory_nofollow(path: &Path, label: &str) -> Result<TrustedRoot> {
    let file = OpenOptions::new()
        .access_mode(GENERIC_READ)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| exec_error(format!("open {label} {}: {error}", path.display())))?;
    let metadata = file
        .metadata()
        .map_err(|error| exec_error(format!("stat {label} {}: {error}", path.display())))?;
    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(exec_error(format!(
            "{label} must be a non-reparse directory: {}",
            path.display()
        )));
    }
    let final_path = final_path(&file, path)?;
    let identity = file_identity(&file, path)?;
    Ok(TrustedRoot {
        path: path.to_path_buf(),
        final_path,
        file,
        identity,
    })
}

fn create_or_open_child_directory(parent: &TrustedRoot, component: &str) -> Result<TrustedRoot> {
    let path = parent.final_path.join(component);
    let expected = path.clone();
    let wide = widen_path(&path);
    if unsafe { CreateDirectoryW(wide.as_ptr(), ptr::null()) } == 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(exec_error(format!(
                "create AppContainer ACL lease directory {}: {error}",
                path.display()
            )));
        }
    }
    let child = open_directory_nofollow(&path, "AppContainer ACL lease directory component")?;
    if !same_windows_path(&child.final_path, &expected) {
        return Err(exec_error(format!(
            "AppContainer ACL lease directory component traverses a reparse point: expected {}, opened {}",
            expected.display(),
            child.final_path.display()
        )));
    }
    Ok(child)
}

fn create_new_nofollow(root: &TrustedRoot, path: &Path) -> Result<File> {
    validate_child(root, path)?;
    let file = OpenOptions::new()
        // `access_mode` sets the real CreateFile access (GENERIC_READ|GENERIC_WRITE),
        // but std's `get_creation_mode` validates the high-level write/append flags
        // independently of `access_mode`: a `create_new` open with neither `.write(true)`
        // nor `.append(true)` fails with `InvalidInput` ("creating or truncating a file
        // requires write or append access") before CreateFileW is ever called, so the
        // ACL-lease probe file is never created and `is_available()` returns false on
        // every Windows host. `.write(true)` satisfies that gate; `access_mode` keeps the
        // effective access exactly GENERIC_READ|GENERIC_WRITE.
        .write(true)
        .access_mode(GENERIC_READ | GENERIC_WRITE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            exec_error(format!(
                "create AppContainer ACL lease {}: {error}",
                path.display()
            ))
        })?;
    validate_open_file(root, path, &file)?;
    Ok(file)
}

fn open_existing_nofollow(root: &TrustedRoot, path: &Path, access: u32) -> Result<File> {
    validate_child(root, path)?;
    let mut options = OpenOptions::new();
    options
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path).map_err(|error| {
        exec_error(format!(
            "open AppContainer ACL lease {}: {error}",
            path.display()
        ))
    })?;
    validate_open_file(root, path, &file)?;
    Ok(file)
}

fn delete_open_file(file: &File, path: &Path) -> Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            ptr::addr_of!(disposition).cast(),
            mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(last_error(&format!(
            "SetFileInformationByHandle(AppContainer ACL lease delete {})",
            path.display()
        )));
    }
    Ok(())
}

fn confirm_path_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(exec_error(format!(
            "AppContainer ACL lease path was recreated during handle-bound deletion: {}",
            path.display()
        ))),
        Err(error) => Err(exec_error(format!(
            "verify AppContainer ACL lease deletion {}: {error}",
            path.display()
        ))),
    }
}

fn validate_open_file(root: &TrustedRoot, path: &Path, file: &File) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|error| exec_error(format!("stat ACL lease {}: {error}", path.display())))?;
    let attributes = metadata.file_attributes();
    if attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Err(exec_error(format!(
            "ACL lease must be a non-reparse regular file: {}",
            path.display()
        )));
    }
    let information = file_information(file, path)?;
    if information.nNumberOfLinks != 1 {
        return Err(exec_error(format!(
            "ACL lease must have exactly one hard link: {}",
            path.display()
        )));
    }
    let expected = root.final_path.join(
        path.file_name()
            .ok_or_else(|| exec_error(format!("lease has no filename: {}", path.display())))?,
    );
    let opened = final_path(file, path)?;
    if !same_windows_path(&expected, &opened) {
        return Err(exec_error(format!(
            "ACL lease escaped trusted root: expected {}, opened {}",
            expected.display(),
            opened.display()
        )));
    }
    let live_root = open_directory_nofollow(&root.path, "AppContainer ACL lease directory")?;
    if live_root.identity != root.identity {
        return Err(exec_error(
            "AppContainer ACL lease root identity drift".into(),
        ));
    }
    Ok(())
}

fn validate_child(root: &TrustedRoot, path: &Path) -> Result<()> {
    validate_leaf(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| exec_error(format!("lease has no parent: {}", path.display())))?;
    if !same_windows_path(parent, &root.path) {
        return Err(exec_error(format!(
            "ACL lease path is outside trusted root: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_leaf(path: &Path) -> Result<()> {
    use std::path::Component;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| exec_error(format!("invalid ACL lease filename: {}", path.display())))?;
    if matches!(name, "." | "..") || name.contains(['/', '\\']) || name.ends_with(['.', ' ']) {
        return Err(exec_error(format!(
            "invalid ACL lease filename: {}",
            path.display()
        )));
    }
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(exec_error(format!(
            "invalid ACL lease filename component: {}",
            path.display()
        )));
    }
    Ok(())
}

fn final_path(file: &File, path: &Path) -> Result<PathBuf> {
    let handle = file.as_raw_handle() as _;
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
    let needed = unsafe { GetFinalPathNameByHandleW(handle, ptr::null_mut(), 0, flags) };
    if needed == 0 {
        return Err(last_error(&format!(
            "GetFinalPathNameByHandleW sizing for {}",
            path.display()
        )));
    }
    let mut buffer = vec![0u16; needed as usize + 1];
    let written = unsafe {
        GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, flags)
    };
    if written == 0 || written as usize >= buffer.len() {
        return Err(last_error(&format!(
            "GetFinalPathNameByHandleW for {}",
            path.display()
        )));
    }
    buffer.truncate(written as usize);
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer)))
}

fn file_identity(file: &File, path: &Path) -> Result<FileIdentity> {
    let information = file_information(file, path)?;
    Ok(FileIdentity {
        volume_serial: information.dwVolumeSerialNumber,
        index_high: information.nFileIndexHigh,
        index_low: information.nFileIndexLow,
    })
}

fn file_information(file: &File, path: &Path) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0 {
        return Err(exec_error(format!(
            "GetFileInformationByHandle({}): {:#x}",
            path.display(),
            unsafe { GetLastError() }
        )));
    }
    Ok(information)
}

fn sync_root(root: &TrustedRoot) -> Result<()> {
    match root.file.sync_all() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
        Err(error) => Err(exec_error(format!(
            "fsync AppContainer ACL lease directory {}: {error}",
            root.path.display()
        ))),
    }
}

fn widen_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn require_live() {
        assert_eq!(
            std::env::var_os("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
            Some(OsStr::new("1"))
        );
    }

    fn test_lease(tag: u64, state: LeaseState) -> LeaseFile {
        let mut lease = LeaseFile::new(
            format!("WCore-storage-{:08x}-{tag:016x}", std::process::id()),
            b"storage-test-sid",
            Vec::new(),
        )
        .unwrap();
        lease.state = state;
        lease.refresh_digest();
        lease
    }

    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn atomic_rewrite_is_old_or_new_across_injected_crash_phases() {
        require_live();
        let root = lease_directory().unwrap();
        for (index, phase) in [
            RewritePhase::TempCreated,
            RewritePhase::TempSynced,
            RewritePhase::Replaced,
        ]
        .into_iter()
        .enumerate()
        {
            let old = test_lease(index as u64, LeaseState::Prepared);
            let path = root.join(format!("{}.toml", old.profile_name));
            write_new_synced_lease(&path, &old).unwrap();
            let mut new = old.clone();
            new.state = LeaseState::GrantActive;
            new.refresh_digest();

            assert!(rewrite_synced_lease_with_crash(&path, &new, phase).is_err());
            let observed = read_validated_lease(&path).unwrap();
            let expected = if phase == RewritePhase::Replaced {
                LeaseState::GrantActive
            } else {
                LeaseState::Prepared
            };
            assert_eq!(observed.state, expected, "crash phase {phase:?}");
            recover_rewrite_temps(&root).unwrap();
            assert_eq!(read_validated_lease(&path).unwrap().state, expected);
            remove_validated_lease(&path).unwrap();
        }
    }

    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn lease_root_junction_is_rejected_before_external_mutation() {
        require_live();
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local");
        let target = temp.path().join("target");
        let junction = local.join(LEASE_DIRECTORY_COMPONENTS[0]);
        fs::create_dir(&local).unwrap();
        fs::create_dir(&target).unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(output.status.success(), "create junction: {output:?}");

        let result = lease_directory_from(local);
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        fs::remove_dir(&junction).unwrap();
    }

    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn lease_symlink_is_rejected_without_following_it() {
        require_live();
        use std::os::windows::fs::symlink_file;

        let root = lease_directory().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let target = target_dir.path().join("target.toml");
        fs::write(&target, "not authority").unwrap();
        let link = root.join(format!("WCore-symlink-{:08x}.toml", std::process::id()));
        symlink_file(&target, &link).expect("native acceptance needs symlink capability");
        assert!(read_validated_lease(&link).is_err());
        fs::remove_file(link).unwrap();
    }

    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn opened_lease_cannot_be_swapped_under_validation() {
        require_live();
        let root_path = lease_directory().unwrap();
        let old = test_lease(0xf0, LeaseState::Prepared);
        let new = test_lease(0xf1, LeaseState::GrantActive);
        let path = root_path.join(format!("{}.toml", old.profile_name));
        let replacement = root_path.join(format!("{}.toml", new.profile_name));
        write_new_synced_lease(&path, &old).unwrap();
        write_new_synced_lease(&replacement, &new).unwrap();

        let root = trusted_root(&root_path).unwrap();
        let held = open_existing_nofollow(&root, &path, GENERIC_READ | DELETE).unwrap();
        let replacement_w = widen_path(&replacement);
        let path_w = widen_path(&path);
        assert_eq!(
            unsafe {
                MoveFileExW(
                    replacement_w.as_ptr(),
                    path_w.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            },
            0,
            "lease handle omits FILE_SHARE_DELETE, so replacement must fail"
        );
        delete_open_file(&held, &path).unwrap();
        drop(held);
        confirm_path_absent(&path).unwrap();
        assert_eq!(
            read_validated_lease(&replacement).unwrap().state,
            LeaseState::GrantActive
        );
        remove_validated_lease(&replacement).unwrap();
    }

    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn equivalent_windows_spellings_resolve_to_one_file_identity() {
        require_live();
        let root_path = lease_directory().unwrap();
        let lease = test_lease(0xf2, LeaseState::Prepared);
        let path = root_path.join(format!("{}.toml", lease.profile_name));
        write_new_synced_lease(&path, &lease).unwrap();

        let ordinary_root = trusted_root_for(&path).unwrap();
        let ordinary = open_existing_nofollow(&ordinary_root, &path, GENERIC_READ).unwrap();
        let expected = file_identity(&ordinary, &path).unwrap();

        let slash_path = PathBuf::from(path.to_string_lossy().replace('\\', "/"));
        let slash_root = trusted_root_for(&slash_path).unwrap();
        let slash = open_existing_nofollow(&slash_root, &slash_path, GENERIC_READ).unwrap();
        assert_eq!(file_identity(&slash, &slash_path).unwrap(), expected);

        let mut drive_spelling = path.to_string_lossy().into_owned();
        if drive_spelling.as_bytes().get(1) == Some(&b':') {
            let toggled = if drive_spelling.as_bytes()[0].is_ascii_lowercase() {
                drive_spelling[0..1].to_ascii_uppercase()
            } else {
                drive_spelling[0..1].to_ascii_lowercase()
            };
            drive_spelling.replace_range(0..1, &toggled);
            let drive_path = PathBuf::from(drive_spelling);
            let drive_root = trusted_root_for(&drive_path).unwrap();
            let drive = open_existing_nofollow(&drive_root, &drive_path, GENERIC_READ).unwrap();
            assert_eq!(file_identity(&drive, &drive_path).unwrap(), expected);
        }

        let spelling = path.to_string_lossy();
        let extended_path = PathBuf::from(format!(r"\\?\{spelling}"));
        let extended_root = trusted_root_for(&extended_path).unwrap();
        let extended =
            open_existing_nofollow(&extended_root, &extended_path, GENERIC_READ).unwrap();
        assert_eq!(file_identity(&extended, &extended_path).unwrap(), expected);

        drop((ordinary, slash, extended));
        remove_validated_lease(&path).unwrap();
    }

    #[test]
    fn hostile_windows_path_forms_fail_closed_before_open() {
        assert!(validate_local_canonical_path(Path::new(r"\\server\share\lease.toml")).is_err());
        assert!(validate_leaf(Path::new(r"C:\lease.toml.")).is_err());
        assert!(validate_leaf(Path::new(r"C:\lease.toml ")).is_err());
    }
}
