use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
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

/// How long ONE holder may keep the mutation lock before we call it wedged.
///
/// The critical section is bounded, purely local work by this product's own
/// code: a lease-directory recovery pass, `CreateAppContainerProfile`, a few
/// synced lease writes, and DACL edits on paths `validate_local_canonical_path`
/// has already proven are local (never UNC, never a device path). Nothing in it
/// waits on a child process, a network, or an untrusted peer. A holder that
/// *dies* releases the mutex as abandoned, which [`MutationLock::acquire`]
/// already treats as an acquisition. So exceeding this budget for a single
/// holder means the OS itself stalled — not that the machine is busy.
const MUTATION_LOCK_HOLDER_BUDGET: Duration = Duration::from_secs(15);

/// How many holders may legitimately be queued ahead of us before we stop
/// waiting.
///
/// This is the whole of the fix for the 15-second cliff. `MUTATION_LOCK_TIMEOUT`
/// used to be a flat 15s, which is a budget on *queue depth* wearing the
/// costume of a wedge detector: the lock is contended machine-wide (one mutex
/// per user, named `Global\…`) while every sandboxed execution takes it THREE
/// times (setup, exit-marking, cleanup) and each hold pays a
/// `CreateAppContainerProfile`/`DeleteAppContainerProfile` pair plus several
/// `FlushFileBuffers`-class lease writes. Three concurrent executions is
/// therefore ~18 serialized critical sections, and on a loaded host that
/// exceeds 15s — at which point the product did not queue, it FAILED, and the
/// caller saw `sandbox UNAVAILABLE … refusing to run`.
///
/// Concurrency is not a fault. Waiting is the designed behaviour ("Profile
/// creation and DACL changes are serialized…", see the module docs on
/// `acl_lease`), so the deadline is now expressed as what it actually bounds:
/// a per-holder stall budget times a bound on how deep the queue may be.
///
/// Stated honestly, because a Win32 mutex gives a waiter no way to observe
/// hand-offs: this does NOT distinguish sixteen holders taking a second each
/// from one holder wedged for four minutes. It trades a slower verdict on the
/// (OS-level, unfixable-here) wedge for not failing the ordinary case, and it
/// makes the wait loud — every elapsed budget logs a warning naming the
/// elapsed time — so a wedge is visible long before the deadline.
const MUTATION_LOCK_MAX_QUEUED_HOLDERS: u32 = 16;

/// Total nanoseconds this PROCESS has spent blocked waiting for the mutation
/// lock.
///
/// Read by `windows_impl::process::probe_appcontainer_available` so its hard
/// wall-clock guard keeps bounding what it says it bounds — a stalled Win32
/// setup call — instead of counting time spent queued behind another
/// execution's critical section as a stall. Monotonic, so a caller can take a
/// delta across a window; never reset.
pub(crate) static MUTATION_LOCK_WAIT_NANOS: AtomicU64 = AtomicU64::new(0);

const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
const LOCAL_SYSTEM_RID: u32 = 18;

pub(super) struct MutationLock(OwnedHandle);

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
        wait_for_mutation_mutex(handle.0)?;
        Ok(Self(handle))
    }
}

/// Granularity at which an in-flight wait is published to
/// [`MUTATION_LOCK_WAIT_NANOS`].
///
/// The wait CANNOT be charged in one lump when it finishes. The AppContainer
/// probe's stall guard reads this counter while its worker is *still blocked*;
/// a counter that only moves on completion is always zero at exactly the moment
/// the guard needs it, and the guard fires on a queue it cannot see. So the
/// wait is sliced and each slice is charged as it elapses. One second bounds
/// the un-published residue, which is what sets how tightly the guard can track
/// reality; the syscall cost (one `WaitForSingleObject` per second) is nothing
/// next to the critical section it is waiting on.
const MUTATION_LOCK_CHARGE_SLICE: Duration = Duration::from_secs(1);

/// Block until this process owns `handle`, tolerating a queue but not a stall.
///
/// Waits up to [`MUTATION_LOCK_HOLDER_BUDGET`] ×
/// [`MUTATION_LOCK_MAX_QUEUED_HOLDERS`], warning once per elapsed holder budget
/// and charging every elapsed second to [`MUTATION_LOCK_WAIT_NANOS`] as it
/// happens. The accounting is what lets the AppContainer probe's wall-clock
/// guard keep meaning "a Win32 setup call stalled" rather than "this host is
/// running more than one sandboxed command".
fn wait_for_mutation_mutex(handle: HANDLE) -> Result<()> {
    let total_budget = MUTATION_LOCK_HOLDER_BUDGET * MUTATION_LOCK_MAX_QUEUED_HOLDERS;
    let slice = MUTATION_LOCK_CHARGE_SLICE.as_millis() as u32;
    let started = Instant::now();
    let mut charged = Duration::ZERO;
    let mut next_warning = MUTATION_LOCK_HOLDER_BUDGET;
    loop {
        let wait = unsafe { WaitForSingleObject(handle, slice) };
        let elapsed = started.elapsed();
        // Publish before acting on the result, so the failure path charges too:
        // a caller that discounts queue time must be able to see the queue that
        // killed it.
        MUTATION_LOCK_WAIT_NANOS.fetch_add(
            u64::try_from(elapsed.saturating_sub(charged).as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        charged = elapsed;

        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            return Ok(());
        }
        if wait != WAIT_TIMEOUT {
            return Err(last_error(
                "WaitForSingleObject(AppContainer ACL mutation lock)",
            ));
        }
        if elapsed >= total_budget {
            return Err(exec_error(format!(
                "timed out acquiring AppContainer ACL mutation lock after {}s. The lock is \
                 machine-wide (one per user) and every sandboxed execution takes it, so this \
                 is either far more than {} concurrent sandboxed commands, or one holder \
                 wedged inside a Win32 call.",
                elapsed.as_secs(),
                MUTATION_LOCK_MAX_QUEUED_HOLDERS
            )));
        }
        if elapsed >= next_warning {
            tracing::warn!(
                target: "wcore_sandbox",
                waited_secs = elapsed.as_secs(),
                budget_secs = total_budget.as_secs(),
                "still queued for the AppContainer ACL mutation lock — another sandboxed \
                 execution on this machine holds it. This is serialization, not a failure; \
                 it becomes one only if no holder ever releases."
            );
            next_warning += MUTATION_LOCK_HOLDER_BUDGET;
        }
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
        if unsafe { ReleaseMutex(self.0.0) } == 0 {
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
    use std::time::Instant;

    /// Seconds the helper child keeps the mutation lock once it has it.
    ///
    /// Parameterised (rather than the previous hard-coded 2s) so a test can put
    /// a hold LONGER than the old flat 15-second timeout in front of the
    /// parent — which is the only way to observe whether a waiter queues or
    /// gives up.
    const HELPER_HOLD_SECS_ENV: &str = "WCORE_MUTEX_HELPER_HOLD_SECS";

    #[test]
    fn mutation_lock_helper_entry() {
        let Some(marker) = std::env::var_os("WCORE_MUTEX_HELPER_MARKER") else {
            return;
        };
        let hold = std::env::var(HELPER_HOLD_SECS_ENV)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(2);
        let _lock = MutationLock::acquire().unwrap();
        fs::write(marker, b"locked").unwrap();
        std::thread::sleep(Duration::from_secs(hold));
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

    /// The regression for the 15-second cliff.
    ///
    /// A holder that keeps the machine-wide lock for longer than the OLD flat
    /// `MUTATION_LOCK_TIMEOUT` (15s) used to fail every other execution on the
    /// host with `timed out acquiring AppContainer ACL mutation lock`. On real
    /// hardware that holder is not a contrived helper: it is three or four
    /// ordinary sandboxed commands queued ahead of you, each paying a
    /// `CreateAppContainerProfile`/`DeleteAppContainerProfile` pair and several
    /// synced lease writes, three times over.
    ///
    /// The hold is deliberately > 15s so this test cannot pass under the old
    /// constant. It also pins the accounting the AppContainer probe's stall
    /// guard depends on: the wait must be CHARGED to
    /// [`MUTATION_LOCK_WAIT_NANOS`], or the guard has nothing to discount and
    /// contention is still reported as "sandbox UNAVAILABLE".
    #[test]
    #[ignore = "explicit native Windows AppContainer acceptance"]
    fn acquire_queues_behind_a_holder_that_outlives_the_old_fifteen_second_budget() {
        assert_eq!(
            std::env::var_os("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
            Some(OsStr::new("1"))
        );
        // Above the old 15s cliff with margin on BOTH sides: high enough that a
        // descheduled parent still measures >= 15s, low enough that this plus
        // the marker deadline stays under nextest's 60s hard kill
        // (`slow-timeout = { period = "30s", terminate-after = 2 }`).
        const HOLD: Duration = Duration::from_secs(20);
        const MARKER_DEADLINE: Duration = Duration::from_secs(20);
        assert!(
            HOLD > Duration::from_secs(15),
            "the hold must exceed the old flat timeout or this test cannot fail"
        );

        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("locked");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("mutation_lock_helper_entry")
            .arg("--nocapture")
            .env("WCORE_MUTEX_HELPER_MARKER", &marker)
            .env(HELPER_HOLD_SECS_ENV, HOLD.as_secs().to_string())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + MARKER_DEADLINE;
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(marker.exists(), "child never acquired global mutex");

        let charged_before = MUTATION_LOCK_WAIT_NANOS.load(Ordering::Relaxed);

        // Sample the counter WHILE the main thread is still blocked. This is
        // the assertion that matters most and the one a "charge it when the
        // wait finishes" implementation fails: the probe's stall guard reads
        // this counter at the moment its worker is still queued, so a counter
        // that only moves on completion is always zero exactly when it is
        // needed, and the guard fires on a queue it cannot see.
        let midflight = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(8));
            MUTATION_LOCK_WAIT_NANOS.load(Ordering::Relaxed)
        });

        let started = Instant::now();
        let lock = MutationLock::acquire()
            .expect("a waiter must QUEUE behind a busy host, not fail it: this is the 15s cliff");
        let waited = started.elapsed();
        drop(lock);

        assert!(
            waited >= Duration::from_secs(15),
            "parent acquired in {waited:?}, before the old cliff — the holder was not \
             actually still holding, so this run proves nothing"
        );
        let midflight = midflight.join().unwrap() - charged_before;
        assert!(
            Duration::from_nanos(midflight) >= Duration::from_secs(5),
            "after 8s of a {waited:?} wait the counter had moved only {midflight}ns — an \
             in-flight queue is invisible, so the probe's stall guard has nothing to \
             discount while it still matters"
        );
        let charged = MUTATION_LOCK_WAIT_NANOS.load(Ordering::Relaxed) - charged_before;
        assert!(
            Duration::from_nanos(charged) >= Duration::from_secs(15),
            "wait was not charged to MUTATION_LOCK_WAIT_NANOS ({charged}ns); the probe's \
             stall guard has nothing to discount"
        );
        assert!(child.wait().unwrap().success());
    }
}
