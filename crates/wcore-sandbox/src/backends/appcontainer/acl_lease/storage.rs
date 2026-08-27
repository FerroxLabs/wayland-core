use super::*;
use crate::backends::appcontainer::acl_lock_policy as policy;
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
    directory_under(lease_root()?, &LEASE_DIRECTORY_COMPONENTS)
}

/// Where [`MutationLock`] publishes the sidecar naming its holder.
///
/// A sibling of the lease directory, and that placement is the whole point:
/// see [`LOCK_HOLDER_DIRECTORY_COMPONENTS`] for what putting it INSIDE the
/// lease directory costs. The two roots share [`lease_root`], so the same test
/// chokepoint covers both and no test can publish a sidecar into a developer's
/// real profile.
pub(super) fn lock_holder_directory() -> Result<PathBuf> {
    directory_under(lease_root()?, &LOCK_HOLDER_DIRECTORY_COMPONENTS)
}

/// The holder sidecar as [`MutationLock`] owns it: where it goes, and the three
/// things that are done to it.
///
/// A TYPE and not a `PathBuf`, because wave 1 of `#945` shipped
/// `MutationLock::acquire` publishing into `lease_directory()` — swept by
/// `recover_dead_leases_locked` two lines later — and that single call failed
/// EVERY sandboxed command on Windows under
/// `WAYLAND_SANDBOX=appcontainer|strict`. Guarding the two PATHNAMES did not
/// guard that: the acquire site still held a bare `PathBuf` and a one-token
/// edit put the wrong one in it with no test signal.
///
/// Two things close that here, and both are needed:
///
/// 1. **The acquire site has no `PathBuf` to swap.** `MutationLock` holds this
///    type, whose field is private to this module, so handing it
///    `lease_directory().ok()` is a compile error rather than a regression.
/// 2. **The one surviving place to choose wrong is [`Self::resolve`]**, and
///    that function is EXECUTED, against a real lease root, by
///    `the_published_holder_survives_the_sweep_that_follows_every_acquisition`.
///
/// Every operation is best effort by construction: the sidecar is diagnostics,
/// and a lock whose holder could not be reported is still held. An unresolvable
/// directory costs the holder's NAME, never the lock.
pub(super) struct HolderSidecar {
    directory: Option<PathBuf>,
}

impl HolderSidecar {
    /// Resolve where this process publishes itself as the lock's holder.
    ///
    /// Deliberately NOT [`lease_directory`]: see [`HolderSidecar`] and
    /// [`LOCK_HOLDER_DIRECTORY_COMPONENTS`]. This is the whole of the choice,
    /// which is why it is one line in a named function rather than an
    /// expression inside `MutationLock::acquire` where nothing could reach it.
    pub(super) fn resolve() -> Self {
        Self {
            directory: lock_holder_directory().ok(),
        }
    }

    /// Where the sidecar is published, for tests and diagnostics only.
    pub(super) fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    pub(super) fn publish(&self, pid: u32, exe: &str) {
        if let Some(directory) = self.directory() {
            policy::publish_holder(directory, pid, exe);
        }
    }

    /// Read the current holder, treating this process as no holder at all.
    pub(super) fn sample(&self, self_pid: u32) -> Option<policy::LockHolder> {
        policy::read_holder(self.directory()?, self_pid)
    }

    pub(super) fn clear(&self) {
        if let Some(directory) = self.directory() {
            policy::clear_holder(directory);
        }
    }
}

/// Production lease root: the user's real `%LOCALAPPDATA%`.
#[cfg(not(test))]
fn lease_root() -> Result<PathBuf> {
    Ok(PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(
        || exec_error("LOCALAPPDATA is required for AppContainer ACL leases".into()),
    )?))
}

/// Unit-test lease root: one temp directory per test process.
///
/// This is the single chokepoint that makes it STRUCTURALLY impossible for a
/// unit test to write a lease into the user's real lease directory. It is
/// deliberately here rather than at the call sites: there are five call sites
/// in this crate's tests, and the sixth one somebody adds is the one that
/// forgets.
///
/// The stakes are not test hygiene. A lease written by a test carries a
/// synthetic `WCore-storage-…` profile name for which no AppContainer profile
/// is ever created, so `recover_dead_leases_locked` can never derive a matching
/// SID. Two such files were found disabling the Windows sandbox on a real
/// developer box. See `.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md`.
///
/// That used to be PERMANENT: the mismatch returned `Err`, there was no
/// quarantine path, and the negative probe cache is in-process only, so every
/// later process re-read the same file and failed again until a human deleted
/// it (`F-28-02-002`). [`quarantine_lease`] now reclaims such a lease once its
/// owning process is provably gone, so the wedge self-clears — but this test
/// root remains the primary defence, because reclamation is a repair and not a
/// licence for tests to write into a developer's real lease directory.
///
/// One correction to the record this comment previously carried: the product
/// does NOT "carry on running UNSANDBOXED" when the probe reports false. That
/// was measured on `seandesktop` 2026-07-27 and disproved — the delegated
/// dispatcher fails CLOSED (`ran=False` in both wedged observations). The
/// defect was denial of service, not silent loss of containment.
///
/// Integration tests under `tests/` compile the library WITHOUT `cfg(test)` and
/// so still use the real directory. That is correct and intended: they drive
/// `ExecutionIdentity::start`, whose leases carry a real profile and a real SID
/// and therefore reconcile normally. They cannot reach the synthetic-lease
/// helpers at all, because those are private to this module tree — visibility,
/// not discipline, is what keeps them out.
#[cfg(test)]
fn lease_root() -> Result<PathBuf> {
    test_lease_root()
}

/// Environment variable carrying the lease root to a spawned helper process.
///
/// `killed_owner_is_recovered_before_next_execution` spawns the test binary
/// again and then looks for the lease that child deliberately abandoned. A root
/// keyed only on the process id would put the child's lease somewhere the
/// parent never looks, so the root travels to the child through the
/// environment. Test-only.
#[cfg(test)]
pub(super) const TEST_LEASE_ROOT_ENV: &str = "WCORE_TEST_LEASE_ROOT";

#[cfg(test)]
pub(super) fn test_lease_root() -> Result<PathBuf> {
    use std::sync::OnceLock;
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    let root = ROOT.get_or_init(|| match std::env::var_os(TEST_LEASE_ROOT_ENV) {
        // Inherited from a parent test process: join it as-is, and never clear
        // it — the lease this process is about to abandon is the whole point.
        Some(inherited) => PathBuf::from(inherited),
        None => {
            let path =
                std::env::temp_dir().join(format!("wcore-lease-test-{:08x}", std::process::id()));
            // Start every run from an empty root. The name is keyed on the
            // process id, Windows reuses process ids freely, and a lease left
            // behind by an earlier run is not inert: `create_new` collides with
            // it, and `recover_dead_leases_locked` refuses it outright. Both
            // were observed on SEANDESKTOP when a reused id inherited a lease
            // that an earlier failing run had abandoned here.
            let _ = fs::remove_dir_all(&path);
            path
        }
    });
    fs::create_dir_all(root).map_err(|error| {
        exec_error(format!(
            "create test AppContainer ACL lease root {}: {error}",
            root.display()
        ))
    })?;
    Ok(root.clone())
}

/// A lease directory that belongs to ONE test and to nothing else.
///
/// [`test_lease_root`] is keyed per PROCESS, and the default `cargo test`
/// harness runs every test in this binary in ONE process, so that root is
/// shared mutable state between tests running concurrently on different
/// threads. `recover_dead_leases_locked` enumerates the lease directory and
/// only then opens each entry it found, so a test that creates or removes a
/// lease there makes another test's sweep fail on a file that existed at
/// enumeration and was gone at open. Measured 10 failures in 10 runs of
/// `cargo test -p wcore-sandbox --lib appcontainer_acl_lease` on Windows 11
/// build 26200 (`#1095`): the sweeps in `tests.rs` failed with `os error 2`
/// naming the lease that
/// `a_lease_written_by_a_test_never_lands_in_the_production_directory` writes
/// and then removes.
///
/// This is test isolation, NOT a repair of production behaviour, and the
/// distinction is the whole reason the fix is here rather than in the sweep:
/// every production lease write and every production sweep runs inside the
/// same per-user machine-wide `MutationLock`, so a lease can neither appear nor
/// vanish between a sweep's enumeration and its open. The unit tests that call
/// `recover_dead_leases_locked` directly deliberately do NOT take that lock —
/// it lives in the `Global\` namespace, so taking it would test the privileges
/// of whoever ran `cargo test` rather than the repair — which is exactly why
/// they get a directory of their own instead of a lock.
#[cfg(test)]
pub(super) fn private_lease_directory(local: &Path) -> Result<PathBuf> {
    private_directory(local, &LEASE_DIRECTORY_COMPONENTS)
}

/// The holder-sidecar directory belonging to ONE test's private root.
///
/// Exists so a test can assert the sidecar's placement against the lease
/// directory derived from the SAME root. Comparing a private lease directory
/// against the process-wide holder directory would compare two unrelated trees
/// and pass no matter where the sidecar goes.
#[cfg(test)]
pub(super) fn private_lock_holder_directory(local: &Path) -> Result<PathBuf> {
    private_directory(local, &LOCK_HOLDER_DIRECTORY_COMPONENTS)
}

#[cfg(test)]
fn private_directory(local: &Path, components: &[&str]) -> Result<PathBuf> {
    // This is the SECOND door into `directory_under`, so it carries the same
    // lock as the first: whatever a test passes here, it must not be the user's
    // real lease root. See [`lease_root`] for what a test lease written there
    // costs — it disabled the Windows sandbox on a real developer box until a
    // human deleted the file.
    if std::env::var_os("LOCALAPPDATA")
        .is_some_and(|real| same_windows_path(local, Path::new(&real)))
    {
        return Err(exec_error(
            "a private test lease directory must never be rooted at the real LOCALAPPDATA".into(),
        ));
    }
    directory_under(local.to_path_buf(), components)
}

fn directory_under(local: PathBuf, components: &[&str]) -> Result<PathBuf> {
    let local_root = open_directory_nofollow(&local, "LOCALAPPDATA")?;
    validate_local_canonical_path(&local_root.final_path)?;

    let mut root = local_root;
    for component in components {
        root = create_or_open_child_directory(&root, component)?;
    }
    Ok(root.path)
}

pub(super) fn write_new_synced_lease(path: &Path, lease: &LeaseFile) -> Result<()> {
    write_new_synced_lease_with_probe(path, lease, |_| ())
}

/// The create-then-write sequence, with a test probe between the two steps.
///
/// The probe exists so `F-28-ADJ-002` can be evidenced at its CAUSE rather than
/// asserted from reading the source: the file is created here and its content
/// is written on the next line, so anything that stops the process in between —
/// a crash, a power loss — leaves a 0-byte `.toml`. In production the probe is
/// a closure that does nothing and compiles away. This mirrors the crash-phase
/// hook `rewrite_with_hook` already uses for the rewrite path, so it is the
/// file's existing pattern rather than a new one.
pub(super) fn write_new_synced_lease_with_probe(
    path: &Path,
    lease: &LeaseFile,
    probe: impl FnOnce(&Path),
) -> Result<()> {
    let root = trusted_root_for(path)?;
    let serialized = serialize(lease)?;
    let mut file = create_new_nofollow(&root, path)?;
    probe(path);
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

/// True when the lease file is exactly zero bytes.
///
/// [`write_new_synced_lease`] creates the file and only then writes its
/// content, so an interrupted create leaves a 0-byte `.toml` behind
/// (`F-28-ADJ-002`). `read_validated_lease` rejects that file, and before this
/// existed the rejection propagated out of `recover_dead_leases_locked` and
/// wedged the sandbox permanently — the same denial of service `F-28-02-002`
/// closed, arriving through a different door.
///
/// Deliberately answers ONLY the zero-length question, and is not a general
/// "is this lease readable" probe. A non-empty lease that fails to parse is
/// indistinguishable from a tampered one and must keep failing closed.
pub(super) fn lease_is_zero_length(path: &Path) -> Result<bool> {
    let root = trusted_root_for(path)?;
    let file = open_existing_nofollow(&root, path, GENERIC_READ)?;
    validate_open_file(&root, path, &file)?;
    let metadata = file
        .metadata()
        .map_err(|error| exec_error(format!("stat ACL lease {}: {error}", path.display())))?;
    Ok(metadata.len() == 0)
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

/// Move a lease that can never reconcile out of the ACTIVE lease directory,
/// into the quarantine sub-directory, and prove it is gone from the active set.
///
/// The file is moved, never deleted. A lease in this state is the only record
/// of how the wedge was produced — two such files were found disabling the
/// Windows sandbox on a real developer box — and deleting them would trade a
/// silent permanent failure for a silent permanent loss of the evidence.
///
/// An already-quarantined artifact is NEVER overwritten: `MoveFileExW` is
/// called without `MOVEFILE_REPLACE_EXISTING`, and a name collision advances to
/// a fresh suffix instead of clobbering.
pub(super) fn quarantine_lease(path: &Path) -> Result<PathBuf> {
    let root = trusted_root_for(path)?;
    let quarantine = create_or_open_child_directory(&root, QUARANTINE_DIRECTORY)?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| exec_error(format!("invalid ACL lease filename: {}", path.display())))?;
    let start = TEMP_COUNTER.fetch_add(TEMP_ATTEMPTS, Ordering::Relaxed);
    for offset in 0..TEMP_ATTEMPTS {
        let destination = quarantine.final_path.join(format!(
            "{name}.quarantined-{:08x}-{:016x}",
            std::process::id(),
            start + offset
        ));
        let source_wide = widen_path(path);
        let destination_wide = widen_path(&destination);
        if unsafe {
            MoveFileExW(
                source_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            confirm_path_absent(path)?;
            sync_root(&root)?;
            sync_root(&quarantine)?;
            return Ok(destination);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(exec_error(format!(
                "quarantine AppContainer ACL lease {} -> {}: {error}",
                path.display(),
                destination.display()
            )));
        }
    }
    Err(exec_error(format!(
        "could not allocate a unique quarantine name for AppContainer ACL lease {}",
        path.display()
    )))
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
            TEST_SID_SENTINEL,
            Vec::new(),
        )
        .unwrap();
        lease.state = state;
        lease.refresh_digest();
        lease
    }

    /// Drop a `\\?\` verbatim prefix so two spellings of the same directory
    /// compare equal.
    ///
    /// Without this the comparison below is VACUOUS: `lease_directory()`
    /// returns the path via `GetFinalPathNameByHandleW`, which yields the
    /// verbatim `\\?\C:\…` form, while the production path composed from
    /// `%LOCALAPPDATA%` is the ordinary `C:\…` form. A plain string compare
    /// therefore never matched and the assertion passed against the UNFIXED
    /// tree — measured, not hypothesised, on SEANDESKTOP at 2419b868.
    fn strip_verbatim(path: &Path) -> PathBuf {
        let text = path.to_string_lossy();
        match text.strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => PathBuf::from(text.as_ref()),
        }
    }

    /// The real, user-facing lease directory: what `lease_directory()` resolves
    /// to in a production build. Computed here WITHOUT creating anything, so
    /// the check itself never brings the production tree into existence.
    fn production_lease_directory() -> Option<PathBuf> {
        let mut path = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        for component in LEASE_DIRECTORY_COMPONENTS {
            path.push(component);
        }
        Some(path)
    }

    /// A unit test must never resolve the production lease directory.
    ///
    /// Every lease this module's tests write goes to `lease_directory()`, so
    /// this one path decides whether the whole test module writes into the
    /// user's real sandbox state. It resolved to production, and the native
    /// acceptance suite left two unreconcilable leases on a real developer box
    /// that silently disabled its Windows sandbox until a human deleted them
    /// (`.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md`).
    #[test]
    fn unit_tests_never_resolve_the_production_lease_directory() {
        let resolved = lease_directory().expect("test lease directory must resolve");
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let Some(production) = production_lease_directory() else {
            return;
        };
        assert!(
            !same_windows_path(&strip_verbatim(&resolved), &strip_verbatim(&production)),
            "lease_directory() resolved to the PRODUCTION lease directory under \
             cfg(test): {}. Every lease written by this test module lands in the \
             user's real sandbox state, where a synthetic test profile can never \
             reconcile and disables the sandbox permanently.",
            resolved.display()
        );
        // The per-test door has to refuse the same destination, or the
        // chokepoint above is only one of two ways in.
        assert!(
            private_lease_directory(Path::new(&local)).is_err(),
            "private_lease_directory accepted the real LOCALAPPDATA"
        );
    }

    /// End-to-end form of the same invariant: drive the real write path with
    /// the same helper the acceptance tests use and prove nothing appeared in
    /// production. The observation is captured and the lease removed BEFORE the
    /// assertion, so even the failing (pre-fix) run leaves no residue behind —
    /// a test that proves pollution must not itself pollute.
    #[test]
    fn a_lease_written_by_a_test_never_lands_in_the_production_directory() {
        let root = lease_directory().expect("test lease directory must resolve");
        let lease = test_lease(0xdead, LeaseState::Prepared);
        let name = format!("{}.toml", lease.profile_name);
        let path = root.join(&name);

        write_new_synced_lease(&path, &lease).expect("write test lease");
        let landed_in_production = production_lease_directory()
            .map(|production| production.join(&name).exists())
            .unwrap_or(false);
        remove_validated_lease(&path).expect("remove test lease");

        assert!(
            !landed_in_production,
            "a lease written by a test appeared in the PRODUCTION lease directory \
             as {name}; it carries a synthetic profile name with no AppContainer \
             profile behind it, so recovery can never reconcile it and every \
             later sandboxed execution on this machine is refused."
        );
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

        let result = directory_under(local, &LEASE_DIRECTORY_COMPONENTS);
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

        // `lease_directory()` resolves through `GetFinalPathNameByHandleW`, so
        // `path` is ALREADY the verbatim `\\?\C:\…` spelling. Every spelling
        // below is therefore derived from the ordinary form, not from `path`:
        // prepending `\\?\` to `path` produced `\\?\\\?\C:\…` and failed with
        // os error 123, and the drive-letter leg silently never ran because
        // byte 1 of a verbatim path is `\`, not `:`. Both were measured on
        // SEANDESKTOP; the extended leg is what aborted this test before its
        // cleanup below, which is how a lease leaked into the real lease
        // directory and disabled the sandbox on that machine.
        let ordinary_path = strip_verbatim(&path);
        let ordinary_root = trusted_root_for(&ordinary_path).unwrap();
        let ordinary =
            open_existing_nofollow(&ordinary_root, &ordinary_path, GENERIC_READ).unwrap();
        let expected = file_identity(&ordinary, &ordinary_path).unwrap();

        let slash_path = PathBuf::from(ordinary_path.to_string_lossy().replace('\\', "/"));
        let slash_root = trusted_root_for(&slash_path).unwrap();
        let slash = open_existing_nofollow(&slash_root, &slash_path, GENERIC_READ).unwrap();
        assert_eq!(file_identity(&slash, &slash_path).unwrap(), expected);

        let mut drive_spelling = ordinary_path.to_string_lossy().into_owned();
        assert_eq!(
            drive_spelling.as_bytes().get(1),
            Some(&b':'),
            "the drive-letter leg must actually run: {drive_spelling}"
        );
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

        let spelling = ordinary_path.to_string_lossy();
        let extended_path = PathBuf::from(format!(r"\\?\{spelling}"));
        let extended_root = trusted_root_for(&extended_path).unwrap();
        let extended =
            open_existing_nofollow(&extended_root, &extended_path, GENERIC_READ).unwrap();
        assert_eq!(file_identity(&extended, &extended_path).unwrap(), expected);

        // `drive` belongs here too. Lease handles deliberately omit
        // FILE_SHARE_DELETE, so any still-open handle makes the DELETE open
        // inside `remove_validated_lease` fail with a sharing violation — the
        // same property `opened_lease_cannot_be_swapped_under_validation`
        // asserts. It was absent from this drop only because the drive-letter
        // leg never used to run.
        drop((ordinary, slash, drive, extended));
        remove_validated_lease(&path).unwrap();
    }

    /// The sentinel digest is frozen, because PRODUCTION reads it.
    ///
    /// `TEST_SID_SENTINEL_SHA256` is how a production build recognises a lease
    /// that a test suite leaked into a real lease directory. The two files
    /// found wedging a developer box carry exactly this digest; changing either
    /// constant would make those files unrecognisable again. This also re-proves
    /// on every run the byte-level identification that established the defect.
    #[test]
    fn test_sid_sentinel_digest_is_frozen() {
        assert_eq!(sha256_hex(TEST_SID_SENTINEL), TEST_SID_SENTINEL_SHA256);
    }

    /// A leaked test lease must be named as such, not reported as a generic
    /// mismatch. The generic text is what operators read as a platform limit.
    ///
    /// Retargeted from `unreconcilable_lease_message` onto the pair that
    /// replaced it when `F-28-02-002` was repaired
    /// (`unreconcilable_lease_reason` + `reclamation_report`). Every assertion
    /// it made is still made here; the remedy assertion additionally now pins
    /// that the message denies the three false explanations that let the wedge
    /// survive for weeks.
    #[test]
    fn a_leaked_test_lease_is_diagnosed_by_name() {
        let lease = test_lease(0xbeef, LeaseState::Prepared);
        let reason = unreconcilable_lease_reason(&lease);
        assert!(
            reason.contains("OWN TEST SUITE"),
            "test-origin lease must be named as test-origin, got: {reason}"
        );

        let message = reclamation_report(
            &lease,
            Path::new(r"C:\leases\quarantine\x.toml.quarantined-0-0"),
            &reason,
        );
        assert!(
            message.contains("DELETED") || message.contains("Delete it"),
            "the diagnosis must state the remedy, got: {message}"
        );
        assert!(
            message.contains("NOT a platform limitation")
                && message.contains("NOT an SSH or session-0 effect")
                && message.contains("NOT transient"),
            "the diagnosis must deny the explanations that hid this defect, got: {message}"
        );
        assert!(
            message.contains(r"C:\leases\quarantine\x.toml.quarantined-0-0"),
            "the diagnosis must name where the evidence went, got: {message}"
        );

        let mut genuine = lease.clone();
        genuine.sid_sha256 = sha256_hex(b"a-real-appcontainer-package-sid");
        genuine.refresh_digest();
        let other = unreconcilable_lease_reason(&genuine);
        assert!(
            !other.contains("OWN TEST SUITE"),
            "a genuine mismatch must NOT be blamed on the test suite, got: {other}"
        );
    }

    #[test]
    fn hostile_windows_path_forms_fail_closed_before_open() {
        assert!(validate_local_canonical_path(Path::new(r"\\server\share\lease.toml")).is_err());
        assert!(validate_leaf(Path::new(r"C:\lease.toml.")).is_err());
        assert!(validate_leaf(Path::new(r"C:\lease.toml ")).is_err());
    }
}
