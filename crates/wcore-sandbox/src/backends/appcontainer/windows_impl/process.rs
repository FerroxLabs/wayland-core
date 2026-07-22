//! AppContainer backend spawn/execute pipeline (F20-03 Task 1A split of `windows_impl`).
#![allow(unused_imports)]

use super::super::super::SandboxBackend;
use super::super::appcontainer_acl_lease::ExecutionIdentity;
use super::super::{NEGATIVE_PROBE_TTL, ProbeCache};
use crate::error::{Result, SandboxError};
use crate::manifest::{NetworkPolicy, SandboxManifest};
use crate::{ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::mem;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    AllocateAndInitializeSid, CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, FreeSid, GetLengthSid,
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, SECURITY_ATTRIBUTES,
    SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, SID_IDENTIFIER_AUTHORITY, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};

use super::command::*;
use super::handles::*;
use super::*;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PRIORITY_CLASS,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_SUSPENDED, CreateProcessAsUserW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    OpenProcessToken, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

/// `SE_GROUP_INTEGRITY` from `winnt.h`. Not re-exported by windows-sys
/// (versions ≤ 0.59); defined locally per the Windows SDK header.
const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;

pub struct AppContainerBackend;

impl AppContainerBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AppContainerBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Probe cache: stores `Some(true)` once a real spawn has succeeded, and
/// stays sticky for the process lifetime. Negative results are cached
/// only for [`NEGATIVE_PROBE_TTL`], after which `is_available()`
/// re-probes. This avoids both the "transient flake at startup
/// permanently disables sandboxing" silent-failure pattern and the
/// re-probe-every-command hang of #125.
pub(super) fn probe_cache() -> &'static Mutex<ProbeCache> {
    static CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ProbeCache::new()))
}

/// Single-flight gate for the availability probe (FerroxLabs/wayland#754).
///
/// The probe cache alone does NOT prevent a stampede: when the cache is
/// cold, N concurrent `is_available()` callers all miss it *before* any of
/// them records a verdict, so each launches its OWN real AppContainer spawn
/// at the same instant. On Windows those parallel spawns contend on the
/// shared per-PID AppContainer profile / profile-service RPC and most of
/// them FAIL — and every failure is written into the cache as
/// `UnavailableUntil(now + NEGATIVE_PROBE_TTL)`, so for the next 30s
/// `default_for_platform()` returns `FailClosedBackend` and EVERY tool
/// command is refused ("sandbox UNAVAILABLE … refusing to run"). The agent
/// reads that as a failed command and retries, which the user sees as every
/// shell command timing out / returning empty / looping.
///
/// This gate serializes the SLOW (probe) path so only the first cold caller
/// actually spawns; the rest block briefly, then observe its verdict via the
/// double-checked cache read in `is_available()`. A single serial probe is
/// reliable (it is exactly the serial path every non-concurrent command
/// already takes), so the cache warms to `Available` (sticky) and all later
/// calls take the lock-free fast path. Held only across the cold probe, so it
/// adds no steady-state contention.
pub(super) fn probe_gate() -> &'static Mutex<()> {
    static GATE: OnceLock<Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
}

#[async_trait]
impl SandboxBackend for AppContainerBackend {
    fn name(&self) -> &'static str {
        "appcontainer"
    }

    /// PowerShell (`powershell.exe` / `pwsh.exe`) cannot load .NET / GAC
    /// assemblies under the Low-integrity restricted token (STATUS_DLL_NOT_FOUND,
    /// 0xC0000135). See FerroxLabs/wayland#413 / #324.
    fn blocks_powershell(&self) -> bool {
        true
    }

    fn owns_descendants_hard(&self) -> bool {
        true
    }

    /// Real-spawn availability probe.
    ///
    /// On first call, runs a wall-clock-guarded `cmd.exe /c exit 0`
    /// through the full pipeline. A success is cached permanently so
    /// subsequent calls return instantly. A failure is cached only for
    /// [`NEGATIVE_PROBE_TTL`]: a transient probe failure (AV scan, disk
    /// contention, slow profile-service RPC) neither permanently disables
    /// sandboxing (a silent security regression) nor re-runs the full
    /// probe on every command (the ~120s-per-Bash hang of #125). The
    /// probe itself is bounded by a hard wall-clock guard in
    /// [`probe_appcontainer_available`], so a stalled Win32 setup call can
    /// cost at most one guarded probe per TTL window.
    fn is_available(&self) -> bool {
        // Single-flight the probe so concurrent cold callers collapse onto
        // ONE real AppContainer spawn instead of stampeding it (#754). The
        // logic lives in a platform-independent helper so it is unit-tested
        // on every target; here it is driven by the real Win32 probe.
        super::super::probe_single_flight(
            probe_cache(),
            probe_gate(),
            NEGATIVE_PROBE_TTL,
            probe_appcontainer_available,
        )
    }

    fn enforces_read_deny(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        if matches!(manifest.network, NetworkPolicy::AllowHosts(_)) {
            return Err(SandboxError::PolicyNotSupported(
                "AppContainer has no DNS-name allowlist; use NetworkPolicy::Deny + WFP filter (v0.7.0)".into(),
            ));
        }
        let manifest = manifest.clone();
        // Defense-in-depth wall-clock ceiling (#125). `execute_blocking`'s
        // inner `WaitForSingleObject` bounds only the child's *run*, not the
        // Win32 setup calls before it (`CreateAppContainerProfile`,
        // `CreateProcessAsUserW`). Bound the whole blocking call at the
        // effective wait timeout plus a setup grace so a stalled setup call
        // cannot hang the async caller. The grace guarantees this ceiling
        // never preempts a legitimately-timed command (the inner wait always
        // fires first). Shared Job control turns timeout or future-drop
        // into an immediate full-tree termination. If cancellation lands
        // during pre-spawn setup, the worker observes it before process
        // creation and again atomically before resuming the suspended child.
        let ceiling = manifest
            .timeout
            .unwrap_or(Duration::from_secs(60))
            .saturating_add(Duration::from_secs(15));
        let control = Arc::new(JobControl::default());
        let mut cancellation = JobCancellationGuard::new(Arc::clone(&control));
        let worker_control = Arc::clone(&control);
        let handle =
            tokio::task::spawn_blocking(move || execute_blocking(&manifest, &cmd, &worker_control));
        let result = match tokio::time::timeout(ceiling, handle).await {
            Ok(joined) => joined.map_err(|e| SandboxError::ExecFailed(format!("join: {e}")))?,
            Err(_elapsed) => {
                control.cancel();
                Err(SandboxError::Timeout)
            }
        };
        cancellation.disarm();
        result
    }
}

pub(super) fn probe_appcontainer_available() -> bool {
    // Inner `manifest.timeout` bounds ONLY `WaitForSingleObject` (the wait
    // for the child to exit). It does NOT bound the Win32 setup calls
    // before that wait — `CreateAppContainerProfile` (profile-service RPC)
    // and `CreateProcessAsUserW` (image load under the Low-IL token, where
    // AV process-creation callbacks run synchronously) — either of which
    // can stall ~120s, so control never reaches the wait and this timeout
    // never fires (#125). The real bound is the wall-clock guard below.
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    let cmd = SandboxCommand {
        argv: vec![
            "cmd.exe".to_string(),
            "/c".to_string(),
            "exit 0".to_string(),
        ],
        cwd: None,
    };

    // Hard wall-clock guard: run the probe on a dedicated thread and bound
    // the whole thing with `recv_timeout`, so a stalled setup call upstream
    // of the wait cannot hang the caller. A timeout marks shared
    // cancellation: any published Job is terminated immediately, and a
    // worker stalled in pre-spawn setup refuses process creation on return.
    const PROBE_WALL_CLOCK: Duration = Duration::from_secs(15);
    let (tx, rx) = mpsc::channel();
    let control = Arc::new(JobControl::default());
    let worker_control = Arc::clone(&control);
    if std::thread::Builder::new()
        .name("appcontainer-probe".into())
        .spawn(move || {
            let _ = tx.send(execute_blocking(&manifest, &cmd, &worker_control));
        })
        .is_err()
    {
        tracing::error!(
            target: "wcore_sandbox",
            "could not spawn AppContainer probe thread; sandbox disabled."
        );
        return false;
    }

    match rx.recv_timeout(PROBE_WALL_CLOCK) {
        Ok(Ok(out)) if out.exit_code == 0 => true,
        Ok(Ok(out)) => {
            tracing::error!(
                target: "wcore_sandbox",
                exit_code = out.exit_code,
                "AppContainer real-spawn probe completed but exit code non-zero; \
                 sandbox disabled. WAYLAND_SANDBOX_LIVE_WINDOWS spawn may also fail."
            );
            false
        }
        Ok(Err(e)) => {
            tracing::error!(
                target: "wcore_sandbox",
                error = %e,
                "AppContainer real-spawn probe failed; sandbox disabled. \
                 If the failure is transient (AV, disk contention), the probe \
                 re-runs after the negative-cache TTL."
            );
            false
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            control.cancel();
            tracing::error!(
                target: "wcore_sandbox",
                guard_secs = PROBE_WALL_CLOCK.as_secs(),
                "AppContainer probe exceeded its hard wall-clock guard — a Win32 \
                 setup call (CreateAppContainerProfile / CreateProcessAsUserW) \
                 stalled, most likely an AV image scan or profile-service RPC. \
                 Treating the sandbox as unavailable for this probe; it re-runs \
                 after the negative-cache TTL (#125)."
            );
            false
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            control.cancel();
            tracing::error!(
                target: "wcore_sandbox",
                "AppContainer probe thread ended without a result; sandbox disabled."
            );
            false
        }
    }
}

pub(super) fn execute_blocking(
    manifest: &SandboxManifest,
    cmd: &SandboxCommand,
    control: &JobControl,
) -> Result<SandboxOutput> {
    control.ensure_active()?;
    if cmd.argv.is_empty() {
        return Err(SandboxError::ExecFailed("empty argv".into()));
    }

    let cwd_w: Option<Vec<u16>> = match cmd.cwd.as_ref() {
        Some(p) => {
            if !p.is_absolute() {
                return Err(SandboxError::ExecFailed(format!(
                    "cwd {p:?} must be absolute"
                )));
            }
            Some(widen_os(p.as_os_str()))
        }
        None => None,
    };

    let app_name_w = resolve_program(&cmd.argv[0])?;
    let mut identity = ExecutionIdentity::start(manifest)?;
    let sid_ptr = identity.sid();
    let package_root = identity.package_root();

    let execution = (|| -> Result<SandboxOutput> {
        unsafe {
            // ---- 2. Restricted token ----
            //
            // SidsToDisable: explicitly mark BUILTIN\Administrators,
            // BUILTIN\Users, and Authenticated Users as "for deny only" in
            // the child's token. Without this, an elevated parent leaves
            // these SIDs enabled, and any resource whose DACL grants those
            // groups would be reachable by the AppContainer child despite
            // the AppContainer SID restriction (Chromium / sandboxie use
            // the same pattern).
            let admins_sid = allocate_sid([0, 0, 0, 0, 0, 5], &[32, 544])?;
            let users_sid = allocate_sid([0, 0, 0, 0, 0, 5], &[32, 545])?;
            let auth_users_sid = allocate_sid([0, 0, 0, 0, 0, 5], &[11])?;
            let mut sids_to_disable: [SID_AND_ATTRIBUTES; 3] = [
                SID_AND_ATTRIBUTES {
                    Sid: admins_sid.as_psid(),
                    Attributes: 0,
                },
                SID_AND_ATTRIBUTES {
                    Sid: users_sid.as_psid(),
                    Attributes: 0,
                },
                SID_AND_ATTRIBUTES {
                    Sid: auth_users_sid.as_psid(),
                    Attributes: 0,
                },
            ];

            let mut current_token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(
                GetCurrentProcess(),
                // TOKEN_ADJUST_DEFAULT is required because CreateRestrictedToken
                // propagates the source token's access mask onto the new
                // handle, and SetTokenInformation(TokenIntegrityLevel, ...)
                // fails with 0x5 (ACCESS_DENIED) without it.
                TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
                &mut current_token,
            ) == 0
            {
                return Err(SandboxError::ExecFailed(format!(
                    "OpenProcessToken: {:#x}",
                    GetLastError()
                )));
            }
            let current_token = OwnedHandle::new(current_token);
            let mut restricted_raw: HANDLE = std::ptr::null_mut();
            if CreateRestrictedToken(
                current_token.as_raw(),
                DISABLE_MAX_PRIVILEGE,
                sids_to_disable.len() as u32,
                sids_to_disable.as_mut_ptr(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                &mut restricted_raw,
            ) == 0
            {
                return Err(SandboxError::ExecFailed(format!(
                    "CreateRestrictedToken: {:#x}",
                    GetLastError()
                )));
            }
            let restricted_token = OwnedHandle::new(restricted_raw);

            // ---- 3. Explicit Low Integrity Level ----
            //
            // AppContainer-tagged tokens are normally pinned to Low integrity
            // by the kernel during process creation, but explicitly setting
            // it on the restricted token defends against future Windows
            // changes and makes the contract visible in code review.
            let low_il_sid = allocate_sid([0, 0, 0, 0, 0, 16], &[0x1000])?;
            let label = TOKEN_MANDATORY_LABEL {
                Label: SID_AND_ATTRIBUTES {
                    Sid: low_il_sid.as_psid(),
                    Attributes: SE_GROUP_INTEGRITY,
                },
            };
            // sizeof(TOKEN_MANDATORY_LABEL) does NOT include the variable-
            // length SID body that `Sid` points at; the kernel reads the SID
            // via the pointer. Per Microsoft's `SetTokenInformation` examples
            // we pass sizeof(struct) + GetLengthSid(label.Label.Sid). We use
            // the conservative sum here even though many implementations get
            // away with just sizeof(TOKEN_MANDATORY_LABEL) — the conservative
            // size has zero downside.
            let label_size = (mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32)
                + GetLengthSid(low_il_sid.as_psid() as _);
            if SetTokenInformation(
                restricted_token.as_raw(),
                TokenIntegrityLevel,
                &label as *const _ as *const _,
                label_size,
            ) == 0
            {
                return Err(SandboxError::ExecFailed(format!(
                    "SetTokenInformation(IntegrityLevel=Low): {:#x}",
                    GetLastError()
                )));
            }

            // ---- 4. Job Object with FULL resource + UI limits ----
            let job_raw = CreateJobObjectW(ptr::null(), ptr::null());
            if job_raw.is_null() {
                return Err(SandboxError::ExecFailed(format!(
                    "CreateJobObjectW: {:#x}",
                    GetLastError()
                )));
            }
            let job = Arc::new(SharedJob::new(OwnedHandle::new(job_raw)));
            control.install(Arc::clone(&job))?;

            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            // Always-on hardening flags:
            //   KILL_ON_JOB_CLOSE        — child dies if engine drops job
            //   ACTIVE_PROCESS=N         — runaway-fork cap (see below)
            //   DIE_ON_UNHANDLED_EXC.    — no WerFault popup
            //   PRIORITY_CLASS=BELOW_N.  — child can't starve the engine
            //   BREAKAWAY_OK=0           — CREATE_BREAKAWAY_FROM_JOB rejected
            //   SILENT_BREAKAWAY_OK=0    — same for silent breakaway
            //
            // BREAKAWAY_OK and SILENT_BREAKAWAY_OK are not OR'd in (their
            // flag bits represent "allow breakaway"); leaving them unset is
            // the deny-default. Documented here for clarity.
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
                | JOB_OBJECT_LIMIT_PRIORITY_CLASS;
            // Defensive: explicitly clear the breakaway-allow bits in case a
            // future Windows / driver toggles the default.
            limits.BasicLimitInformation.LimitFlags &=
                !(JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK);
            // #322: an ActiveProcessLimit of 1 permits only the shell process
            // and structurally blocks EVERY subprocess (git, node, npm, a
            // parallel build), making the sandboxed Bash tool unusable for the
            // build/run workflows it exists to serve. Raise the cap to a value
            // high enough for normal command execution and parallel builds
            // while still bounding a runaway fork. KILL_ON_JOB_CLOSE plus the
            // optional PROCESS_MEMORY cap remain the meaningful fork-bomb
            // guards (a fork bomb exhausts memory long before 512 PIDs), so the
            // active-process cap can safely be raised off 1.
            limits.BasicLimitInformation.ActiveProcessLimit = SANDBOX_ACTIVE_PROCESS_LIMIT;
            limits.BasicLimitInformation.PriorityClass = BELOW_NORMAL_PRIORITY_CLASS;
            if let Some(mem_bytes) = manifest.max_memory_bytes {
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                limits.ProcessMemoryLimit = mem_bytes as usize;
            }
            if let Some(cpu_secs) = manifest.max_cpu_secs {
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
                let ticks = (cpu_secs as i64).saturating_mul(10_000_000);
                limits.BasicLimitInformation.PerProcessUserTimeLimit = ticks;
            }
            if SetInformationJobObject(
                job.as_raw(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(SandboxError::ExecFailed(format!(
                    "SetInformationJobObject(ExtendedLimit): {:#x}",
                    GetLastError()
                )));
            }

            // UI restrictions: deny clipboard, USER handle inheritance across
            // jobs, system parameter changes, display changes, global atoms,
            // desktop switches, and shutdown calls. AppContainer SIDs gate
            // KERNEL objects but not USER32 surfaces; these flags close that.
            let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
                UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES
                    | JOB_OBJECT_UILIMIT_READCLIPBOARD
                    | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
                    | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                    | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                    | JOB_OBJECT_UILIMIT_GLOBALATOMS
                    | JOB_OBJECT_UILIMIT_DESKTOP
                    | JOB_OBJECT_UILIMIT_EXITWINDOWS,
            };
            if SetInformationJobObject(
                job.as_raw(),
                JobObjectBasicUIRestrictions,
                &ui as *const _ as _,
                mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
            ) == 0
            {
                return Err(SandboxError::ExecFailed(format!(
                    "SetInformationJobObject(UIRestrictions): {:#x}",
                    GetLastError()
                )));
            }

            // ---- 5. Pipes for stdout / stderr ----
            let sa_inherit = SECURITY_ATTRIBUTES {
                nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: ptr::null_mut(),
                bInheritHandle: 1,
            };
            let mut stdout_r: HANDLE = std::ptr::null_mut();
            let mut stdout_w: HANDLE = std::ptr::null_mut();
            if CreatePipe(&mut stdout_r, &mut stdout_w, &sa_inherit, 0) == 0 {
                return Err(SandboxError::ExecFailed(format!(
                    "CreatePipe(stdout): {:#x}",
                    GetLastError()
                )));
            }
            let stdout_r = OwnedHandle::new(stdout_r);
            let stdout_w = OwnedHandle::new(stdout_w);
            let mut stderr_r: HANDLE = std::ptr::null_mut();
            let mut stderr_w: HANDLE = std::ptr::null_mut();
            if CreatePipe(&mut stderr_r, &mut stderr_w, &sa_inherit, 0) == 0 {
                return Err(SandboxError::ExecFailed(format!(
                    "CreatePipe(stderr): {:#x}",
                    GetLastError()
                )));
            }
            let stderr_r = OwnedHandle::new(stderr_r);
            let stderr_w = OwnedHandle::new(stderr_w);

            // ---- 6. Attribute list with SECURITY_CAPABILITIES + HANDLE_LIST ----
            //
            // Drop-order note: `sec_caps` and `handle_list` MUST be declared
            // BEFORE `_attr_guard`. UpdateProcThreadAttribute stores POINTERS
            // to these buffers in the attribute list; per the SDK contract the
            // backing storage must remain valid until `DeleteProcThreadAttributeList`
            // runs. Rust drops locals in reverse declaration order, so the
            // guard (which calls Delete...) must drop FIRST, before the
            // attribute backing buffers.
            let mut sec_caps = SECURITY_CAPABILITIES {
                AppContainerSid: sid_ptr as _,
                Capabilities: ptr::null_mut(),
                CapabilityCount: 0,
                Reserved: 0,
            };
            // PROC_THREAD_ATTRIBUTE_HANDLE_LIST overrides bInheritHandles=TRUE
            // globally: ONLY the handles in this list are inherited by the
            // child, even if other handles in the parent are flagged
            // inheritable. So `stdout_r` / `stderr_r` (also created
            // inheritable, for the parent's read end of the pipe) are NOT
            // inherited by the child despite their SECURITY_ATTRIBUTES.
            let mut handle_list: [HANDLE; 2] = [stdout_w.as_raw(), stderr_w.as_raw()];

            let mut attr_size: usize = 0;
            InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut attr_size);
            if attr_size == 0 {
                return Err(SandboxError::ExecFailed(
                    "InitializeProcThreadAttributeList sizing returned 0".into(),
                ));
            }
            let mut attr_buf: Vec<u8> = vec![0u8; attr_size];
            let attr_list: LPPROC_THREAD_ATTRIBUTE_LIST = attr_buf.as_mut_ptr() as _;
            if InitializeProcThreadAttributeList(attr_list, 2, 0, &mut attr_size) == 0 {
                return Err(SandboxError::ExecFailed(format!(
                    "InitializeProcThreadAttributeList: {:#x}",
                    GetLastError()
                )));
            }
            let _attr_guard = AttrListGuard { list: attr_list };

            if UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &mut sec_caps as *mut _ as _,
                mem::size_of::<SECURITY_CAPABILITIES>(),
                ptr::null_mut(),
                ptr::null(),
            ) == 0
            {
                return Err(SandboxError::ExecFailed(format!(
                    "UpdateProcThreadAttribute(SECURITY_CAPABILITIES): {:#x}",
                    GetLastError()
                )));
            }
            if UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handle_list.as_mut_ptr() as *mut _,
                mem::size_of::<HANDLE>() * handle_list.len(),
                ptr::null_mut(),
                ptr::null(),
            ) == 0
            {
                return Err(SandboxError::ExecFailed(format!(
                    "UpdateProcThreadAttribute(HANDLE_LIST): {:#x}",
                    GetLastError()
                )));
            }

            // ---- 7. STARTUPINFOEXW ----
            let mut sinfo: STARTUPINFOEXW = mem::zeroed();
            sinfo.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
            sinfo.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            sinfo.StartupInfo.hStdInput = std::ptr::null_mut();
            sinfo.StartupInfo.hStdOutput = stdout_w.as_raw();
            sinfo.StartupInfo.hStdError = stderr_w.as_raw();
            sinfo.lpAttributeList = attr_list;

            // ---- 8. Command line + env block ----
            let cmdline: String = cmd
                .argv
                .iter()
                .map(|a| quote_arg(a))
                .collect::<Vec<_>>()
                .join(" ");
            let mut cmdline_w: Vec<u16> = widen(&cmdline);

            let mut env_pairs: Vec<(String, String)> = Vec::new();
            for key in [
                "SYSTEMROOT",
                "WINDIR",
                "COMSPEC",
                "PATH",
                "PATHEXT",
                "PROCESSOR_ARCHITECTURE",
                "USERPROFILE",
                "APPDATA",
                "LOCALAPPDATA",
                "TEMP",
                "TMP",
                "USERNAME",
                "USERDOMAIN",
                "HOMEDRIVE",
                "HOMEPATH",
                "PROCESSOR_ARCHITEW6432",
                "NUMBER_OF_PROCESSORS",
                "ALLUSERSPROFILE",
                "PROGRAMDATA",
                "PROGRAMFILES",
                "PROGRAMFILES(X86)",
                "PROGRAMW6432",
                "COMMONPROGRAMFILES",
                "COMMONPROGRAMFILES(X86)",
                "COMMONPROGRAMW6432",
                "PUBLIC",
                "SYSTEMDRIVE",
            ] {
                if let Ok(val) = std::env::var(key) {
                    env_pairs.push((key.to_string(), val));
                }
            }
            // Remap TEMP/TMP to AppContainer-writable storage. If
            // LOCALAPPDATA is unset we cannot compute the package root —
            // warn loudly so the operator can fix it; child tools writing
            // to %TEMP% will then ACL-fail until they do.
            match package_root.as_ref() {
                Some(ac_root) => {
                    let temp_path = ac_root.join("Temp");
                    match std::fs::create_dir_all(&temp_path) {
                        Ok(()) => {
                            let temp_str = temp_path.to_string_lossy().into_owned();
                            env_pairs.push(("TEMP".to_string(), temp_str.clone()));
                            env_pairs.push(("TMP".to_string(), temp_str));
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "wcore_sandbox",
                                path = %temp_path.display(),
                                error = %e,
                                "create_dir_all on AppContainer Temp failed; \
                                 TEMP/TMP not remapped — child writes to %TEMP% will ACL-fail"
                            );
                        }
                    }
                }
                None => {
                    tracing::warn!(
                        target: "wcore_sandbox",
                        "LOCALAPPDATA env var is unset; AppContainer TEMP/TMP remap skipped. \
                         Child tools that write to %TEMP% will fail with ACL-denied. \
                         Set LOCALAPPDATA before invoking the engine to enable the remap."
                    );
                }
            }
            env_pairs.extend(manifest.env.iter().cloned());
            let env_block = build_env_block(&env_pairs)?;

            // Diagnostics — at debug level emit one summary line per spawn;
            // at trace level emit per-pair detail with redacted values for
            // unsafe keys. Both routed through `tracing` so operators control
            // via RUST_LOG.
            tracing::debug!(
                target: "wcore_sandbox",
                cmdline = %cmdline,
                program = %String::from_utf16_lossy(
                    &app_name_w[..app_name_w.len().saturating_sub(1)]
                ),
                cwd = ?cmd.cwd,
                env_pairs_n = env_pairs.len(),
                env_block_words = env_block.len(),
                "AppContainer spawn ready"
            );
            for (k, v) in &env_pairs {
                if is_trace_safe_env_key(k) {
                    tracing::trace!(
                        target: "wcore_sandbox",
                        env_key = %k,
                        env_value = %v.escape_debug()
                    );
                } else {
                    tracing::trace!(
                        target: "wcore_sandbox",
                        env_key = %k,
                        redacted_value_bytes = v.len(),
                        "env value redacted"
                    );
                }
            }

            // ---- 9. CreateProcessAsUserW (suspended) ----
            let mut pi: PROCESS_INFORMATION = mem::zeroed();
            // NOTE: do NOT add CREATE_NO_WINDOW here. Under the AppContainer
            // Low-IL restricted token, forcing `cmd.exe` window-less makes its
            // console-host init fail with 0xC0000142 (STATUS_DLL_INIT_FAILED) —
            // breaking every command. cmd needs its console host; the #100 hang
            // is instead handled at drain time by reaping the whole job tree, so
            // a lingering conhost can't keep the inherited pipe write-end open.
            let creation_flags =
            EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | 0x0400 /* CREATE_UNICODE_ENVIRONMENT */;
            // Setup calls above may have blocked while the async caller was
            // cancelled. Refuse to create a child after that cancellation.
            control.ensure_active()?;
            let cp_ok = CreateProcessAsUserW(
                restricted_token.as_raw(),
                app_name_w.as_ptr(),
                cmdline_w.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1, // bInheritHandles = TRUE; HANDLE_LIST attribute narrows the actual inheritance set
                creation_flags,
                env_block.as_ptr() as _,
                cwd_w.as_ref().map(|w| w.as_ptr()).unwrap_or(ptr::null()),
                &mut sinfo as *mut _ as _,
                &mut pi,
            );
            if cp_ok == 0 {
                let last_err = GetLastError();
                tracing::error!(
                    target: "wcore_sandbox",
                    last_err = format!("{last_err:#x}"),
                    "CreateProcessAsUserW failed"
                );
                return Err(SandboxError::ExecFailed(format!(
                    "CreateProcessAsUserW: {last_err:#x}"
                )));
            }
            tracing::debug!(target: "wcore_sandbox", pid = pi.dwProcessId, "CreateProcessAsUserW OK");
            let process = OwnedHandle::new(pi.hProcess);
            let thread = OwnedHandle::new(pi.hThread);

            // OS-layer invariant: the child MUST be running at Low
            // integrity. Querying the child's token directly from the
            // parent (which has full access to its own children's
            // tokens) — if the kernel didn't apply Low IL, the child
            // is silently running at a higher privilege level than
            // the sandbox contract claims, which is a security
            // regression. Bail loudly here so the bug surfaces in
            // logs + tests rather than at exploit time.
            let il_rid = query_process_integrity_rid(process.as_raw())?;
            tracing::debug!(
                target: "wcore_sandbox",
                il_rid = format!("{il_rid:#x}"),
                "child token integrity level"
            );
            if il_rid != SECURITY_MANDATORY_LOW_RID {
                TerminateProcess(process.as_raw(), 1);
                return Err(SandboxError::ExecFailed(format!(
                    "AppContainer child token integrity level is {il_rid:#x}; \
                 expected Low ({:#x}). Sandbox boundary failed at OS layer.",
                    SECURITY_MANDATORY_LOW_RID
                )));
            }

            // ---- 10. Assign to Job BEFORE resume ----
            if AssignProcessToJobObject(job.as_raw(), process.as_raw()) == 0 {
                TerminateProcess(process.as_raw(), 1);
                return Err(SandboxError::ExecFailed(format!(
                    "AssignProcessToJobObject: {:#x}",
                    GetLastError()
                )));
            }

            drop(stdout_w);
            drop(stderr_w);

            // ---- 11. Resume + wait ----
            if let Err(error) = control.resume_if_active(thread.as_raw()) {
                job.terminate();
                return Err(error);
            }

            // ---- 11a. Drain the pipes CONCURRENTLY with the child (#520). ----
            // The stdout/stderr pipe buffers are only ~4 KB. Draining them only
            // after the child exits (the pre-#520 behaviour) deadlocks any
            // command whose output exceeds that buffer: the child blocks in
            // WriteFile with a full pipe, never exits, `WaitForSingleObject`
            // times out, and the post-hoc drain returns only the truncated head.
            // Users saw this as blank output on small commands and 60s timeouts
            // on large ones (#453 / #500). Reader threads keep the pipes drained
            // so the child can always make progress. The `stdout_r` / `stderr_r`
            // OwnedHandles stay in this scope and outlive the joins below, so the
            // raw handles the threads hold are valid for the threads' whole life;
            // EOF (and thus thread exit) is reached once every write-end closes —
            // guaranteed by the `TerminateJobObject` reap below (#100).
            let stdout_h = stdout_r.as_raw() as usize;
            let stderr_h = stderr_r.as_raw() as usize;
            // `drain_pipe` is unsafe; the call is bare because this whole fn body
            // is one `unsafe` block and the closures inherit that context.
            let output_bytes = Arc::new(AtomicUsize::new(0));
            let stdout_output_bytes = Arc::clone(&output_bytes);
            let stdout_reader =
                std::thread::spawn(move || drain_pipe(stdout_h as _, stdout_output_bytes));
            let stderr_reader = std::thread::spawn(move || drain_pipe(stderr_h as _, output_bytes));

            let timeout_ms: u32 = match manifest.timeout {
                Some(d) => clamp_timeout_ms(d),
                None => 60_000,
            };

            let wait_res = WaitForSingleObject(process.as_raw(), timeout_ms);
            let timed_out = wait_res == WAIT_TIMEOUT;
            // A wait result other than OBJECT_0 / TIMEOUT is a hard error, but we
            // must NOT return before the reap + join below: the detached reader
            // threads hold raw read-handles owned by this scope's OwnedHandles,
            // so an early return would leak the threads and drop the handles out
            // from under them. Capture the failure and surface it after the join.
            // Snapshot GetLastError() now — TerminateJobObject clobbers it.
            let wait_err = if !timed_out && wait_res != WAIT_OBJECT_0 {
                Some((wait_res, GetLastError()))
            } else {
                None
            };

            // ---- 12. Exit code + drain ----
            // Capture the child's exit code BEFORE reaping the tree (only
            // meaningful on a clean exit; on timeout it is replaced by the
            // `Timeout` error below). As above, defer any error return past the
            // reap + join.
            let mut exit_code: u32 = 0;
            let exitcode_err = if !timed_out
                && wait_err.is_none()
                && GetExitCodeProcess(process.as_raw(), &mut exit_code) == 0
            {
                Some(GetLastError())
            } else {
                None
            };

            // Reap the ENTIRE job tree before joining the drain threads (#100).
            // The direct child can spawn helpers — most notably a console host
            // (`conhost.exe`) — that outlive it and keep the inherited
            // stdout/stderr write-ends open. A plain `TerminateProcess(child)`
            // leaves them running, so the reader threads would never reach EOF
            // and the joins below would hang far past the timeout (observed as a
            // 120s "command timed out" with no output on disconnected RDP
            // sessions). Terminating the job closes every member's handles so the
            // pipes EOF; bytes already written stay readable and the threads have
            // been draining them all along. The short wait lets the kernel finish
            // closing the handles before the threads see EOF.
            TerminateJobObject(job.as_raw(), if timed_out { 1 } else { exit_code });
            WaitForSingleObject(process.as_raw(), 2_000);

            // Now that every write-end is closed the reader threads reach EOF;
            // join them to collect the fully-drained output. This MUST run before
            // the deferred error returns so the threads never outlive their
            // handles.
            let (stdout, stdout_exceeded) = stdout_reader.join().unwrap_or_default();
            let (mut stderr, stderr_exceeded) = stderr_reader.join().unwrap_or_default();

            if let Some((wait_res, last_err)) = wait_err {
                return Err(SandboxError::ExecFailed(format!(
                    "WaitForSingleObject: {wait_res:#x} last_err={last_err:#x}"
                )));
            }
            if let Some(last_err) = exitcode_err {
                return Err(SandboxError::ExecFailed(format!(
                    "GetExitCodeProcess: {last_err:#x}"
                )));
            }

            // #324: a child that loads a DLL the Low-IL restricted-token
            // AppContainer cannot map (PowerShell's .NET/GAC, git-bash's
            // msys-2.0.dll, busybox-w32's Secur32/WS2_32/bcrypt/USER32) dies at
            // image initialization with NTSTATUS STATUS_DLL_NOT_FOUND and empty
            // output — which surfaces to the user as "the command did nothing."
            // Bare shells are rejected in `resolve_program`, but a caller can
            // still reach here by passing such a shell as an ABSOLUTE path, so
            // annotate the empty failure with an actionable diagnostic instead
            // of leaving it silent. Annotate stderr (not an Err) so the exit
            // code and any partial output are preserved for the caller.
            const STATUS_DLL_NOT_FOUND: i32 = 0xC000_0135u32 as i32;
            const STATUS_DLL_INIT_FAILED: i32 = 0xC000_0142u32 as i32;
            if matches!(
                exit_code as i32,
                STATUS_DLL_NOT_FOUND | STATUS_DLL_INIT_FAILED
            ) && stdout.is_empty()
                && stderr.is_empty()
            {
                let hint = format!(
                    "wcore-sandbox: the program exited at image initialization with \
                 {ec:#010x} (STATUS_DLL_NOT_FOUND / STATUS_DLL_INIT_FAILED) and no \
                 output. Under the Windows AppContainer sandbox's Low-integrity \
                 restricted token, executables that depend on DLLs outside the minimal \
                 System32 set (e.g. PowerShell's .NET/GAC assemblies, git-bash's \
                 msys-2.0.dll, or even static busybox-w32's network/auth/UI imports) \
                 cannot load. Use cmd as the sandbox shell, or run a sandbox-compatible \
                 executable.\n",
                    ec = exit_code,
                );
                stderr.extend_from_slice(hint.as_bytes());
            }

            tracing::debug!(
                target: "wcore_sandbox",
                exit_code = exit_code as i32,
                timed_out,
                stdout_bytes = stdout.len(),
                stderr_bytes = stderr.len(),
                "child exited"
            );

            if timed_out {
                return Err(SandboxError::Timeout);
            }
            if stdout_exceeded || stderr_exceeded {
                return Err(SandboxError::OutputLimitExceeded {
                    limit_bytes: super::super::super::BUFFERED_OUTPUT_LIMIT_BYTES,
                });
            }

            Ok(SandboxOutput {
                exit_code: exit_code as i32,
                stdout,
                stderr,
                resource_limits: ResourceLimitEnforcement::Enforced,
            })
        }
    })();

    let cleanup = identity
        .mark_process_exited()
        .and_then(|()| identity.cleanup());
    match (execution, cleanup) {
        (_, Err(cleanup_error)) => Err(cleanup_error),
        (result, Ok(())) => result,
    }
}
