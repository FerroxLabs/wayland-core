use super::*;
use crate::backends::appcontainer::acl_lock_policy as policy;
use windows_sys::Win32::Foundation::WAIT_ABANDONED;
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetSecurityInfo, SE_KERNEL_OBJECT, SetEntriesInAclW,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER,
};
use windows_sys::Win32::Security::{
    AllocateAndInitializeSid, FreeSid, GetLengthSid, GetSecurityDescriptorControl,
    GetTokenInformation, InitializeSecurityDescriptor, IsValidSid, OWNER_SECURITY_INFORMATION,
    SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SID_IDENTIFIER_AUTHORITY,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, MUTEX_ALL_ACCESS, OpenProcessToken, ReleaseMutex, WaitForSingleObject,
};

const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
const LOCAL_SYSTEM_RID: u32 = 18;

/// Why this mutex is keyed per USER and not per workspace or per profile.
///
/// Narrowing the key is the obvious response to two agents blocking each other,
/// and it is unsound here for two independent reasons, both of which have to
/// stop being true before the key can shrink:
///
/// 1. **The lease directory is one shared per-user directory.** Every
///    `ExecutionIdentity::start` runs `recover_dead_leases_locked` over the
///    whole of `lease_directory()`, and reclaiming a dead owner's lease mutates
///    THAT owner's DACLs and deletes ITS profile. Two sweeps keyed on different
///    workspaces would run concurrently over the same files and the same
///    foreign DACLs, and could each try to reclaim the same abandoned lease.
///
/// 2. **Grant sets are never workspace-disjoint.** Every Contained execution
///    grants `minimal_toolchain_read_dirs()` (`%RUSTUP_HOME%` /
///    `%CARGO_HOME%\\bin`, defaulting to `~/.rustup` / `~/.cargo/bin`) and
///    the shared `%TEMP%\wayland-scratch` tree no matter which workspace it
///    runs in — see `wcore_tools::workspace_policy`. `apply_explicit_access` is
///    a read-modify-write of one object's DACL
///    (`GetNamedSecurityInfoW` → `SetEntriesInAclW` → `SetNamedSecurityInfoW`),
///    so two concurrent grants on `~/.cargo/bin` are a lost update: whichever
///    writes second writes back a DACL built from a snapshot taken before the
///    first, and the first execution's package-SID ALLOW silently disappears
///    while its child is still running. The symptom would be an intermittent
///    "cannot execute cargo" inside a sandbox that was granted cargo.
///
/// Per-user is also not over-broad: two users have disjoint lease directories
/// and disjoint `~`/`%TEMP%` trees, which is exactly the boundary the key draws.
///
/// The cost this serialises is NOT the number of DACL intents. Measured on
/// SEANDESKTOP (`measure_locked_phase_cost_per_execution`), an execution with 0
/// intents and one with 10 projected DACL writes both cost ~40 ms of setup and
/// ~41-46 ms of teardown; the same 4 intents over a 2000-file tree cost 239 ms
/// and 240 ms. The driver is `SUB_CONTAINERS_AND_OBJECTS_INHERIT` propagation,
/// ~100 µs per file under every granted directory, paid once on grant and again
/// on revoke — so hold time is O(files in the workspace), and a large checkout
/// is what pushes one execution past the acquisition budget below (default 15 s,
/// see `acl_lock_policy`) and makes a second agent fail. Shortening the hold
/// means not re-granting a whole tree per execution, not trimming the intent
/// list; raising `WAYLAND_SANDBOX_ACL_LOCK_TIMEOUT_SECS` only buys patience.
pub(super) struct MutationLock {
    handle: OwnedHandle,
    /// Where this process published itself as the lock's holder, so a
    /// contender's timeout can name it. `None` when the lease directory could
    /// not be resolved — that costs the holder's NAME, never the lock.
    holder_directory: Option<PathBuf>,
}

impl MutationLock {
    pub(super) fn acquire() -> Result<Self> {
        let token_user = CurrentUserSid::load()?;
        let name = widen(&mutex_name(&token_user));
        let system_sid = SystemSid::allocate()?;

        let mut entries: [EXPLICIT_ACCESS_W; 2] = unsafe { mem::zeroed() };
        for (entry, sid) in entries.iter_mut().zip([token_user.sid(), system_sid.sid()]) {
            entry.grfAccessPermissions = MUTEX_ALL_ACCESS;
            entry.grfAccessMode = GRANT_ACCESS;
            entry.grfInheritance = 0;
            entry.Trustee.TrusteeForm = TRUSTEE_IS_SID;
            entry.Trustee.TrusteeType = TRUSTEE_IS_USER;
            entry.Trustee.ptstrName = sid.cast();
        }

        let mut dacl = ptr::null_mut();
        let acl_rc = unsafe {
            SetEntriesInAclW(
                entries.len() as u32,
                entries.as_ptr(),
                ptr::null(),
                &mut dacl,
            )
        };
        if acl_rc != 0 || dacl.is_null() {
            return Err(exec_error(format!(
                "build AppContainer mutation-mutex DACL: {acl_rc:#x}"
            )));
        }
        let _dacl_guard = LocalFreeGuard(dacl.cast());

        let mut descriptor: SECURITY_DESCRIPTOR = unsafe { mem::zeroed() };
        let descriptor_ptr = ptr::addr_of_mut!(descriptor).cast();
        if unsafe { InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION) }
            == 0
            || unsafe { SetSecurityDescriptorOwner(descriptor_ptr, token_user.sid(), 0) } == 0
            || unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, dacl, 0) } == 0
            || unsafe {
                SetSecurityDescriptorControl(descriptor_ptr, SE_DACL_PROTECTED, SE_DACL_PROTECTED)
            } == 0
        {
            return Err(last_error(
                "initialize AppContainer mutation-mutex security descriptor",
            ));
        }

        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor_ptr,
            bInheritHandle: 0,
        };
        let handle = unsafe { CreateMutexW(&attributes, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(last_error("CreateMutexW(AppContainer ACL mutation lock)"));
        }
        let handle = OwnedHandle(handle);
        validate_mutex_security(handle.0, token_user.sid(), system_sid.sid())?;

        let timeout =
            policy::timeout_from(std::env::var(policy::ACL_LOCK_TIMEOUT_ENV).ok().as_deref());
        let outcome = policy::wait_with_retry(timeout, |slice| {
            match unsafe { WaitForSingleObject(handle.0, slice.as_millis() as u32) } {
                WAIT_OBJECT_0 | WAIT_ABANDONED => policy::WaitVerdict::Acquired,
                WAIT_TIMEOUT => policy::WaitVerdict::Timeout,
                _ => policy::WaitVerdict::Failed,
            }
        });
        // Resolved after the wait so a contended acquisition pays nothing for
        // it, and best-effort so a diagnostic can never fail an acquisition.
        let holder_directory = lease_directory().ok();
        match outcome.verdict {
            policy::WaitVerdict::Acquired => {}
            policy::WaitVerdict::Failed => {
                return Err(last_error(
                    "WaitForSingleObject(AppContainer ACL mutation lock)",
                ));
            }
            policy::WaitVerdict::Timeout => {
                let holder = holder_directory
                    .as_deref()
                    .and_then(|directory| policy::read_holder(directory, std::process::id()));
                return Err(exec_error(policy::timeout_message(
                    holder.as_ref(),
                    &outcome,
                )));
            }
        }
        if let Some(directory) = holder_directory.as_deref() {
            policy::publish_holder(
                directory,
                std::process::id(),
                &std::env::current_exe()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            );
        }
        Ok(Self {
            handle,
            holder_directory,
        })
    }
}

fn validate_mutex_security(
    handle: HANDLE,
    user_sid: *mut core::ffi::c_void,
    system_sid: *mut core::ffi::c_void,
) -> Result<()> {
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let rc = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if rc != 0 || descriptor.is_null() || owner.is_null() || dacl.is_null() {
        return Err(exec_error(format!(
            "query AppContainer mutation-mutex security: {rc:#x}"
        )));
    }
    let _descriptor_guard = LocalFreeGuard(descriptor);
    if unsafe { EqualSid(owner, user_sid) } == 0 {
        return Err(exec_error(
            "AppContainer mutation-mutex owner is not the current user".into(),
        ));
    }

    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(exec_error(
            "AppContainer mutation-mutex DACL is not protected".into(),
        ));
    }

    let same_authority = unsafe { EqualSid(user_sid, system_sid) } != 0;
    let expected_ace_count = if same_authority { 1 } else { 2 };
    let mut information: ACL_SIZE_INFORMATION = unsafe { mem::zeroed() };
    if unsafe {
        GetAclInformation(
            dacl,
            ptr::addr_of_mut!(information).cast(),
            mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || information.AceCount != expected_ace_count
    {
        return Err(exec_error(
            "AppContainer mutation-mutex DACL has an unexpected ACE count".into(),
        ));
    }

    let mut user_seen = false;
    let mut system_seen = false;
    for index in 0..information.AceCount {
        let mut raw = ptr::null_mut();
        if unsafe { GetAce(dacl, index, &mut raw) } == 0 || raw.is_null() {
            return Err(last_error("GetAce(AppContainer mutation mutex)"));
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE
            || ace.Header.AceFlags != 0
            || ace.Mask != MUTEX_ALL_ACCESS
        {
            return Err(exec_error(
                "AppContainer mutation-mutex contains an unexpected ACE".into(),
            ));
        }
        let sid = ptr::addr_of!(ace.SidStart).cast_mut().cast();
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(exec_error(
                "AppContainer mutation-mutex contains an invalid SID".into(),
            ));
        }
        if unsafe { EqualSid(sid, user_sid) } != 0 {
            user_seen = true;
            system_seen |= same_authority;
        } else if unsafe { EqualSid(sid, system_sid) } != 0 {
            system_seen = true;
        } else {
            return Err(exec_error(
                "AppContainer mutation-mutex grants an unexpected SID".into(),
            ));
        }
    }
    if !user_seen || !system_seen {
        return Err(exec_error(
            "AppContainer mutation-mutex DACL is missing current-user or SYSTEM authority".into(),
        ));
    }
    Ok(())
}

fn mutex_name(token_user: &CurrentUserSid) -> String {
    format!(
        "Global\\WaylandCore.AppContainerAclLease.v1.{}",
        &sha256_hex(token_user.bytes())[..32]
    )
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        if let Some(directory) = self.holder_directory.as_deref() {
            policy::clear_holder(directory);
        }
        if unsafe { ReleaseMutex(self.handle.0) } == 0 {
            tracing::error!(
                target: "wcore_sandbox",
                error = %last_error("ReleaseMutex(AppContainer ACL mutation lock)"),
                "failed to release AppContainer ACL mutation lock"
            );
        }
    }
}

struct CurrentUserSid {
    buffer: Vec<u8>,
}

impl CurrentUserSid {
    fn load() -> Result<Self> {
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error("OpenProcessToken(AppContainer mutation lock)"));
        }
        let token = OwnedHandle(token);
        let mut needed = 0;
        unsafe {
            GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(last_error(
                "GetTokenInformation(TokenUser) sizing for mutation lock",
            ));
        }
        let mut buffer = vec![0u8; needed as usize];
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(last_error(
                "GetTokenInformation(TokenUser) for mutation lock",
            ));
        }
        let value = Self { buffer };
        if unsafe { IsValidSid(value.sid()) } == 0 {
            return Err(exec_error("current TokenUser SID is invalid".into()));
        }
        Ok(value)
    }

    fn sid(&self) -> *mut core::ffi::c_void {
        unsafe { ptr::read_unaligned(self.buffer.as_ptr().cast::<TOKEN_USER>()) }
            .User
            .Sid
    }

    fn bytes(&self) -> &[u8] {
        let length = unsafe { GetLengthSid(self.sid()) } as usize;
        unsafe { std::slice::from_raw_parts(self.sid().cast::<u8>(), length) }
    }
}

struct SystemSid(*mut core::ffi::c_void);

impl SystemSid {
    fn allocate() -> Result<Self> {
        let authority = SID_IDENTIFIER_AUTHORITY {
            Value: [0, 0, 0, 0, 0, 5],
        };
        let mut sid = ptr::null_mut();
        if unsafe {
            AllocateAndInitializeSid(
                &authority,
                1,
                LOCAL_SYSTEM_RID,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                &mut sid,
            )
        } == 0
        {
            return Err(last_error("AllocateAndInitializeSid(LocalSystem)"));
        }
        Ok(Self(sid))
    }

    fn sid(&self) -> *mut core::ffi::c_void {
        self.0
    }
}

impl Drop for SystemSid {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                FreeSid(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Helper-process entry: acquire the machine-wide mutation mutex and hold
    /// it, so a sibling process can be observed contending for it.
    ///
    /// Two optional rendezvous files let a caller place the hold at a precise
    /// point in ITS OWN lifecycle rather than at helper start-up, which is what
    /// the W-B cleanup-timeout proof needs: the mutex must be free while the
    /// sandbox is being set up and taken by the time teardown runs.
    /// `WCORE_MUTEX_HELPER_GO` — wait for this path to exist before acquiring.
    /// `WCORE_MUTEX_HELPER_RELEASE` — release as soon as this path exists.
    #[test]
    fn mutation_lock_helper_entry() {
        let Some(marker) = std::env::var_os("WCORE_MUTEX_HELPER_MARKER") else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        if let Some(go) = std::env::var_os("WCORE_MUTEX_HELPER_GO") {
            let go = std::path::PathBuf::from(go);
            while !go.exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _lock = MutationLock::acquire().unwrap();
        fs::write(marker, b"locked").unwrap();
        match std::env::var_os("WCORE_MUTEX_HELPER_RELEASE") {
            None => std::thread::sleep(Duration::from_secs(2)),
            Some(release) => {
                let release = std::path::PathBuf::from(release);
                while !release.exists() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn global_user_keyed_mutex_serializes_processes() {
        assert_eq!(
            std::env::var_os("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
            Some(OsStr::new("1"))
        );
        let name = mutex_name(&CurrentUserSid::load().unwrap());
        assert!(name.starts_with("Global\\WaylandCore.AppContainerAclLease.v1."));

        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("locked");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("mutation_lock_helper_entry")
            .arg("--nocapture")
            .env("WCORE_MUTEX_HELPER_MARKER", &marker)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(marker.exists(), "child never acquired global mutex");

        let started = Instant::now();
        let lock = MutationLock::acquire().unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(750),
            "parent acquired while child still held the cross-process mutex"
        );
        drop(lock);
        assert!(child.wait().unwrap().success());
    }

    /// Live proof that a contended acquisition names the process holding it.
    ///
    /// This runs on ANY Windows box, with or without a working AppContainer
    /// profile: `MutationLock::acquire` calls OpenProcessToken,
    /// SetEntriesInAclW, CreateMutexW and WaitForSingleObject and no
    /// AppContainer API at all, so it is reachable where the real-spawn probe
    /// is not. The helper publishes its own holder sidecar, so the assertion is
    /// on a pid this test never told the message about.
    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn a_contended_acquisition_names_the_holding_process() {
        assert_eq!(
            std::env::var_os("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
            Some(OsStr::new("1"))
        );
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("locked");
        let release = temp.path().join("release");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("mutation_lock_helper_entry")
            .arg("--nocapture")
            .env("WCORE_MUTEX_HELPER_MARKER", &marker)
            .env("WCORE_MUTEX_HELPER_RELEASE", &release)
            // Share this process's lease root, or the helper publishes its
            // holder sidecar somewhere this process never looks.
            .env(TEST_LEASE_ROOT_ENV, test_lease_root().unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(marker.exists(), "helper never acquired the global mutex");

        let message = MutationLock::acquire()
            .err()
            .expect("the helper still holds the lock, so acquisition must time out")
            .to_string();
        fs::write(&release, b"go").unwrap();
        assert!(child.wait().unwrap().success());

        assert!(
            message.contains(&format!("pid {}", child.id())),
            "the timeout must name the contending process: {message:?}"
        );
        assert!(
            message.contains(policy::ACL_LOCK_TIMEOUT_ENV),
            "the timeout must offer a remedy: {message:?}"
        );
    }
}
