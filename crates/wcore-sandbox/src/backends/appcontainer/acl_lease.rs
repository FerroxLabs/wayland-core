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
    HolderSidecar, lease_directory, lease_is_zero_length, quarantine_lease, read_validated_lease,
    recover_rewrite_temps, remove_validated_lease, rewrite_synced_lease, write_new_synced_lease,
};
#[cfg(test)]
use self::storage::{
    TEST_LEASE_ROOT_ENV, lock_holder_directory, private_lease_directory,
    private_lock_holder_directory, test_lease_root,
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
/// Where the mutation lock publishes its holder sidecar — a SIBLING of the
/// lease directory, deliberately NOT a file inside it.
///
/// Same reasoning, and the same shape, as
/// `windows_impl::shared_verdict::record_path`. `recover_dead_leases_locked`
/// treats every entry in the lease directory it does not recognise as a hard
/// error that aborts recovery, and it runs two lines after
/// `MutationLock::acquire` in `start_with_apply` — so a sidecar written in
/// there fails every sandboxed command, not just a contended one. Allow-listing
/// a second name in the sweep would fix this build and wedge any older build
/// that met the file, which on a machine running a Desktop app and a CLI at
/// different versions is the configuration the lock exists to serve.
const LOCK_HOLDER_DIRECTORY_COMPONENTS: [&str; 4] =
    ["Wayland", "Core", "AppContainerAclLock", "v1"];
/// Sub-directory of the lease directory holding leases that were reclaimed
/// because they can never reconcile against their own AppContainer profile.
///
/// Reclaimed leases are MOVED here rather than deleted: the file is the only
/// evidence of how the wedge was produced, and destroying it would replace one
/// invisible failure with another.
const QUARANTINE_DIRECTORY: &str = "quarantine";
const PROFILE_PREFIX: &str = "WCore";
const MAX_PROFILE_ATTEMPTS: u64 = 64;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const SUB_CONTAINERS_AND_OBJECTS_INHERIT: u32 = 0x3;
const ACL_READ_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
const ACL_WRITE_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;

static PROFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// SID bytes this crate's OWN test helpers stamp into a lease.
///
/// Production never builds a lease from these bytes: a real lease always
/// carries the bytes of a real AppContainer package SID. The value is frozen
/// (not regenerated, not prettified) so that leases already leaked onto real
/// machines by earlier runs of the acceptance suite stay recognisable.
#[cfg(test)]
pub(super) const TEST_SID_SENTINEL: &[u8] = b"storage-test-sid";

/// `sha256(TEST_SID_SENTINEL)`, kept in PRODUCTION builds on purpose.
///
/// It is the only way a running product can tell "a test suite leaked a lease
/// into my real lease directory" apart from "this lease genuinely does not
/// match its profile". Two files carrying exactly this digest were found
/// disabling the Windows sandbox on a developer box
/// (`.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md`); the paired
/// `test_sid_sentinel_digest_is_frozen` test re-proves the correspondence so
/// neither constant can drift away from those files.
const TEST_SID_SENTINEL_SHA256: &str =
    "5b22ee051799cf8aa6783a40faf32ce5bc9a7f7817bae7ab4076db3279005155";

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

        // Profile allocation runs OUTSIDE the machine-wide mutation lock, and
        // that placement is the whole of `F-RC-WIN-001`.
        //
        // `CreateAppContainerProfile` is an RPC to the AppX profile service and
        // costs ~14.5ms on a 32-core/NVMe box (measured on SEANDESKTOP:
        // alloc_us=14223..18491 over five idle rounds). Holding a `Global\`
        // mutex across it serialised that cost machine-wide, so N concurrent
        // sandboxed commands cost ~N x 14.5ms of pure queueing before any of
        // them could start. Together with the ~40ms `DeleteAppContainerProfile`
        // on the teardown path that put ~60ms of profile-service RPC under one
        // lock per command, which is what pushed the sandbox probe past its 15s
        // wall-clock guard under a saturated CI matrix and made the backend
        // refuse to run at all.
        //
        // It needs no cross-process exclusion. Names are unique per
        // (pid, process-creation-time, counter) — see `profile_name` — so two
        // processes cannot select the same name, and the ERROR_ALREADY_EXISTS
        // arm below already covers pid reuse inside one creation time. Nothing
        // here touches the lease directory or any shared DACL, which are the
        // two things the lock actually protects.
        let start = PROFILE_COUNTER.fetch_add(MAX_PROFILE_ATTEMPTS, Ordering::Relaxed);
        let (profile_name, sid) = unsafe { allocate_unique_profile(start)? };
        // Moving allocation out of the lock lengthens the window in which a
        // profile exists with no durable lease naming it, so that window is now
        // guarded rather than open-coded per return. See [`UnrecordedProfile`].
        let mut unrecorded = UnrecordedProfile::holding(profile_name.clone(), sid);

        let _lock = MutationLock::acquire()?;
        unsafe { recover_dead_leases_locked(&lease_dir)? };

        let sid_bytes = unsafe { sid_bytes(sid)? };
        let lease = LeaseFile::new(profile_name.clone(), &sid_bytes, intents)?;
        let lease_path = lease_dir.join(format!("{profile_name}.toml"));
        write_new_synced_lease(&lease_path, &lease)?;
        unrecorded.recorded();

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
        // Deliberately TWO lock acquisitions with the profile deletion between
        // them, rather than one acquisition spanning the lot.
        //
        // `DeleteAppContainerProfile` is ~40ms of profile-service RPC; holding
        // the machine-wide mutex across it made every concurrent sandboxed
        // command queue behind every other command's teardown.
        //
        // The gap is safe because the protocol was already built to be
        // interruptible at exactly this point. The lease is durably in
        // `ProfileDeletionPending` before the lock is released, so:
        //   - a concurrent recovery sweep sees a LIVE owner and skips it
        //     (`owner_is_live` is checked before any state is acted on); and
        //   - if this process dies in the gap, recovery finds a dead owner in
        //     `ProfileDeletionPending` and completes exactly these remaining
        //     steps — that arm already exists and is what the state is for.
        let pending = {
            let _lock = MutationLock::acquire()?;
            unsafe { revoke_and_mark_pending_locked(&self.lease_path, &self.lease, self.sid)? }
        };

        unsafe { delete_owned_profile(&pending.profile_name)? };

        let _lock = MutationLock::acquire()?;
        finalize_cleaned_locked(&self.lease_path, pending)?;
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

/// Deletes a freshly created AppContainer profile unless the lease that records
/// it reached disk.
///
/// The guarded window opens when `CreateAppContainerProfile` returns and closes
/// when `write_new_synced_lease` succeeds. Inside it the profile exists with NO
/// durable record, so an early return that failed to delete it would leak a
/// profile that recovery can never reclaim — recovery works from lease files,
/// and there is no lease naming this one.
///
/// The window was always present; it used to sit between two statements inside
/// the mutation lock. Moving `allocate_unique_profile` outside that lock widened
/// it by a lock acquisition, which is why it is now closed by a guard on every
/// path instead of by a cleanup block on the three paths somebody remembered.
struct UnrecordedProfile {
    name: String,
    sid: *mut core::ffi::c_void,
    recorded: bool,
}

impl UnrecordedProfile {
    fn holding(name: String, sid: *mut core::ffi::c_void) -> Self {
        Self {
            name,
            sid,
            recorded: false,
        }
    }

    /// The durable lease naming this profile is on disk; recovery owns it now.
    fn recorded(&mut self) {
        self.recorded = true;
    }
}

impl Drop for UnrecordedProfile {
    fn drop(&mut self) {
        if self.recorded {
            return;
        }
        unsafe {
            let hr = DeleteAppContainerProfile(widen(&self.name).as_ptr());
            if !profile_delete_succeeded(hr) {
                tracing::error!(
                    target: "wcore_sandbox",
                    profile = %self.name,
                    hresult = format!("{hr:#x}"),
                    "leaked an AppContainer profile that no lease records"
                );
            }
            FreeSid(self.sid as _);
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
    // Resolved ONCE. Two `canonicalize` syscalls per ALLOW intent is not
    // free at the scale this actually runs at -- the lease #369 was filed
    // about carries 4367 intents -- and per-intent latency here also shifts
    // the interleaving `#368`'s live concurrency test turns on, which would
    // make this guard a variable in a race it has nothing to do with.
    let over_broad_roots = over_broad_allow_roots();
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
            if kind == IntentKind::Allow {
                refuse_over_broad_allow_root(&canonical, &over_broad_roots)?;
            }
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

/// Refuse to write an AppContainer package ALLOW onto a root so broad that
/// the grant is effectively the whole machine or the whole user (`#369` c3).
///
/// # Found, not modelled
///
/// `#369` reports a lease on SEANDESKTOP whose FIRST intent is
/// `path = '\\?\C:\Users\seand'  kind = "allow"  mask = 1180095`. That mask
/// is [`ACL_WRITE_MASK`] to the bit, so the grant came from
/// `SandboxManifest::fs_write_allow`, which `wcore-tools`' bash policy fills
/// from `WorkspacePolicy::writable_roots()` -- i.e. the WORKSPACE root. The
/// workspace root was the user's profile directory, because wayland-core was
/// started there. Nothing in the ACL layer went wrong; it faithfully applied
/// what it was handed.
///
/// The other 4366 intents in that same lease are DENIES, one per secret found
/// beneath it -- `.aws\credentials` and `.aws\config` among them. That is the
/// shape of the problem: a single inheritable ALLOW on the profile root
/// confers the entire subtree, and what claws it back is an ENUMERATION of
/// secrets computed at one instant. `FerroxLabs/wayland-core#368` measures
/// that same deny mechanism being stripped by a concurrent identity, so the
/// enumeration is not merely incomplete in principle, it is unreliable in
/// practice.
///
/// # Why refusing, and what it costs
///
/// Refusing means an operator who runs wayland-core FROM their home directory
/// with `WAYLAND_SANDBOX` set to the AppContainer backend gets a fail-closed
/// refusal instead of a whole-profile package grant. That cost is bounded:
/// AppContainer is opt-in on Windows (`windows_candidate`), the shipping
/// default is the Job Object backend, so no default configuration reaches
/// this. A grant that wide is not a sandbox, and applying it silently is worse
/// than declining to.
///
/// The three roots below are the observed one plus the two that strictly
/// contain it. Nothing speculative is added: a project directory at any depth,
/// including `C:\src`, is still granted.
/// The roots a package ALLOW may never be written on, resolved once.
///
/// Returned as owned pairs rather than read from the environment at each
/// comparison so that the answer cannot change between two intents in the same
/// manifest, and so the syscalls happen once per manifest instead of twice per
/// intent. A variable that is unset, or a path that will not canonicalize, is
/// simply absent from the list: this guard is a bound on breadth, and it must
/// not turn a missing environment variable into a refusal to run.
fn over_broad_allow_roots() -> Vec<(PathBuf, &'static str)> {
    let mut roots = Vec::new();
    for (variable, description) in [
        ("USERPROFILE", "it is this user's entire profile directory"),
        ("PUBLIC", "it is the machine's shared public profile"),
    ] {
        let Ok(value) = std::env::var(variable) else {
            continue;
        };
        let Ok(root) = fs::canonicalize(&value) else {
            continue;
        };
        // The parent of every profile (`C:\Users`) is broader still, and is
        // added from whichever of the two resolved.
        if let Some(parent) = root.parent() {
            let parent = parent.to_path_buf();
            if !roots.iter().any(|(known, _)| *known == parent) {
                roots.push((
                    parent,
                    "it is the directory holding every user profile on this machine",
                ));
            }
        }
        roots.push((root, description));
    }
    roots
}

fn refuse_over_broad_allow_root(
    canonical: &Path,
    over_broad_roots: &[(PathBuf, &'static str)],
) -> Result<()> {
    let too_broad = |reason: &str| {
        Err(exec_error(format!(
            "refusing to grant the AppContainer package read/write on {}: {reason}. A single              inheritable ALLOW there confers the whole subtree, and the per-secret denies              that would claw it back are an enumeration taken at one instant              (FerroxLabs/wayland-core#369, and #368 for why that enumeration is not              reliable). Start wayland-core in a project directory, or use the default              Windows backend, which does not write ACLs at all.",
            canonical.display()
        )))
    };

    // A drive root: `C:\` has a prefix, a root, and no normal component.
    if !canonical
        .components()
        .any(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return too_broad("it is a drive root");
    }
    // The user's own profile directory, the shared public profile, and the
    // directory holding every profile on the machine. Compared through the
    // same canonicalization the intent went through, so a short path and its
    // verbatim form are the same answer.
    for (root, description) in over_broad_roots {
        if same_windows_path(canonical, root) {
            return too_broad(description);
        }
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
    let pending = unsafe { revoke_and_mark_pending_locked(lease_path, lease, sid)? };
    unsafe { delete_owned_profile(&pending.profile_name)? };
    finalize_cleaned_locked(lease_path, pending)
}

/// Revoke the granted ACLs and durably record that only profile deletion is
/// left. MUST run under [`MutationLock`]: it rewrites the lease file and
/// mutates DACLs that concurrent executions also read-modify-write.
unsafe fn revoke_and_mark_pending_locked(
    lease_path: &Path,
    lease: &LeaseFile,
    sid: *mut core::ffi::c_void,
) -> Result<LeaseFile> {
    let intents: Vec<&AclIntent> = lease.intents.iter().collect();
    unsafe { revoke_intents(&intents, sid)? };

    let mut cleanup = lease.clone();
    cleanup.state = LeaseState::AclRevoked;
    cleanup.refresh_digest();
    rewrite_synced_lease(lease_path, &cleanup)?;

    cleanup.state = LeaseState::ProfileDeletionPending;
    cleanup.refresh_digest();
    rewrite_synced_lease(lease_path, &cleanup)?;
    Ok(cleanup)
}

/// Delete the AppContainer profile this execution exclusively owns.
///
/// Needs NO lock, for the same reason `allocate_unique_profile` does not: the
/// name is unique per (pid, process-creation-time, counter), so no other
/// process can name this profile. It is the single most expensive step in the
/// whole lifecycle — ~40ms of AppX profile-service RPC, measured on SEANDESKTOP
/// at cleanup_us=39785..53863 — which is precisely why it must not be holding a
/// machine-wide mutex while it runs.
unsafe fn delete_owned_profile(profile_name: &str) -> Result<()> {
    let profile = widen(profile_name);
    let delete_hr = unsafe { DeleteAppContainerProfile(profile.as_ptr()) };
    if !profile_delete_succeeded(delete_hr) {
        return Err(exec_error(format!(
            "DeleteAppContainerProfile({profile_name}) failed: {delete_hr:#x}"
        )));
    }
    Ok(())
}

/// Retire the lease once its profile is gone. MUST run under [`MutationLock`].
fn finalize_cleaned_locked(lease_path: &Path, mut cleanup: LeaseFile) -> Result<()> {
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
        let file_type = entry
            .file_type()
            .map_err(|error| exec_error(format!("stat ACL lease {}: {error}", path.display())))?;
        // The quarantine directory is the one non-lease entry that legitimately
        // lives here, and it MUST be skipped explicitly. The rejection below
        // treats every unrecognised entry as a hard error that aborts the whole
        // recovery pass, so a quarantine directory that was not allow-listed
        // would itself wedge the sandbox permanently on the very next
        // acquisition — reproducing the exact defect this quarantine path
        // exists to remove.
        if file_type.is_dir()
            && path.file_name().and_then(OsStr::to_str) == Some(QUARANTINE_DIRECTORY)
        {
            continue;
        }
        if !file_type.is_file() || path.extension().and_then(OsStr::to_str) != Some("toml") {
            return Err(exec_error(format!(
                "unknown entry in AppContainer ACL lease directory: {}",
                path.display()
            )));
        }
        paths.push(path);
    }
    paths.sort();

    for path in paths {
        // #369 c1. EVERY failure recovering ONE lease is bounded to that
        // lease. Before this, any `Err` raised below propagated out of the
        // loop, out of `recover_dead_leases_locked`, and out of
        // `ExecutionIdentity::start` -- so a single unrecoverable file
        // disabled ALL sandboxed execution on the machine, permanently,
        // because nothing expired it and every later process re-read it and
        // failed the same way. MEASURED: twelve days on SEANDESKTOP, cleared
        // by moving one file aside.
        //
        // Bounding it HERE and not at each failing call site is deliberate,
        // and is the difference between closing c1 as written and closing an
        // easier adjacent property. Two failure shapes already had bespoke
        // reclamation (`reclaim_zero_length_lease`,
        // `reclaim_unreconcilable_lease`) and the one #369 actually reported
        // -- `cleanup_locked` failing inside `remove_and_verify_exact_sid` --
        // had none, because the list of shapes was being extended one
        // incident at a time. A criterion that says "a lease that cannot be
        // recovered" means every way it can fail to recover, including the
        // ones nobody has hit yet.
        if let Err(error) = unsafe { recover_one_dead_lease_locked(&path) } {
            quarantine_unrecoverable_lease(&path, &error)?;
        }
    }
    Ok(())
}

/// Recover ONE dead-owner lease, or say why it could not be.
///
/// Split out of the sweep so that its caller can bound a failure to this one
/// file. Every `?` in here used to abort the whole pass; see the call site.
unsafe fn recover_one_dead_lease_locked(path: &Path) -> Result<()> {
    {
        // An interrupted create leaves a 0-byte lease, which
        // `read_validated_lease` rejects — and that rejection used to propagate
        // straight out of this loop, wedging the sandbox permanently on every
        // later acquisition (`F-28-ADJ-002`). Reproduced on `seandesktop` at
        // `1b9f148f`: `ran=False` on two consecutive runs, backend degraded to
        // `fail_closed`, diagnostic `invalid AppContainer ACL lease size 0`.
        //
        // Reclaiming it is safe for a reason worth stating, because it is the
        // only thing separating this from destroying a live writer's file: the
        // sole production caller of `write_new_synced_lease`
        // (`start_with_apply`) holds the SAME `MutationLock` this recovery pass
        // runs under, across the whole create-then-write sequence. A 0-byte
        // lease visible here therefore cannot belong to a writer that is still
        // running; it is a crash or power-loss remnant. This is the exact
        // argument `recover_rewrite_temps` already relies on to delete orphaned
        // `.rewrite-*.tmp` files unconditionally.
        if lease_is_zero_length(path)? {
            reclaim_zero_length_lease(path)?;
            return Ok(());
        }
        let lease = read_validated_lease(path)?;
        if owner_is_live(&lease)? {
            return Ok(());
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
            remove_validated_lease(path)?;
            return Ok(());
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
            reclaim_unreconcilable_lease(
                path,
                &lease,
                &format!(
                    "no AppContainer SID can be derived from its profile name {:?} (hr={derive_hr:#x})",
                    lease.profile_name
                ),
            )?;
            return Ok(());
        }
        let sid_guard = SidFreeGuard(derived_sid);
        let bytes = unsafe { sid_bytes(sid_guard.0)? };
        if !constant_time_eq(sha256_hex(&bytes).as_bytes(), lease.sid_sha256.as_bytes()) {
            reclaim_unreconcilable_lease(path, &lease, &unreconcilable_lease_reason(&lease))?;
            return Ok(());
        }
        unsafe { cleanup_locked(path, &lease, sid_guard.0)? };
    }
    Ok(())
}

/// Move a lease that could not be recovered out of the way, and TELL the
/// operator, rather than refusing every sandboxed command until a human finds
/// the file (`#369` c1).
///
/// # Why quarantining is right here even though the grants may still stand
///
/// This runs after a recovery attempt has already failed, and the attempt that
/// failed most often is `remove_and_verify_exact_sid`, which re-reads the DACL
/// and retries three times before giving up -- so by the time this is reached,
/// transience has been excluded rather than assumed. What is left is a lease
/// whose ACEs may STILL be applied, and that is exactly what the report says.
///
/// Refusing forever did not revoke them either. It left the same ACEs on disk
/// AND disabled every sandboxed command on the machine, so it is dominated on
/// both axes; the only thing it bought was silence. The file is MOVED, never
/// deleted, so the recorded intents remain inspectable and an operator can act
/// on the named paths.
///
/// If the quarantine itself fails there is nothing left to try, and the error
/// propagates: a lease that can neither be recovered NOR moved aside is a
/// genuine reason to fail closed.
fn quarantine_unrecoverable_lease(path: &Path, cause: &SandboxError) -> Result<()> {
    // Read BEFORE the move, and best-effort: this lease has already defeated
    // the recovery path, so it may be exactly the file that cannot be parsed.
    // `None` means the intents are unknown, which the report says rather than
    // reporting "nothing was left behind". Reading it after `quarantine_lease`
    // would ALWAYS be `None`, because the file is no longer at `path` -- which
    // would turn the honest three-case report into a constant.
    let lease = read_validated_lease(path).ok();
    let destination = quarantine_lease(path)?;
    let report = unrecoverable_lease_report(&lease, path, &destination, cause);
    #[cfg(test)]
    record_emitted_reclamation(&report);
    tracing::error!(
        target: "wcore_sandbox",
        lease = %path.display(),
        quarantined_to = %destination.display(),
        "{report}"
    );
    Ok(())
}

/// What an operator is told when a lease could not be recovered at all.
///
/// The residual clause has THREE cases and not two, because "we could not read
/// it" is not "it granted nothing". Collapsing them is how a report reassures
/// an operator about state it never inspected.
fn unrecoverable_lease_report(
    lease: &Option<LeaseFile>,
    path: &Path,
    destination: &Path,
    cause: &SandboxError,
) -> String {
    let residual = match lease {
        Some(lease) if lease.intents.is_empty() => {
            "It recorded NO filesystem ACL grant, so nothing was left behind on this machine."
                .to_string()
        }
        Some(lease) => format!(
            "Its recovery FAILED PART WAY, so the {} filesystem ACL grant(s) it recorded may              still be applied and could NOT be revoked automatically. Review those paths: {}.",
            lease.intents.len(),
            lease
                .intents
                .iter()
                .map(|intent| intent.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => "Its contents could not be read, so WHICH filesystem ACL grants it recorded                  is unknown -- inspect the quarantined file itself rather than assuming it                  granted nothing."
            .to_string(),
    };
    format!(
        "QUARANTINED an AppContainer ACL lease {} that could not be recovered: {cause}. This          is persistent on-disk state -- NOT a platform limitation, NOT an SSH or session-0          effect, and NOT transient. Until this quarantine landed, a lease in this state          disabled ALL sandboxed execution on this machine for as long as it existed, and the          only symptom was every command refusing. The file has been MOVED (not deleted) to          {} so the cause stays inspectable. {residual}",
        path.display(),
        destination.display()
    )
}

/// Why a dead-owner lease can never reconcile against its own profile.
///
/// A lease bearing the test SID sentinel is called out by name and never
/// reported as a generic mismatch: the two need different remedies, and
/// conflating them cost this program weeks.
fn unreconcilable_lease_reason(lease: &LeaseFile) -> String {
    if constant_time_eq(
        lease.sid_sha256.as_bytes(),
        TEST_SID_SENTINEL_SHA256.as_bytes(),
    ) {
        return "it was written by wcore-sandbox's OWN TEST SUITE (it carries the test SID \
                sentinel) and can never match a real AppContainer profile"
            .to_string();
    }
    format!(
        "the SID recorded in it does not match the SID derived from its profile {:?}",
        lease.profile_name
    )
}

/// Reclaim a dead-owner lease that can never reconcile, and say so in terms an
/// operator can act on.
///
/// This is the path whose ABSENCE was `F-28-02-002`. Before it existed, both
/// unreconcilable cases returned `Err`, which aborted the whole recovery pass
/// and therefore every later `ExecutionIdentity::start`. Nothing expired the
/// file and nothing reclaimed it, the negative probe cache is in-process only,
/// and so every subsequent process re-read the same file and failed the same
/// way: a permanent denial of sandboxed execution caused by a file nobody knew
/// to look for. Measured on `seandesktop` 2026-07-27 (`28-02`, observations 3
/// and 6): probe `unavailable`, `ran=False`, the product refusing to execute.
///
/// Reclamation is gated on the owning process being provably gone — the caller
/// reaches this only after `owner_is_live` returned false — so a lease held by
/// a RUNNING owner is still honoured untouched. That leg matters as much as
/// this one: reclaiming a live owner's lease would revoke the ACLs of a
/// container that is still executing.
///
/// The refusal itself was never the bug. Failing closed is correct, and it is
/// unchanged for every condition that still warrants it. The bug was that this
/// particular condition is **self-clearing** — a dead owner's unreconcilable
/// lease has no authority over anything — and the product treated it as
/// permanent, behind a message that read like a platform limitation.
fn reclaim_unreconcilable_lease(path: &Path, lease: &LeaseFile, reason: &str) -> Result<()> {
    let destination = quarantine_lease(path)?;
    let report = reclamation_report(lease, &destination, reason);
    #[cfg(test)]
    record_emitted_reclamation(&report);
    tracing::error!(
        target: "wcore_sandbox",
        lease = %path.display(),
        quarantined_to = %destination.display(),
        owner_pid = lease.owner_pid,
        "{report}"
    );
    Ok(())
}

/// Reclaim a 0-byte lease left behind by an interrupted create.
///
/// Reuses the quarantine path rather than introducing a second recovery
/// concept: the file is MOVED, not deleted, so an operator investigating an
/// interrupted run can still see that it happened and when.
///
/// The file carries no owner identity to check — it has no content at all — so
/// unlike [`reclaim_unreconcilable_lease`] the liveness gate cannot apply here.
/// The mutation lock supplies the equivalent guarantee; see the call site.
fn reclaim_zero_length_lease(path: &Path) -> Result<()> {
    let destination = quarantine_lease(path)?;
    let report = zero_length_report(path, &destination);
    #[cfg(test)]
    record_emitted_reclamation(&report);
    tracing::error!(
        target: "wcore_sandbox",
        lease = %path.display(),
        quarantined_to = %destination.display(),
        "{report}"
    );
    Ok(())
}

/// Operator-facing text for a 0-byte lease reclamation, as a pure function.
///
/// Separate from [`reclamation_report`] because the two say different things: a
/// zero-length lease never recorded an ACL grant, so there is nothing that
/// could have been left behind, and claiming otherwise would be noise.
fn zero_length_report(path: &Path, destination: &Path) -> String {
    format!(
        "RECLAIMED a 0-byte AppContainer ACL lease {}. A lease file is created before its \
         content is written, so an execution interrupted in that window leaves an empty \
         file. This is persistent on-disk state — NOT a platform limitation and NOT \
         transient — and until this reclamation landed it disabled ALL sandboxed execution \
         on this machine until a human deleted it. It was empty, so it recorded no \
         filesystem ACL grant and nothing was left behind. The file has been MOVED (not \
         deleted) to {} so the interruption stays visible.",
        path.display(),
        destination.display()
    )
}

/// Every reclamation report actually emitted, recorded for tests only.
///
/// This seam exists because of `F-28-ADJ-001`. The test named for the
/// residual-grant disclosure asserted only that the QUARANTINED FILE still
/// contained the grant path — which the move guarantees whatever the report
/// says — so deleting the disclosure branch outright left the suite at a
/// byte-identical 133 passed / 0 failed. It was a test that never called the
/// function it was named for.
///
/// Asserting on [`reclamation_report`] alone would close only half of that: it
/// would not prove a REAL reclamation passes the real lease to it, so an
/// implementation that logged a constant would still pass. Recording the exact
/// string handed to `tracing` is what makes the test observe what an operator
/// would actually read.
#[cfg(test)]
static EMITTED_RECLAMATIONS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn record_emitted_reclamation(report: &str) {
    EMITTED_RECLAMATIONS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(report.to_string());
}

/// Drain and return every reclamation reported since the last call.
#[cfg(test)]
pub(super) fn take_emitted_reclamations() -> Vec<String> {
    std::mem::take(
        &mut *EMITTED_RECLAMATIONS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()),
    )
}

/// The operator-facing text of a reclamation, as a pure function.
///
/// Deliberately NOT inlined into the `tracing` call. What an operator reads is
/// the whole remedy for a defect that survived for weeks precisely because its
/// message pointed away from its cause, so the wording is a behaviour worth
/// pinning in a test rather than a formatting detail.
fn reclamation_report(lease: &LeaseFile, destination: &Path, reason: &str) -> String {
    // Stated rather than glossed: a mismatching SID cannot be reconstructed
    // from its digest, so any ACL grant the lease recorded cannot be revoked
    // automatically. Refusing forever did not revoke them either — it only
    // also disabled the sandbox — so quarantining strictly dominates, but the
    // operator is told exactly what may remain.
    let residual = if lease.intents.is_empty() {
        "It recorded NO filesystem ACL grant, so nothing was left behind on this machine."
            .to_string()
    } else {
        format!(
            "It recorded {} filesystem ACL grant(s) under a SID that cannot be \
             reconstructed from its digest, so they could NOT be revoked automatically \
             and may remain on: {}. Review those paths.",
            lease.intents.len(),
            lease
                .intents
                .iter()
                .map(|intent| intent.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "RECLAIMED a stale AppContainer ACL lease: {reason}, and its owning process {} is \
         gone. This was persistent on-disk state — NOT a platform limitation, NOT an SSH \
         or session-0 effect, and NOT transient. Until this reclamation landed, a file in \
         this state disabled ALL sandboxed execution on this machine until a human DELETED \
         it. The file has been MOVED (not deleted) to {} so the cause stays inspectable. \
         {residual}",
        lease.owner_pid,
        destination.display()
    )
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
