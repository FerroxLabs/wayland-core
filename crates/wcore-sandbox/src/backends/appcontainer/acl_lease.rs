//! Crash-recoverable per-execution AppContainer profile and filesystem ACL lease.
//!
//! The lease is the durable authority for every DACL mutation made on behalf of
//! one sandbox execution. Profile creation and DACL changes are serialized only
//! while this module holds its cross-process mutation mutex; the sandboxed child
//! runs after the mutex is released, so unrelated executions remain concurrent.

#[path = "acl_lease/mutation_lock.rs"]
mod mutation_lock;
#[path = "acl_lease/sha256.rs"]
mod sha256;
#[path = "acl_lease/storage.rs"]
mod storage;
#[cfg(test)]
#[path = "acl_lease/tests.rs"]
mod tests;

use self::mutation_lock::MutationLock;
use self::sha256::sha256_hex;
use self::storage::{
    lease_directory, read_validated_lease, recover_rewrite_temps, remove_validated_lease,
    rewrite_synced_lease, write_new_synced_lease,
};

use crate::error::{Result, SandboxError};
use crate::manifest::SandboxManifest;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACCESS_DENIED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
    AclSizeInformation, DACL_SECURITY_INFORMATION, DeleteAce, EqualSid, FreeSid, GetAce,
    GetAclInformation, GetLengthSid, IsValidSid, PROTECTED_DACL_SECURITY_INFORMATION,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, SYNCHRONIZE,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    WaitForSingleObject,
};

const LEASE_VERSION: u32 = 1;
const LEASE_DIRECTORY_COMPONENTS: [&str; 4] = ["Wayland", "Core", "AppContainerLeases", "v1"];
const PROFILE_PREFIX: &str = "WCore";
const MAX_PROFILE_ATTEMPTS: u64 = 64;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const SUB_CONTAINERS_AND_OBJECTS_INHERIT: u32 = 0x3;
const ACL_READ_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
const ACL_WRITE_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;

static PROFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum IntentKind {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct AclIntent {
    path: String,
    kind: IntentKind,
    mask: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LeaseState {
    Prepared,
    GrantActive,
    ProcessExited,
    AclRevoked,
    ProfileDeletionPending,
    Cleaned,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LeaseFile {
    version: u32,
    state: LeaseState,
    profile_name: String,
    sid_sha256: String,
    owner_pid: u32,
    owner_creation_time: u64,
    intents: Vec<AclIntent>,
    lease_sha256: String,
}

impl LeaseFile {
    fn new(profile_name: String, sid: &[u8], intents: Vec<AclIntent>) -> Result<Self> {
        let mut lease = Self {
            version: LEASE_VERSION,
            state: LeaseState::Prepared,
            profile_name,
            sid_sha256: sha256_hex(sid),
            owner_pid: std::process::id(),
            owner_creation_time: current_process_creation_time()?,
            intents,
            lease_sha256: String::new(),
        };
        lease.refresh_digest();
        Ok(lease)
    }

    fn refresh_digest(&mut self) {
        self.lease_sha256 = sha256_hex(self.digest_input().as_bytes());
    }

    fn digest_input(&self) -> String {
        let mut input = format!(
            "v={}\nstate={:?}\nprofile={}\nsid={}\npid={}\ncreated={}\n",
            self.version,
            self.state,
            self.profile_name,
            self.sid_sha256,
            self.owner_pid,
            self.owner_creation_time
        );
        for intent in &self.intents {
            input.push_str(&format!(
                "intent={:?}:{}:{}:{}\n",
                intent.kind,
                intent.mask,
                intent.path.len(),
                intent.path
            ));
        }
        input
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.version != LEASE_VERSION {
            return Err(exec_error(format!(
                "unknown AppContainer ACL lease version {} in {}",
                self.version,
                path.display()
            )));
        }
        validate_profile_name(&self.profile_name)?;
        if path.file_stem().and_then(OsStr::to_str) != Some(self.profile_name.as_str()) {
            return Err(exec_error(format!(
                "AppContainer ACL lease filename/profile mismatch in {}",
                path.display()
            )));
        }
        if self.owner_pid == 0 || self.owner_creation_time == 0 {
            return Err(exec_error(format!(
                "invalid AppContainer ACL lease owner identity in {}",
                path.display()
            )));
        }
        if self.sid_sha256.len() != 64
            || !self.sid_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(exec_error(format!(
                "invalid AppContainer ACL lease SID digest in {}",
                path.display()
            )));
        }
        let mut seen = BTreeSet::new();
        for intent in &self.intents {
            validate_intent(intent, path)?;
            if !seen.insert((intent.path.clone(), intent.kind)) {
                return Err(exec_error(format!(
                    "duplicate AppContainer ACL intent in {}",
                    path.display()
                )));
            }
        }
        let expected = sha256_hex(self.digest_input().as_bytes());
        if !constant_time_eq(expected.as_bytes(), self.lease_sha256.as_bytes()) {
            return Err(exec_error(format!(
                "AppContainer ACL lease digest mismatch in {}",
                path.display()
            )));
        }
        Ok(())
    }
}

/// Owns one profile/SID/lease from setup through verified cleanup.
pub(super) struct ExecutionIdentity {
    profile_name: String,
    sid: *mut core::ffi::c_void,
    lease_path: PathBuf,
    lease: LeaseFile,
    cleaned: bool,
}

impl ExecutionIdentity {
    pub(super) fn start(manifest: &SandboxManifest) -> Result<Self> {
        Self::start_with_apply(manifest, |intents, sid| unsafe {
            apply_intents(intents, sid)
        })
    }

    fn start_with_apply(
        manifest: &SandboxManifest,
        apply: impl FnOnce(&[AclIntent], *mut core::ffi::c_void) -> Result<()>,
    ) -> Result<Self> {
        let intents = canonical_intents(manifest)?;
        let lease_dir = lease_directory()?;
        let _lock = MutationLock::acquire()?;
        unsafe { recover_dead_leases_locked(&lease_dir)? };

        let start = PROFILE_COUNTER.fetch_add(MAX_PROFILE_ATTEMPTS, Ordering::Relaxed);
        let (profile_name, sid) = unsafe { allocate_unique_profile(start)? };
        let sid_bytes = unsafe { sid_bytes(sid)? };
        let lease = LeaseFile::new(profile_name.clone(), &sid_bytes, intents)?;
        let lease_path = lease_dir.join(format!("{profile_name}.toml"));

        if let Err(error) = write_new_synced_lease(&lease_path, &lease) {
            unsafe {
                let _ = DeleteAppContainerProfile(widen(&profile_name).as_ptr());
                FreeSid(sid as _);
            }
            return Err(error);
        }

        if let Err(setup_error) = apply(&lease.intents, sid) {
            let cleanup = unsafe { cleanup_locked(&lease_path, &lease, sid) };
            unsafe { FreeSid(sid as _) };
            return match cleanup {
                Ok(()) => Err(setup_error),
                Err(cleanup_error) => Err(exec_error(format!(
                    "AppContainer ACL setup failed ({setup_error}); cleanup also failed ({cleanup_error})"
                ))),
            };
        }

        let mut active = lease.clone();
        active.state = LeaseState::GrantActive;
        active.refresh_digest();
        if let Err(error) = rewrite_synced_lease(&lease_path, &active) {
            let cleanup = unsafe { cleanup_locked(&lease_path, &lease, sid) };
            unsafe { FreeSid(sid as _) };
            return match cleanup {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(exec_error(format!(
                    "could not activate AppContainer ACL lease ({error}); cleanup also failed ({cleanup_error})"
                ))),
            };
        }

        Ok(Self {
            profile_name,
            sid,
            lease_path,
            lease: active,
            cleaned: false,
        })
    }

    pub(super) fn sid(&self) -> *mut core::ffi::c_void {
        self.sid
    }

    pub(super) fn package_root(&self) -> Option<PathBuf> {
        let mut path = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        path.push("Packages");
        path.push(&self.profile_name);
        path.push("AC");
        Some(path)
    }

    pub(super) fn cleanup(&mut self) -> Result<()> {
        if self.cleaned {
            return Ok(());
        }
        if self.lease.state != LeaseState::ProcessExited {
            return Err(exec_error(format!(
                "refusing AppContainer cleanup before durable process exit: {:?}",
                self.lease.state
            )));
        }
        let _lock = MutationLock::acquire()?;
        unsafe { cleanup_locked(&self.lease_path, &self.lease, self.sid)? };
        self.cleaned = true;
        Ok(())
    }

    /// Persist the whole-tree exit boundary before any owned ACL is revoked.
    /// The caller invokes this only after the Job object has been reaped or
    /// dropped, so recovery never mistakes a running child for cleanup-ready.
    pub(super) fn mark_process_exited(&mut self) -> Result<()> {
        if self.cleaned || self.lease.state == LeaseState::ProcessExited {
            return Ok(());
        }
        if self.lease.state != LeaseState::GrantActive {
            return Err(exec_error(format!(
                "cannot mark AppContainer process exited from lease state {:?}",
                self.lease.state
            )));
        }
        let _lock = MutationLock::acquire()?;
        let mut exited = self.lease.clone();
        exited.state = LeaseState::ProcessExited;
        exited.refresh_digest();
        rewrite_synced_lease(&self.lease_path, &exited)?;
        self.lease = exited;
        Ok(())
    }
}

impl Drop for ExecutionIdentity {
    fn drop(&mut self) {
        let cleanup = (!self.cleaned).then(|| self.cleanup());
        if let Some(Err(error)) = cleanup {
            tracing::error!(
                target: "wcore_sandbox",
                profile = %self.profile_name,
                error = %error,
                "AppContainer identity cleanup failed; durable lease retained for recovery"
            );
        }
        unsafe {
            if !self.sid.is_null() {
                FreeSid(self.sid as _);
                self.sid = ptr::null_mut();
            }
        }
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                CloseHandle(self.0);
            }
        }
    }
}

unsafe fn allocate_unique_profile(start: u64) -> Result<(String, *mut core::ffi::c_void)> {
    let creation = current_process_creation_time()?;
    for offset in 0..MAX_PROFILE_ATTEMPTS {
        let profile_name = profile_name(start + offset, creation);
        let name = widen(&profile_name);
        let display = widen("Wayland-Core Sandbox");
        let description = widen("Per-execution sandbox identity for Wayland-Core");
        let mut sid: *mut core::ffi::c_void = ptr::null_mut();
        let hr = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                display.as_ptr(),
                description.as_ptr(),
                ptr::null(),
                0,
                &mut sid as *mut _ as _,
            )
        };
        if hr == 0 && !sid.is_null() {
            return Ok((profile_name, sid));
        }
        if !sid.is_null() {
            unsafe { FreeSid(sid as _) };
        }
        if hr != hresult_from_win32(ERROR_ALREADY_EXISTS) {
            return Err(exec_error(format!(
                "CreateAppContainerProfile({profile_name}) failed: {hr:#x}"
            )));
        }
        // Existing identities are never reused: advance to a fresh name/SID.
    }
    Err(exec_error(format!(
        "could not allocate a unique AppContainer profile after {MAX_PROFILE_ATTEMPTS} collisions"
    )))
}

fn profile_name(sequence: u64, creation: u64) -> String {
    let value = format!(
        "{PROFILE_PREFIX}-{:08x}-{:016x}-{sequence:016x}",
        std::process::id(),
        creation
    );
    debug_assert!(value.len() <= 64);
    value
}

fn validate_profile_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b' '))
        || !name.starts_with(&format!("{PROFILE_PREFIX}-"))
    {
        return Err(exec_error(format!(
            "invalid AppContainer profile name {name:?}"
        )));
    }
    Ok(())
}

fn canonical_intents(manifest: &SandboxManifest) -> Result<Vec<AclIntent>> {
    let mut intents: BTreeMap<(String, IntentKind), u32> = BTreeMap::new();
    for (paths, kind, mask) in [
        (&manifest.fs_read_allow, IntentKind::Allow, ACL_READ_MASK),
        (&manifest.fs_write_allow, IntentKind::Allow, ACL_WRITE_MASK),
        (&manifest.fs_read_deny, IntentKind::Deny, ACL_READ_MASK),
    ] {
        for path in paths {
            if !path.exists() {
                tracing::debug!(
                    target: "wcore_sandbox",
                    path = %path.display(),
                    "skipping AppContainer ACL intent for non-existent path"
                );
                continue;
            }
            let canonical = fs::canonicalize(path).map_err(|error| {
                exec_error(format!(
                    "canonicalize AppContainer ACL path {}: {error}",
                    path.display()
                ))
            })?;
            validate_local_canonical_path(&canonical)?;
            let canonical = canonical.to_str().ok_or_else(|| {
                exec_error(format!(
                    "AppContainer ACL path is not valid Unicode: {}",
                    canonical.display()
                ))
            })?;
            intents
                .entry((canonical.to_owned(), kind))
                .and_modify(|existing| *existing |= mask)
                .or_insert(mask);
        }
    }
    Ok(intents
        .into_iter()
        .map(|((path, kind), mask)| AclIntent { path, kind, mask })
        .collect())
}

fn validate_intent(intent: &AclIntent, lease_path: &Path) -> Result<()> {
    match intent.kind {
        IntentKind::Allow if matches!(intent.mask, ACL_READ_MASK | ACL_WRITE_MASK) => {}
        IntentKind::Deny if intent.mask == ACL_READ_MASK => {}
        _ => {
            return Err(exec_error(format!(
                "unknown AppContainer ACL intent mask/mode in {}",
                lease_path.display()
            )));
        }
    }
    let path = Path::new(&intent.path);
    validate_local_canonical_path(path)?;
    if path.exists() {
        let recanonicalized = fs::canonicalize(path).map_err(|error| {
            exec_error(format!(
                "re-canonicalize AppContainer ACL path {}: {error}",
                path.display()
            ))
        })?;
        if !same_windows_path(path, &recanonicalized) {
            return Err(exec_error(format!(
                "AppContainer ACL path canonical identity drift in {}",
                lease_path.display()
            )));
        }
    }
    Ok(())
}

fn validate_local_canonical_path(path: &Path) -> Result<()> {
    use std::path::{Component, Prefix};
    if !path.is_absolute() {
        return Err(exec_error(format!(
            "AppContainer ACL path must be absolute: {}",
            path.display()
        )));
    }
    let local = matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    );
    if !local {
        return Err(exec_error(format!(
            "AppContainer ACL path must be local (no UNC/device): {}",
            path.display()
        )));
    }
    Ok(())
}

fn same_windows_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

unsafe fn apply_intents(intents: &[AclIntent], sid: *mut core::ffi::c_void) -> Result<()> {
    let mut applied: Vec<&AclIntent> = Vec::new();
    for intent in intents {
        let path = Path::new(&intent.path);
        if !path.exists() {
            continue;
        }
        let outcome = match intent.kind {
            IntentKind::Allow => {
                let access = unsafe { explicit_access_for_sid(sid, intent.mask, GRANT_ACCESS) };
                unsafe { apply_explicit_access(path, &access) }
            }
            // AppContainer ignores a DENY ace against its own package SID, so a
            // deny is enforced by REMOVING every package-SID ALLOW and
            // protecting the DACL — never by adding an (inert) DENY ace.
            IntentKind::Deny => unsafe { apply_protected_deny(path) },
        };
        if let Err(error) = outcome {
            unsafe { revoke_intents(&applied, sid)? };
            return Err(error);
        }
        applied.push(intent);
    }
    Ok(())
}

unsafe fn cleanup_locked(
    lease_path: &Path,
    lease: &LeaseFile,
    sid: *mut core::ffi::c_void,
) -> Result<()> {
    let intents: Vec<&AclIntent> = lease.intents.iter().collect();
    unsafe { revoke_intents(&intents, sid)? };

    let mut cleanup = lease.clone();
    cleanup.state = LeaseState::AclRevoked;
    cleanup.refresh_digest();
    rewrite_synced_lease(lease_path, &cleanup)?;

    cleanup.state = LeaseState::ProfileDeletionPending;
    cleanup.refresh_digest();
    rewrite_synced_lease(lease_path, &cleanup)?;

    let profile = widen(&lease.profile_name);
    let delete_hr = unsafe { DeleteAppContainerProfile(profile.as_ptr()) };
    if !profile_delete_succeeded(delete_hr) {
        return Err(exec_error(format!(
            "DeleteAppContainerProfile({}) failed: {delete_hr:#x}",
            lease.profile_name
        )));
    }

    cleanup.state = LeaseState::Cleaned;
    cleanup.refresh_digest();
    rewrite_synced_lease(lease_path, &cleanup)?;
    remove_validated_lease(lease_path)?;
    Ok(())
}

fn profile_delete_succeeded(hr: i32) -> bool {
    const HRESULT_FILE_NOT_FOUND: i32 = 0x8007_0002u32 as i32;
    const HRESULT_NOT_FOUND: i32 = 0x8007_0490u32 as i32;
    matches!(hr, 0 | HRESULT_FILE_NOT_FOUND | HRESULT_NOT_FOUND)
}

unsafe fn recover_dead_leases_locked(lease_dir: &Path) -> Result<()> {
    recover_rewrite_temps(lease_dir)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(lease_dir).map_err(|error| {
        exec_error(format!(
            "read AppContainer ACL lease directory {}: {error}",
            lease_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| exec_error(format!("read ACL lease entry: {error}")))?;
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|error| exec_error(format!("stat ACL lease {}: {error}", path.display())))?
            .is_file()
            || path.extension().and_then(OsStr::to_str) != Some("toml")
        {
            return Err(exec_error(format!(
                "unknown entry in AppContainer ACL lease directory: {}",
                path.display()
            )));
        }
        paths.push(path);
    }
    paths.sort();

    for path in paths {
        let lease = read_validated_lease(&path)?;
        if owner_is_live(&lease)? {
            continue;
        }
        if matches!(
            lease.state,
            LeaseState::AclRevoked | LeaseState::ProfileDeletionPending | LeaseState::Cleaned
        ) {
            let profile = widen(&lease.profile_name);
            let hr = unsafe { DeleteAppContainerProfile(profile.as_ptr()) };
            if !profile_delete_succeeded(hr) {
                return Err(exec_error(format!(
                    "recover DeleteAppContainerProfile({}) failed: {hr:#x}",
                    lease.profile_name
                )));
            }
            remove_validated_lease(&path)?;
            continue;
        }

        let profile = widen(&lease.profile_name);
        let mut derived_sid: *mut core::ffi::c_void = ptr::null_mut();
        let derive_hr = unsafe {
            DeriveAppContainerSidFromAppContainerName(
                profile.as_ptr(),
                &mut derived_sid as *mut _ as _,
            )
        };
        if derive_hr != 0 || derived_sid.is_null() {
            return Err(exec_error(format!(
                "unreconciled AppContainer ACL lease {}: profile SID cannot be derived (hr={derive_hr:#x})",
                path.display()
            )));
        }
        let sid_guard = SidFreeGuard(derived_sid);
        let bytes = unsafe { sid_bytes(sid_guard.0)? };
        if !constant_time_eq(sha256_hex(&bytes).as_bytes(), lease.sid_sha256.as_bytes()) {
            return Err(exec_error(format!(
                "AppContainer ACL lease SID/profile mismatch in {}",
                path.display()
            )));
        }
        unsafe { cleanup_locked(&path, &lease, sid_guard.0)? };
    }
    Ok(())
}

fn owner_is_live(lease: &LeaseFile) -> Result<bool> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE,
            0,
            lease.owner_pid,
        )
    };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        if error == ERROR_INVALID_PARAMETER {
            return Ok(false);
        }
        return Err(exec_error(format!(
            "cannot determine AppContainer ACL lease owner {} liveness: {error:#x}",
            lease.owner_pid
        )));
    }
    let handle = OwnedHandle(handle);
    let creation = unsafe { process_creation_time(handle.0)? };
    if creation != lease.owner_creation_time {
        return Ok(false);
    }
    match unsafe { WaitForSingleObject(handle.0, 0) } {
        WAIT_TIMEOUT => Ok(true),
        WAIT_OBJECT_0 => Ok(false),
        _ => Err(last_error("WaitForSingleObject(ACL lease owner)")),
    }
}

/// Symmetric revoke for both intent kinds. Grants are removed first (their
/// exact-SID ALLOW aces), then deny targets are un-protected — so a
/// now-ungranted parent is not momentarily re-inherited onto a deny child
/// before its protection is cleared. Denial was enforced by package-ALLOW
/// removal + `PROTECTED_DACL_SECURITY_INFORMATION`, so revoke restores
/// inheritance and leaves no residual protection or grant on the host,
/// preserving the Phase-20 no-residual invariant.
unsafe fn revoke_intents(intents: &[&AclIntent], sid: *mut core::ffi::c_void) -> Result<()> {
    let paths: Vec<&Path> = intents
        .iter()
        .map(|intent| Path::new(&intent.path))
        .collect();
    unsafe { remove_and_verify_exact_sid(&paths, sid)? };
    for intent in intents {
        if intent.kind == IntentKind::Deny {
            let path = Path::new(&intent.path);
            if path.exists() {
                unsafe { restore_unprotected_dacl(path)? };
            }
        }
    }
    Ok(())
}

unsafe fn remove_and_verify_exact_sid(paths: &[&Path], sid: *mut core::ffi::c_void) -> Result<()> {
    let unique: BTreeSet<_> = paths.iter().copied().collect();
    for path in unique {
        if !path.exists() {
            continue;
        }
        unsafe { remove_exact_sid_aces(path, sid)? };
        if unsafe { contains_exact_sid_ace(path, sid)? } {
            return Err(exec_error(format!(
                "AppContainer ACE cleanup verification failed for {}",
                path.display()
            )));
        }
    }
    Ok(())
}

unsafe fn remove_exact_sid_aces(path: &Path, sid: *mut core::ffi::c_void) -> Result<()> {
    let (mut path_w, dacl, _sd_guard) = unsafe { read_dacl(path)? };
    if dacl.is_null() {
        return Ok(());
    }
    let count = unsafe { ace_count(dacl)? };
    let mut changed = false;
    for index in (0..count).rev() {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(last_error("GetAce(AppContainer cleanup)"));
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if !matches!(
            header.AceType,
            ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE
        ) {
            continue;
        }
        let ace_sid: *mut core::ffi::c_void = if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
            unsafe { &mut (*(ace as *mut ACCESS_ALLOWED_ACE)).SidStart as *mut u32 as _ }
        } else {
            unsafe { &mut (*(ace as *mut ACCESS_DENIED_ACE)).SidStart as *mut u32 as _ }
        };
        if unsafe { IsValidSid(ace_sid) } == 0 {
            return Err(exec_error(format!(
                "invalid SID in DACL for {}",
                path.display()
            )));
        }
        if unsafe { EqualSid(ace_sid, sid) } != 0 {
            if unsafe { DeleteAce(dacl, index) } == 0 {
                return Err(last_error("DeleteAce(AppContainer exact SID)"));
            }
            changed = true;
        }
    }
    if changed {
        let rc = unsafe {
            SetNamedSecurityInfoW(
                path_w.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(exec_error(format!(
                "SetNamedSecurityInfoW exact SID cleanup for {}: {rc:#x}",
                path.display()
            )));
        }
    }
    Ok(())
}

unsafe fn contains_exact_sid_ace(path: &Path, sid: *mut core::ffi::c_void) -> Result<bool> {
    let (_path_w, dacl, _sd_guard) = unsafe { read_dacl(path)? };
    if dacl.is_null() {
        return Ok(false);
    }
    for index in 0..unsafe { ace_count(dacl)? } {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(last_error("GetAce(AppContainer verification)"));
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if !matches!(
            header.AceType,
            ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE
        ) {
            continue;
        }
        let ace_sid: *mut core::ffi::c_void = if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
            unsafe { &(*(ace as *const ACCESS_ALLOWED_ACE)).SidStart as *const u32 as _ }
        } else {
            unsafe { &(*(ace as *const ACCESS_DENIED_ACE)).SidStart as *const u32 as _ }
        };
        if unsafe { IsValidSid(ace_sid as _) } == 0 {
            return Err(exec_error(format!(
                "invalid SID in DACL for {}",
                path.display()
            )));
        }
        if unsafe { EqualSid(ace_sid as _, sid) } != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

unsafe fn ace_count(dacl: *mut ACL) -> Result<u32> {
    let mut info: ACL_SIZE_INFORMATION = unsafe { mem::zeroed() };
    if unsafe {
        GetAclInformation(
            dacl,
            &mut info as *mut _ as _,
            mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(last_error("GetAclInformation(AppContainer DACL)"));
    }
    Ok(info.AceCount)
}

unsafe fn read_dacl(path: &Path) -> Result<(Vec<u16>, *mut ACL, LocalFreeGuard)> {
    let path_w = widen_os(path.as_os_str());
    let mut dacl = ptr::null_mut();
    let mut security_descriptor = ptr::null_mut();
    let rc = unsafe {
        GetNamedSecurityInfoW(
            path_w.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut security_descriptor,
        )
    };
    if rc != 0 {
        return Err(exec_error(format!(
            "GetNamedSecurityInfoW for {}: {rc:#x}",
            path.display()
        )));
    }
    Ok((path_w, dacl, LocalFreeGuard(security_descriptor)))
}

unsafe fn explicit_access_for_sid(
    sid: *mut core::ffi::c_void,
    mask: u32,
    mode: i32,
) -> EXPLICIT_ACCESS_W {
    let mut access: EXPLICIT_ACCESS_W = unsafe { mem::zeroed() };
    access.grfAccessPermissions = mask;
    access.grfAccessMode = mode;
    access.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
    access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    access.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN;
    access.Trustee.ptstrName = sid as _;
    access
}

unsafe fn apply_explicit_access(path: &Path, access: &EXPLICIT_ACCESS_W) -> Result<()> {
    let (mut path_w, old_dacl, _sd_guard) = unsafe { read_dacl(path)? };
    let mut new_dacl = ptr::null_mut();
    let rc = unsafe { SetEntriesInAclW(1, access, old_dacl, &mut new_dacl) };
    if rc != 0 {
        return Err(exec_error(format!(
            "SetEntriesInAclW for {}: {rc:#x}",
            path.display()
        )));
    }
    let _new_dacl_guard = LocalFreeGuard(new_dacl as _);
    let rc = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            new_dacl,
            ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(exec_error(format!(
            "SetNamedSecurityInfoW for {}: {rc:#x}",
            path.display()
        )));
    }
    Ok(())
}

/// Enforce an `fs_read_deny` intent the only way Windows AppContainer honors.
///
/// The lowbox access check ignores a DENY ace against the container's OWN
/// package SID (hardware-proven at 20-53: a canonically ordered DENY→ALLOW
/// DACL was read straight through, secret disclosed, exit 0), so a package
/// DENY ace is INERT. Instead we strip every AppContainer-package
/// (`S-1-15-2-…`) ALLOW ace from the target — both explicit and the one
/// inherited from a granted parent — and set
/// `PROTECTED_DACL_SECURITY_INFORMATION` so no inheritable package ALLOW can
/// re-apply. AppContainer ignores normal SIDs when granting, so the child is
/// denied by ABSENCE of a package grant (hardware-proven: exit 1, "Access is
/// denied."). Denial never comes from re-enabling a deny-only SID — that path
/// caused the "sandbox can read no file" regression an earlier native fix
/// closed. A denied FILE and a denied DIRECTORY are each protected per-object.
unsafe fn apply_protected_deny(path: &Path) -> Result<()> {
    let (mut path_w, dacl, _sd_guard) = unsafe { read_dacl(path)? };
    if dacl.is_null() {
        // A NULL DACL grants everyone (including the package) full access and
        // cannot be protected into a denial. Fail closed rather than leave the
        // deny silently ineffective.
        return Err(exec_error(format!(
            "cannot enforce AppContainer deny on NULL-DACL target {}",
            path.display()
        )));
    }
    let count = unsafe { ace_count(dacl)? };
    for index in (0..count).rev() {
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(last_error("GetAce(AppContainer deny strip)"));
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            continue;
        }
        let ace_sid: *const core::ffi::c_void =
            unsafe { &(*(ace as *const ACCESS_ALLOWED_ACE)).SidStart as *const u32 as _ };
        if unsafe { IsValidSid(ace_sid as _) } == 0 {
            return Err(exec_error(format!(
                "invalid SID in DACL for {}",
                path.display()
            )));
        }
        if unsafe { is_app_package_sid(ace_sid) } && unsafe { DeleteAce(dacl, index) } == 0 {
            return Err(last_error("DeleteAce(AppContainer package ALLOW)"));
        }
    }
    // Always protect, even when no explicit package ALLOW was present: the
    // protection is what severs an inheritable package ALLOW on the granted
    // parent from re-applying to this target.
    let rc = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(exec_error(format!(
            "SetNamedSecurityInfoW protected deny for {}: {rc:#x}",
            path.display()
        )));
    }
    Ok(())
}

/// Symmetric revoke of [`apply_protected_deny`]: clear
/// `PROTECTED_DACL_SECURITY_INFORMATION` so the target is governed by
/// inheritance again. The current (normal-SID) DACL is written back with
/// `UNPROTECTED_DACL_SECURITY_INFORMATION`; Windows drops the
/// inheritance-flagged entries and re-propagates from the parent. Because the
/// enclosing grant is removed earlier in the same revoke pass, the target ends
/// with no package grant and no residual protection. A denied DIRECTORY is
/// un-protected per-object, matching the per-object protection in apply.
unsafe fn restore_unprotected_dacl(path: &Path) -> Result<()> {
    let (mut path_w, dacl, _sd_guard) = unsafe { read_dacl(path)? };
    if dacl.is_null() {
        // Nothing was protected (a protected target always carries a non-null
        // DACL); never write a NULL DACL, which would grant everyone access.
        return Ok(());
    }
    let rc = unsafe {
        SetNamedSecurityInfoW(
            path_w.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    if rc != 0 {
        return Err(exec_error(format!(
            "SetNamedSecurityInfoW unprotect deny for {}: {rc:#x}",
            path.display()
        )));
    }
    Ok(())
}

/// True when `sid` is an AppContainer package SID (`S-1-15-2-…`): identifier
/// authority 15 (`SECURITY_APP_PACKAGE_AUTHORITY`) with first sub-authority 2
/// (`SECURITY_APP_PACKAGE_BASE_RID`). The raw SID layout is read directly
/// because windows-sys 0.59 does not expose the `GetSidSubAuthority`
/// accessors; `IsValidSid` has already bounded the readable length.
unsafe fn is_app_package_sid(sid: *const core::ffi::c_void) -> bool {
    if sid.is_null() || unsafe { IsValidSid(sid as _) } == 0 {
        return false;
    }
    // SID layout: [Revision:1][SubAuthorityCount:1][IdentifierAuthority:6 BE]
    //             [SubAuthority0:4 LE] … A valid SID is at least 8 bytes; with
    // SubAuthorityCount >= 1 the first sub-authority (12 bytes total) is
    // guaranteed present and readable.
    let header = unsafe { std::slice::from_raw_parts(sid as *const u8, 8) };
    if header[1] == 0 {
        return false;
    }
    if header[2..8] != [0, 0, 0, 0, 0, 15] {
        return false;
    }
    let full = unsafe { std::slice::from_raw_parts(sid as *const u8, 12) };
    u32::from_le_bytes([full[8], full[9], full[10], full[11]]) == 2
}

struct LocalFreeGuard(*mut core::ffi::c_void);

impl Drop for LocalFreeGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                windows_sys::Win32::Foundation::LocalFree(self.0 as _);
            }
        }
    }
}

struct SidFreeGuard(*mut core::ffi::c_void);

impl Drop for SidFreeGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                FreeSid(self.0 as _);
            }
        }
    }
}

unsafe fn sid_bytes(sid: *mut core::ffi::c_void) -> Result<Vec<u8>> {
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(exec_error("invalid AppContainer SID".into()));
    }
    let length = unsafe { GetLengthSid(sid) } as usize;
    if length == 0 || length > 68 {
        return Err(exec_error(format!(
            "invalid AppContainer SID length {length}"
        )));
    }
    Ok(unsafe { std::slice::from_raw_parts(sid as *const u8, length) }.to_vec())
}

fn current_process_creation_time() -> Result<u64> {
    unsafe { process_creation_time(GetCurrentProcess()) }
}

unsafe fn process_creation_time(process: HANDLE) -> Result<u64> {
    let mut creation: FILETIME = unsafe { mem::zeroed() };
    let mut exit: FILETIME = unsafe { mem::zeroed() };
    let mut kernel: FILETIME = unsafe { mem::zeroed() };
    let mut user: FILETIME = unsafe { mem::zeroed() };
    if unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return Err(last_error("GetProcessTimes(AppContainer lease owner)"));
    }
    Ok(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
}

fn widen(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn widen_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

const fn hresult_from_win32(code: u32) -> i32 {
    ((code & 0xffff) | 0x8007_0000) as i32
}

fn last_error(operation: &str) -> SandboxError {
    exec_error(format!("{operation}: {:#x}", unsafe { GetLastError() }))
}

fn exec_error(message: String) -> SandboxError {
    SandboxError::ExecFailed(message)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

// Dependency-free SHA-256 keeps this Windows-only authority inside the
// existing crate boundary (the packet may not alter Cargo.toml/Cargo.lock).
