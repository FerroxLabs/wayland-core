use super::*;
// wayland#945 — the wait budget, the operator override and the wording of
// the timeout message live outside this cfg(windows) island so they can be
// graded on every platform. See `acl_lock_policy`.
use crate::backends::appcontainer::acl_lock_policy::{
    HolderNote, MUTATION_LOCK_SLICE, attempt_budget, contended_timeout_message, parse_holder_note,
};
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
    CreateMutexW, GetCurrentProcessId, GetExitCodeProcess, MUTEX_ALL_ACCESS, OpenProcess,
    OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, ReleaseMutex, WaitForSingleObject,
};

/// What `GetExitCodeProcess` reports while a process is still running.
const STILL_ACTIVE_EXIT_CODE: u32 = 259;

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
/// is what pushed one execution past the single 15 s wait this lock used to
/// ship with and made a second agent fail (wayland#945). Shortening the hold
/// means not re-granting a whole tree per execution, not trimming the intent
/// list; [`super::super::acl_lock_policy::MUTATION_LOCK_DEFAULT_TIMEOUT`] only
/// stops that hold from failing
/// the neighbour.
pub(super) struct MutationLock {
    handle: OwnedHandle,
    /// Advisory note naming this process while it holds the lock, so a
    /// CONTENDER can say who it is waiting on. See [`write_holder_note`].
    holder_note: PathBuf,
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

        // Wait in slices rather than in one shot. The kernel mutex would let
        // us pass the whole budget to a single `WaitForSingleObject`; the
        // slices exist so the holder is re-read on every expiry, which is what
        // turns "timed out on a lock" into "pid 4242 (wayland.exe) has it".
        let holder_note = holder_note_path(&token_user);
        let attempts = attempt_budget();
        for attempt in 1..=attempts {
            let wait =
                unsafe { WaitForSingleObject(handle.0, MUTATION_LOCK_SLICE.as_millis() as u32) };
            if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
                write_holder_note(&holder_note);
                return Ok(Self {
                    handle,
                    holder_note,
                });
            }
            if wait != WAIT_TIMEOUT {
                return Err(last_error(
                    "WaitForSingleObject(AppContainer ACL mutation lock)",
                ));
            }
            // Operator breadcrumb only. The USER-facing channel for this
            // condition is the returned `SandboxError` below, which the tool
            // surface renders — with `RUST_LOG` unset this line reaches nobody.
            tracing::warn!(
                target: "wcore_sandbox",
                attempt,
                attempts,
                holder = ?read_holder_note(&holder_note),
                "still waiting for the AppContainer ACL mutation lock"
            );
        }
        // The liveness probe is the one part of this message that needs
        // Windows, so it is resolved HERE and passed in as a verdict.
        let holder = read_holder_note(&holder_note);
        let holder_is_running = holder.as_ref().is_some_and(|h| process_is_running(h.pid));
        Err(exec_error(contended_timeout_message(
            attempts,
            holder,
            holder_is_running,
        )))
    }
}

/// Where the holder note lives.
///
/// `%TEMP%` is per-Windows-user and the mutex is keyed per user SID, so the two
/// scopes coincide exactly: a note found here always belongs to a process that
/// contends for the mutex this process is waiting on.
fn holder_note_path(token_user: &CurrentUserSid) -> PathBuf {
    std::env::temp_dir().join(format!(
        "WaylandCore.AppContainerAclLease.v1.{}.holder",
        &sha256_hex(token_user.bytes())[..32]
    ))
}

/// Record this process as the holder.
///
/// ADVISORY ONLY. Correctness of the lock is the kernel mutex and nothing here;
/// the note exists so a contender can NAME who it is waiting on instead of
/// reporting an anonymous timeout. Every failure is therefore swallowed — a
/// missing note costs a less specific message and nothing else.
fn write_holder_note(path: &Path) {
    let pid = unsafe { GetCurrentProcessId() };
    let _ = fs::write(path, format!("{pid}\n{}", current_image_name()));
}

fn current_image_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn read_holder_note(path: &Path) -> Option<HolderNote> {
    parse_holder_note(&fs::read_to_string(path).ok()?)
}

/// Whether `pid` names a process that is still running.
///
/// `OpenProcess` alone is not enough: a handle can still be opened for a
/// process that has exited but whose object is kept alive by another handle,
/// so the exit code decides.
fn process_is_running(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let handle = OwnedHandle(handle);
    let mut exit_code = 0u32;
    unsafe {
        GetExitCodeProcess(handle.0, &mut exit_code) != 0 && exit_code == STILL_ACTIVE_EXIT_CODE
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
        // Retract the claim BEFORE releasing the mutex, so the next holder
        // never reads a note naming this process as the current one.
        let _ = fs::remove_file(&self.holder_note);
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

    /// The ONE thing in the timeout message that needs Windows: `OpenProcess` +
    /// `GetExitCodeProcess`. The message WORDING it feeds is graded on every
    /// platform in `acl_lock_policy`; this grades the probe, in both
    /// directions, because a probe stuck at `true` would report every abandoned
    /// lock as a live contender and one stuck at `false` would do the reverse.
    #[test]
    fn the_liveness_probe_separates_a_live_pid_from_a_dead_one() {
        let live = unsafe { GetCurrentProcessId() };
        assert!(
            process_is_running(live),
            "our own pid {live} must read as running"
        );
        // pid 0 is the System Idle pseudo-pid; `OpenProcess` refuses it.
        assert!(
            !process_is_running(0),
            "pid 0 must not read as a live contender"
        );
    }

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

    /// THE assertion that cannot leave Windows: that the sliced wait actually
    /// BLOCKS on a real `Global\` kernel mutex held by a sibling process. Every
    /// other claim wayland#945 makes is arithmetic or wording and is graded on
    /// Linux and macOS too; this one needs a second Windows process and a
    /// kernel object, so it stays a native acceptance run.
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
}
