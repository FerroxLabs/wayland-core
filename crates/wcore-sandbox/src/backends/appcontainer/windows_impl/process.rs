//! AppContainer backend spawn/execute pipeline (F20-03 Task 1A split of `windows_impl`).
#![allow(unused_imports)]

use super::super::super::SandboxBackend;
use super::super::appcontainer_acl_lease::ExecutionIdentity;
use super::super::{NEGATIVE_PROBE_TTL, ProbeCache};
use crate::directory_authority::DirectoryNameLease;
use crate::error::{Result, SandboxError};
use crate::manifest::{NetworkPolicy, SandboxManifest};
use crate::{DirectoryAuthority, ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
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
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
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
    WaitForMultipleObjects, WaitForSingleObject,
};

/// `SE_GROUP_INTEGRITY` from `winnt.h`. Not re-exported by windows-sys
/// (versions ≤ 0.59); defined locally per the Windows SDK header.
const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;

/// How long the post-`TerminateJobObject` drain waits for the job to reach zero
/// active processes. Measured teardown is 0-2 ms; this is a hang guard, not a
/// budget, and it is bounded so a wedged member cannot stall the caller.
const JOB_DRAIN_LIMIT_SECS: u64 = 5;

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
/// stays sticky for the process lifetime. A negative verdict also stays
/// readable, but throttles re-probing for [`NEGATIVE_PROBE_TTL`], after which
/// `is_available()` re-probes. This avoids both the "transient flake at startup
/// permanently disables sandboxing" silent-failure pattern and the
/// re-probe-every-command hang of #125.
pub(super) fn probe_cache() -> &'static Mutex<ProbeCache> {
    static CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ProbeCache::new()))
}

/// Test-only isolation lock for every test that reads or warms [`probe_cache`].
///
/// The cache is process-global and `cargo test` runs the whole lib suite in ONE
/// process, so a test that needs a cold cache cannot obtain one from the
/// scheduler: any sibling that reaches the availability gate warms it first
/// under `--test-threads=1`, and only misses it by luck under the default
/// parallel runner (FerroxLabs/wayland#1100). Every such test takes this lock
/// as its first statement, so the reset-then-read window cannot be interleaved.
///
/// EVIDENCE STATUS, so it is not re-derived. The reset in
/// `session_selection_reaches_ready_without_running_the_appcontainer_probe`
/// has a deterministic red arm (revert it and
/// `cargo test -p wcore-sandbox --lib -- --test-threads=1` fails on Windows;
/// that is what `scripts/regression-gate-1100.sh` runs in CI). The locks in the
/// cache-WARMING siblings do not. Stripping the lock from the one non-ignored
/// warmer and racing it against the protected test, 300 runs at
/// `--test-threads=16` on Windows 11 26200, produced 0 failures — the warmer's
/// write is behind a real `spawn_blocking` probe that takes milliseconds while
/// the read it would corrupt completes in microseconds, so libtest starting
/// both at once does not reach the window. They are kept because a
/// process-global write racing a process-global read is a real hazard and the
/// lock is free, NOT because a measurement demanded them. Do not cite them as
/// proven.
#[cfg(test)]
pub(super) fn probe_isolation() -> &'static Mutex<()> {
    static ISOLATION: OnceLock<Mutex<()>> = OnceLock::new();
    ISOLATION.get_or_init(|| Mutex::new(()))
}

/// Test-only: return the process-global probe cache to its cold state.
///
/// Callers must already hold [`probe_isolation`]. Poison is absorbed rather
/// than unwrapped so a single panicking test does not cascade into every
/// sibling that shares the cache.
#[cfg(test)]
pub(super) fn reset_probe_cache_for_test() {
    let mut cache = probe_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *cache = ProbeCache::new();
}

/// Single-flight gate for the availability probe (FerroxLabs/wayland#754).
///
/// The probe cache alone does NOT prevent a stampede: when the cache is
/// cold, N concurrent `is_available()` callers all miss it *before* any of
/// them records a verdict, so each launches its OWN real AppContainer spawn
/// at the same instant. On Windows those parallel spawns contend on the
/// shared per-PID AppContainer profile / profile-service RPC and most of
/// them FAIL — and every failure is written into the cache as
/// `Unavailable { retry_after: now + NEGATIVE_PROBE_TTL }`, so every tool
/// command is refused ("sandbox UNAVAILABLE … refusing to run") until a
/// re-probe succeeds. The agent
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

/// The settled probe verdict, read WITHOUT probing.
///
/// `None` means the probe has not run yet. This is the read used by session
/// selection and by the containment predicates, both of which run on the
/// readiness path and must never pay the 15s guarded spawn.
pub(super) fn settled_verdict() -> Option<bool> {
    probe_cache()
        .lock()
        .expect("probe cache poisoned")
        .settled()
}

/// Single-flighted availability, BLOCKING for up to the probe's wall-clock
/// guard on a cold cache.
///
/// A free function rather than only a trait method so the async `execute` gate
/// can hand it to `spawn_blocking` — a 15s guarded spawn must never run on a
/// tokio worker thread.
fn availability() -> bool {
    super::super::probe_single_flight(
        probe_cache(),
        probe_gate(),
        NEGATIVE_PROBE_TTL,
        probe_honouring_shared_verdict,
    )
}

/// The real probe, skipped when another process on this host already proved the
/// answer recently.
///
/// The in-process cache and gate above collapse probes within ONE process, but
/// every sandboxed child is its own process and under `cargo nextest` so is
/// every test, so without this a single run re-answers the same question about
/// the host hundreds of times. Each repeat both queues on the serialized AppX
/// profile service and takes another draw at the AV-stall failure mode
/// documented in `probe_appcontainer_once`, which is how a 15s guard gets
/// tripped on a healthy machine.
///
/// Only SUCCESS is shared. See [`shared_verdict`] for why caching a negative
/// verdict across processes would be unsafe in a way caching a positive one is
/// not.
fn probe_honouring_shared_verdict() -> bool {
    if super::shared_verdict::cached_success() {
        return true;
    }
    let available = probe_appcontainer_available();
    if available {
        super::shared_verdict::record_success();
    }
    available
}

/// Whether a backend admitted WITHOUT a startup probe may still claim its
/// containment properties, given only what the probe has settled.
///
/// Pure in the verdict so both arms are reachable from a test on any host.
/// An "unknown" verdict is NOT a negative — the claim is withdrawn only on
/// evidence, because a session that has not yet run a command has not yet
/// learned anything about this host.
pub(super) fn containment_claim(settled: Option<bool>) -> bool {
    settled != Some(false)
}

/// True once a probe has actually settled UNAVAILABLE.
///
/// Monotone by construction: [`ProbeCache::settled`] keeps a negative verdict
/// until a probe succeeds, so a predicate built on this answers the same way
/// for as long as the host stays broken.
fn containment_withdrawn() -> bool {
    !containment_claim(settled_verdict())
}

/// Why the most recent AppContainer probe failed, verbatim, for the operator.
///
/// The probe's failure detail used to exist ONLY as a `tracing::error!`, and
/// the refusal told the reader "the cause was logged". Under CI — and under
/// any host that does not install a subscriber at that level — no such line is
/// ever emitted, so the product asserted a cause existed and then declined to
/// show it. Both Windows runners refuse to sandbox and nobody could say why.
///
/// This is the diagnosis path, not decoration: the recorded string is the
/// failing Win32 call and its status code (`CreateProcessAsUserW: 0x…`), which
/// is what distinguishes a policy/environment refusal from a product defect.
/// The tracing calls are kept — this is additive, for readers who DO have a
/// subscriber.
fn last_probe_failure() -> &'static Mutex<Option<String>> {
    static LAST: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

/// Record why a probe failed, or clear the record on success.
///
/// Cleared on success so a host that recovers (the negative TTL expires and the
/// re-probe passes) cannot keep quoting a stale cause at the operator.
pub(super) fn record_probe_outcome(failure: Option<String>) {
    if let Ok(mut slot) = last_probe_failure().lock() {
        *slot = failure;
    }
}

/// Compose the refusal text from the recorded cause.
///
/// Pure, and separated from [`unavailable_refusal`] so both arms — cause known
/// and cause genuinely unrecorded — are assertable without needing a host whose
/// AppContainer is broken.
pub(super) fn compose_unavailable_refusal(cause: Option<&str>) -> String {
    let mut s = String::from(
        "sandbox UNAVAILABLE and unsandboxed execution is not permitted — \
         refusing to run with host permissions. The AppContainer real-spawn \
         probe failed on this host. ",
    );
    match cause {
        Some(cause) => {
            s.push_str("Cause, verbatim from the probe: ");
            s.push_str(cause);
            s.push_str(
                ". A status code on a named Win32 call is the thing to search \
                 for — it distinguishes host policy (AppContainer disabled, \
                 profile-service RPC refused, the service account cannot create \
                 a profile) from a defect in this backend. ",
            );
        }
        None => {
            s.push_str(
                "No cause was recorded, which means the refusal was reached \
                 without a probe result — report this, it is a defect in the \
                 probe's own bookkeeping and not a fact about the host. ",
            );
        }
    }
    s.push_str("Set WAYLAND_ALLOW_NO_SANDBOX=1 only to accept running with NO isolation.");
    s
}

/// The refusal a deferred-selection backend returns when its probe has settled
/// unavailable. Deliberately the same sentence `FailClosedBackend` uses, so
/// deferring selection does not change what the operator is told or what they
/// are told to do about it.
fn unavailable_refusal() -> SandboxError {
    let cause = last_probe_failure()
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    SandboxError::ExecFailed(compose_unavailable_refusal(cause.as_deref()))
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
        !containment_withdrawn()
    }

    /// The AppContainer probe is a real guarded `cmd.exe /c exit 0` through the
    /// whole pipeline (15s wall-clock guard), so it is NOT safe to run on the
    /// session-startup path — bootstrap resolves the session backend before the
    /// `--json-stream` `ready` frame, and the host's ready deadline is shorter
    /// than the guard. Session selection therefore takes this backend
    /// structurally and [`Self::execute`] enforces the verdict instead.
    fn availability_probe_is_startup_safe(&self) -> bool {
        false
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
        availability()
    }

    fn enforces_read_deny(&self) -> bool {
        !containment_withdrawn()
    }

    /// The Low-integrity restricted token plus the per-root DACL grants confine
    /// the child to the manifest's filesystem grants. Withdrawn on the same
    /// predicate as the sibling claims above: a backend this host has disproved
    /// must not keep advertising confinement it cannot apply.
    fn confines_filesystem(&self) -> bool {
        !containment_withdrawn()
    }

    /// True because [`Self::execute_with_cwd_authority`] establishes an
    /// OS-ENFORCED PIN on the retained directory's name before the pathname is
    /// used, and holds it for the whole execution — see [`bind_retained_cwd`].
    /// It is NOT true because a pathname is re-resolved; if the pin cannot be
    /// established the execution is refused rather than spawned unbound.
    ///
    /// `binds_workspace_authority` is deliberately NOT overridden: the trait
    /// derives it from this predicate, and a second independent answer is how
    /// the two drift apart.
    ///
    /// Withdrawn once the probe settles unavailable — a backend that cannot
    /// spawn binds nothing, and delegated mutation must refuse rather than
    /// believe a claim this host has disproved.
    fn binds_cwd_authority(&self) -> bool {
        !containment_withdrawn()
    }

    async fn execute_with_cwd_authority(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        cwd: DirectoryAuthority,
    ) -> Result<SandboxOutput> {
        if let Some(declared) = cmd.cwd.as_deref()
            && declared != cwd.display_path()
        {
            return Err(SandboxError::PathDenied(
                "sandbox command cwd does not match the retained cwd authority".to_owned(),
            ));
        }
        // The lease is acquired BEFORE the pathname reaches CreateProcess and
        // is held across the whole execution, so there is no window in which the
        // bound name can be redirected.
        let (lease, bound) = bind_retained_cwd(&cwd)?;
        let bound_cmd = SandboxCommand {
            argv: cmd.argv,
            cwd: Some(bound),
        };
        let output = self.execute(manifest, bound_cmd).await;
        drop(lease);
        output
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        // Static policy refusals first: they are decided from the manifest
        // alone, need no host, and must keep their exact error so a caller can
        // tell "this policy is unsupported" from "this host cannot sandbox".
        if matches!(manifest.network, NetworkPolicy::AllowHosts(_)) {
            return Err(SandboxError::PolicyNotSupported(
                "AppContainer has no DNS-name allowlist; use NetworkPolicy::Deny + WFP filter (v0.7.0)".into(),
            ));
        }
        // THE availability gate for this backend. Session selection takes
        // AppContainer without probing (the probe does not fit inside the
        // host's ready deadline), so the verdict is enforced HERE, at the first
        // command that would actually reach Win32, instead of at selection.
        //
        // This is the single funnel and it must stay one: `execute_streaming`
        // (trait default) and `execute_with_cwd_authority` (above) both route
        // through this method, and `execute_with_workspace_authority` is
        // additionally gated by `binds_workspace_authority`, which withdraws on
        // the same verdict. `is_available` is single-flighted and its positive
        // result is sticky, so the guarded probe is paid at most once per
        // retry window — never per command, and never before `ready`.
        //
        // On `spawn_blocking` because a cold probe blocks for up to its 15s
        // wall-clock guard, and a guarded Win32 spawn must never occupy a
        // tokio worker thread. A join failure is treated as unavailable —
        // fail closed, never "assume it worked".
        if !tokio::task::spawn_blocking(availability)
            .await
            .unwrap_or(false)
        {
            return Err(unavailable_refusal());
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

/// Bind a child's working directory to a RETAINED directory object on a
/// platform whose process-creation API accepts only a pathname.
///
/// The order of the three steps below is the guarantee, and it is not
/// rearrangeable:
///
/// 1. **PIN THE NAME FIRST.** [`DirectoryAuthority::acquire_name_lease`] opens
///    the retained object handle-relatively — no pathname is resolved — with a
///    share mode that omits `FILE_SHARE_DELETE`. While that handle lives the
///    kernel refuses every rename and every unlink of the pinned name. Measured
///    on SEANDESKTOP against the retained observational checkout authority the
///    delegated dispatch path produces; see `windows::acquire_name_lease`.
/// 2. **PROVE THE PIN LANDED ON THE RIGHT NAME.** The lease pins whatever name
///    the object currently carries, so if the object had ALREADY been renamed
///    away from its display path, the display path would now name a decoy.
///    `validate_path` re-proves that the display path still resolves to exactly
///    the retained object, AFTER the pin exists.
/// 3. **ONLY THEN BIND BY PATH.** Everything after step 2 is inside the pin, so
///    there is no residual window: a substitution cannot be performed between
///    the proof and the child's first filesystem operation, because the OS
///    refuses it for as long as the returned lease is held.
///
/// FAILS CLOSED. Any failure returns an error; there is no unbound-spawn
/// fallback. In particular a delete-bearing authority (opened through
/// `DirectoryAuthority::open` rather than `open_observational`) cannot be
/// pinned, and is refused rather than silently bound to a re-resolvable path.
///
/// This function is the single place the binding is established. A downgrade to
/// an unguarded pathname re-resolve means deleting it, which is what the
/// anti-swap regression test in this module's `tests` is written to catch.
pub(super) fn bind_retained_cwd(
    cwd: &DirectoryAuthority,
) -> Result<(DirectoryNameLease, std::path::PathBuf)> {
    let bound = cwd.display_path().to_path_buf();
    let lease = cwd.acquire_name_lease().map_err(|error| {
        SandboxError::PathDenied(format!(
            "AppContainer could not pin the retained working-directory name {}: {error}",
            bound.display()
        ))
    })?;
    cwd.validate_path(&bound)?;
    Ok((lease, bound))
}

/// Outcome of ONE real-spawn attempt.
///
/// `retryable` is the whole point of this type. A probe that FAILED FAST (a
/// Win32 error, a non-zero child exit) is very likely losing a race with
/// another process creating an AppContainer profile at the same instant, and
/// re-attempting costs milliseconds. A probe that STALLED past the wall-clock
/// guard is the opposite: something is wedged, and re-attempting multiplies the
/// #125 hang instead of curing it. Never retry a stall.
pub(super) enum ProbeAttempt {
    Available,
    Failed { reason: String, retryable: bool },
}

/// Availability, with bounded retry for transient contention.
///
/// WHY THIS EXISTS. Every self-hosted Windows CI failure on this branch was one
/// sentence — "sandbox UNAVAILABLE … the AppContainer real-spawn probe failed
/// on this host" — and it was NOT a host defect: the same box probes
/// `available` as an interactive user, as `NT AUTHORITY\NetworkService`, from
/// `C:`, and across 40 concurrent separate-process cold probes. What it does
/// not survive is the FULL workspace test load, where many independent
/// processes each create an AppContainer profile at once. CI's own retries then
/// pass, which is the signature of a transient race, and is exactly why the
/// failure set appeared to "churn between runs".
///
/// The single-flight added in #754 cannot help here: `probe_cache` and
/// `probe_gate` are `OnceLock` statics, so they collapse concurrent probes
/// WITHIN a process, and every sandboxed child is its own process.
///
/// Fail-closed is preserved exactly. After the attempts are spent this still
/// returns `false` and the caller still refuses to run unsandboxed. The change
/// is only that one unlucky millisecond no longer hard-refuses a user's
/// command.
pub(super) fn probe_appcontainer_available() -> bool {
    probe_with_retry(probe_appcontainer_once, std::thread::sleep)
}

/// Deliberately small. Contention resolves in milliseconds; anything larger
/// just delays an honest refusal.
pub(super) const PROBE_ATTEMPTS: u32 = 3;
const BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(750)];

/// The retry policy, separated from the Win32 spawn so it is provable.
///
/// Taking the attempt and the sleep as parameters is the whole reason this is a
/// function: the real prober needs a real AppContainer, and a test that needs a
/// real AppContainer cannot assert "a stall is NOT retried" — the interesting
/// case is the one you cannot conjure on a healthy host.
pub(super) fn probe_with_retry(
    mut attempt: impl FnMut() -> ProbeAttempt,
    mut sleep: impl FnMut(Duration),
) -> bool {
    let mut last: Option<String> = None;
    for n in 1..=PROBE_ATTEMPTS {
        match attempt() {
            ProbeAttempt::Available => {
                record_probe_outcome(None);
                return true;
            }
            ProbeAttempt::Failed { reason, retryable } => {
                if !retryable {
                    record_probe_outcome(Some(reason));
                    return false;
                }
                if n == PROBE_ATTEMPTS {
                    record_probe_outcome(Some(format!(
                        "{reason} (still failing after {PROBE_ATTEMPTS} attempts, so this is \
                         not a transient race)"
                    )));
                    return false;
                }
                tracing::warn!(
                    target: "wcore_sandbox",
                    attempt = n,
                    reason = %reason,
                    "AppContainer probe failed; retrying, this is usually contention with \
                     another process creating a profile."
                );
                last = Some(reason);
                sleep(BACKOFF[(n - 1) as usize]);
            }
        }
    }
    // Unreachable in practice: the loop returns on every path. Kept fail-closed
    // rather than `unreachable!()` so a future edit cannot turn a logic slip
    // into a panic that the caller reads as a sandbox bypass.
    record_probe_outcome(last);
    false
}

fn probe_appcontainer_once() -> ProbeAttempt {
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
        // Thread creation failing is a resource ceiling, not a profile race.
        return ProbeAttempt::Failed {
            reason: "could not spawn the `appcontainer-probe` thread at all (OS refused \
                     thread creation); the AppContainer pipeline was never reached"
                .to_owned(),
            retryable: false,
        };
    }

    match rx.recv_timeout(PROBE_WALL_CLOCK) {
        Ok(Ok(out)) if out.exit_code == 0 => ProbeAttempt::Available,
        Ok(Ok(out)) => {
            tracing::error!(
                target: "wcore_sandbox",
                exit_code = out.exit_code,
                "AppContainer real-spawn probe completed but exit code non-zero; \
                 sandbox disabled. WAYLAND_SANDBOX_LIVE_WINDOWS spawn may also fail."
            );
            ProbeAttempt::Failed {
                reason: format!(
                    "the probe spawned successfully but `cmd.exe /c exit 0` returned \
                     exit code {} instead of 0 — the sandbox pipeline works and the \
                     child itself misbehaved",
                    out.exit_code
                ),
                // The pipeline demonstrably works; a different exit code is a real
                // answer, not a race to re-run.
                retryable: false,
            }
        }
        Ok(Err(e)) => {
            let reason = format!("{e}");
            tracing::error!(
                target: "wcore_sandbox",
                error = %e,
                "AppContainer real-spawn probe failed; sandbox disabled. This is NOT a \
                 statement that Windows cannot sandbox: the cause is the `error` field \
                 on this line, verbatim, and it names the call or file that failed. \
                 Read it first. A stale ACL lease is no longer a cause — an \
                 unreconcilable lease whose owner is gone is now reclaimed to the \
                 quarantine directory and reported by name. Treat this as transient (AV, \
                 disk contention), and wait for the probe to re-run after the \
                 negative-cache TTL, ONLY when the error names no persistent on-disk \
                 state."
            );
            // A fast Win32 failure is the shape a lost profile-creation race
            // takes, so this is the one arm worth re-attempting.
            ProbeAttempt::Failed {
                reason,
                retryable: true,
            }
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
            ProbeAttempt::Failed {
                reason: format!(
                    "the probe exceeded its {}s hard wall-clock guard — a Win32 setup \
                     call (CreateAppContainerProfile / CreateProcessAsUserW) stalled \
                     rather than returning an error, most often an AV image scan or a \
                     slow profile-service RPC",
                    PROBE_WALL_CLOCK.as_secs()
                ),
                // NEVER retry a stall. #125 was a ~120s hang per command; three
                // stacked 15s guards would triple it and cure nothing.
                retryable: false,
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            control.cancel();
            tracing::error!(
                target: "wcore_sandbox",
                "AppContainer probe thread ended without a result; sandbox disabled."
            );
            ProbeAttempt::Failed {
                reason: "the `appcontainer-probe` thread ended without sending a result — \
                         it panicked inside the Win32 FFI path; the panic message is on \
                         stderr, not here"
                    .to_owned(),
                // A panic in the FFI path reproduces; retrying just panics again.
                retryable: false,
            }
        }
    }
}

/// Build the exact UTF-16 buffer handed to `CreateProcessAsUserW`'s
/// `lpCurrentDirectory` (or `None` for a NULL, meaning "inherit the parent's").
///
/// Extracted from [`execute_blocking`] so the value that actually reaches Win32
/// is directly assertable. It is the whole of the cwd contract:
///
/// 1. a relative cwd is rejected — the child must never silently land somewhere
///    resolved against the PARENT's current directory;
/// 2. a verbatim-disk cwd is de-prefixed, because `cmd.exe` treats a leading
///    `\\` as UNC, refuses it as a current directory, and silently substitutes
///    `C:\Windows` — see [`strip_verbatim_disk_prefix`] for why this does not
///    widen the sandbox.
pub(super) fn resolve_cwd(cwd: Option<&std::path::Path>) -> Result<Option<Vec<u16>>> {
    let Some(p) = cwd else {
        return Ok(None);
    };
    if !p.is_absolute() {
        return Err(SandboxError::ExecFailed(format!(
            "cwd {p:?} must be absolute"
        )));
    }
    Ok(Some(widen_os(&strip_verbatim_disk_prefix(p))))
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
    // A `cmd /c` payload with a line break is undeliverable through ANY
    // Windows command line, so refuse it here for the same reason the relaxed
    // Job Object path does: cmd would run only the prefix and hand back its
    // exit status, reporting success for work that never happened.
    if let Some(idx) = crate::backends::windows_cmdline::cmd_payload_index(&cmd.argv) {
        crate::backends::windows_cmdline::reject_undeliverable_cmd_payload(&cmd.argv[idx])?;
    }

    let cwd_w: Option<Vec<u16>> = resolve_cwd(cmd.cwd.as_deref())?;

    let app_name_w = resolve_program(&cmd.argv[0])?;
    let mut identity = ExecutionIdentity::start(manifest)?;
    let sid_ptr = identity.sid();
    let package_root = identity.package_root();

    let execution = (|| -> Result<SandboxOutput> {
        unsafe {
            // ---- 2. Restricted token ----
            //
            // No deny-only SIDs. An earlier revision marked
            // BUILTIN\Administrators, BUILTIN\Users, and Authenticated Users
            // as "for deny only" (SidsToDisable), mirroring the Chromium /
            // sandboxie primary-token pattern. On a real AppContainer that
            // marking is REDUNDANT and actively harmful: containment is
            // intrinsic to the AppContainer package-SID access model, which
            // ignores normal SIDs (Everyone/Users/Authenticated Users) for
            // *granting* — a file granted only to those SIDs is still denied
            // to the child. So isolation does not depend on the deny-only
            // marking. Meanwhile that marking broke the package-SID grant
            // path: it left the child with no usable enabled SID for any
            // file's DACL, so a sandboxed process could read no file at all —
            // not even an AppContainer-granted one. The 2026-07-23 hardware
            // matrix confirmed identical reads exit 0 with the marking OFF and
            // exit 1 with it ON, while a normal-SID-only grant stayed denied
            // either way. Passing 0/null for SidsToDisable therefore restores
            // reads without weakening the sandbox; the child token remains
            // restricted, low-integrity, and AppContainer-tagged.
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
                0,
                ptr::null_mut(),
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
            // These flags are NOT what makes a user32-linked child fail image
            // initialization with 0xC0000142. Measured on SeanDesktop
            // 2026-08-10 by A/B'ing the mask over one run: 0xff (this set),
            // 0x00 (no UI restrictions at all), 0xfe (no HANDLES), 0xbf (no
            // DESKTOP), 0xdf (no GLOBALATOMS), 0xbe and 0x9e all produced the
            // SAME verdict — `where.exe` dead, `cmd`/`hostname`/`attrib`/`find`
            // alive. The cause is the parent's window station; see
            // `crates/wcore-tools/tests/win_toolchain_launch.rs`.
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
            //
            // `lpDesktop` names the engine's OWN window station and desktop.
            // Leaving it NULL makes the child inherit the engine's, and a
            // non-interactive station (`Service-0x1-…$` under OpenSSH, a
            // service, or a scheduled task) carries no ALL APPLICATION
            // PACKAGES ACE — so USER32's process-attach cannot open it and
            // every USER32-linked image dies at load with 0xC0000142
            // STATUS_DLL_INIT_FAILED. See `window_station` for the measured
            // descriptors and for why a private station is TIGHTER than
            // inheriting the interactive `WinSta0`. Declared before `sinfo`
            // so the buffer outlives the `CreateProcessAsUserW` that reads it.
            let mut desktop_w: Option<Vec<u16>> =
                super::window_station::sandbox_desktop().map(<[u16]>::to_vec);
            let mut sinfo: STARTUPINFOEXW = mem::zeroed();
            sinfo.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
            sinfo.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            sinfo.StartupInfo.lpDesktop = desktop_w
                .as_mut()
                .map(|d| d.as_mut_ptr())
                .unwrap_or(ptr::null_mut());
            sinfo.StartupInfo.hStdInput = std::ptr::null_mut();
            sinfo.StartupInfo.hStdOutput = stdout_w.as_raw();
            sinfo.StartupInfo.hStdError = stderr_w.as_raw();
            sinfo.lpAttributeList = attr_list;

            // ---- 8. Command line + env block ----
            // When the resolved program is cmd.exe, its `/c`/`/k` payload must be
            // quoted for cmd's RAW-command-line re-read (single outer pair, inner
            // quotes verbatim), NOT with the MSVC CRT `\"` escaping quote_arg
            // emits — cmd /s strips only the outer pair and would otherwise run
            // the backslash-escaped quotes literally (`type \"path\"` ->
            // ERROR_INVALID_NAME). Every other argv entry keeps the CRT quoting.
            // Quoting-layer only: the token / Job / ACL boundary is unchanged.
            let cmd_payload_idx = if resolved_program_is_cmd(&app_name_w) {
                cmd.argv
                    .iter()
                    .position(|a| {
                        let flag = a.to_ascii_lowercase();
                        flag == "/c" || flag == "/k"
                    })
                    .map(|flag_idx| flag_idx + 1)
            } else {
                None
            };
            let cmdline: String = cmd
                .argv
                .iter()
                .enumerate()
                .map(|(idx, a)| {
                    if Some(idx) == cmd_payload_idx {
                        quote_cmd_payload(a)
                    } else {
                        quote_arg(a)
                    }
                })
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
            // Wakeup channel for the ceiling crossing. Owned here so it
            // outlives both reader threads (they are joined below).
            let exceeded_event = create_output_exceeded_event();
            let exceeded_event_raw = exceeded_event
                .as_ref()
                .map(|e| e.as_raw() as usize)
                .unwrap_or(0);
            let stdout_reader = std::thread::spawn(move || {
                drain_pipe(stdout_h as _, stdout_output_bytes, exceeded_event_raw as _)
            });
            let stderr_reader = std::thread::spawn(move || {
                drain_pipe(stderr_h as _, output_bytes, exceeded_event_raw as _)
            });

            let timeout_ms: u32 = match manifest.timeout {
                Some(d) => clamp_timeout_ms(d),
                None => 60_000,
            };

            // Wait for EITHER the child to exit or the output ceiling to be
            // crossed. Waiting only on the process (the pre-fix behaviour) let
            // an offender that floods and then sleeps run out the full timeout:
            // host memory was bounded because `drain_pipe` discards the excess,
            // but the process was never killed and the failure was reported as
            // a Timeout. Linux kills in milliseconds; this closes the gap.
            const EXCEEDED_WAIT_INDEX: u32 = WAIT_OBJECT_0 + 1;
            let wait_res = if let Some(event) = exceeded_event.as_ref() {
                let waits = [process.as_raw(), event.as_raw()];
                WaitForMultipleObjects(waits.len() as u32, waits.as_ptr(), 0, timeout_ms)
            } else {
                WaitForSingleObject(process.as_raw(), timeout_ms)
            };
            let timed_out = wait_res == WAIT_TIMEOUT;
            let output_exceeded_wakeup = wait_res == EXCEEDED_WAIT_INDEX;
            // A wait result other than OBJECT_0 / OBJECT_0+1 / TIMEOUT is a hard
            // error, but we must NOT return before the reap + join below: the
            // detached reader threads hold raw read-handles owned by this
            // scope's OwnedHandles, so an early return would leak the threads
            // and drop the handles out from under them. Capture the failure and
            // surface it after the join. Snapshot GetLastError() now —
            // TerminateJobObject clobbers it.
            let wait_err = if !timed_out && !output_exceeded_wakeup && wait_res != WAIT_OBJECT_0 {
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
                && !output_exceeded_wakeup
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
            TerminateJobObject(
                job.as_raw(),
                if timed_out || output_exceeded_wakeup {
                    1
                } else {
                    exit_code
                },
            );
            WaitForSingleObject(process.as_raw(), 2_000);

            // ---- 12a. Drain the job to ZERO active processes. ----
            //
            // `TerminateJobObject` only REQUESTS termination of every member; it
            // does not wait, and the wait above covers the DIRECT CHILD ONLY. Any
            // other member — a grandchild, a console host — can still be alive
            // when this returns, and on Windows that is not cosmetic: a live
            // process whose current directory is the worker checkout holds a
            // handle whose share mode omits `FILE_SHARE_DELETE`, so every
            // delete-bearing open of that directory is refused by the kernel with
            // ERROR_SHARING_VIOLATION for as long as it lives. The swarm's
            // terminal-release path then reports the refusal as "worker
            // descendant still holds the retained checkout descriptor" and
            // quarantines a transaction whose worker actually succeeded.
            //
            // MEASURED (Windows 10.0.26200, NTFS): with a live process whose cwd
            // is the directory, the delete-bearing open fails with win32 = 32 for
            // as long as it lives; once it has been waited for, the same open
            // succeeds within 0-2 ms. So the only thing needed here is to WAIT.
            // Breakaway is denied on this job (see the LimitFlags block above),
            // so job membership is the complete descendant set.
            //
            // Bounded, so a member wedged in the kernel cannot hang the caller.
            {
                let drain_deadline =
                    std::time::Instant::now() + Duration::from_secs(JOB_DRAIN_LIMIT_SECS);
                loop {
                    let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = mem::zeroed();
                    let queried = QueryInformationJobObject(
                        job.as_raw(),
                        JobObjectBasicAccountingInformation,
                        ptr::addr_of_mut!(accounting).cast(),
                        mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                        ptr::null_mut(),
                    );
                    if queried == 0 || accounting.ActiveProcesses == 0 {
                        break;
                    }
                    if std::time::Instant::now() >= drain_deadline {
                        tracing::warn!(
                            target: "wcore_sandbox",
                            active_processes = accounting.ActiveProcesses,
                            drain_limit_secs = JOB_DRAIN_LIMIT_SECS,
                            "terminated job still has active members after its drain bound; \
                             a descendant may still hold the workspace directory"
                        );
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }

            // Now that every write-end is closed the reader threads reach EOF;
            // join them to collect the fully-drained output. This MUST run before
            // the deferred error returns so the threads never outlive their
            // handles.
            let (mut stdout, stdout_exceeded) = stdout_reader.join().unwrap_or_default();
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

            // Output exhaustion is checked BEFORE the timeout. The abuse case
            // this exists for — flood the pipe, then sit there — trips both
            // conditions, and the timeout used to win, making
            // `OutputLimitExceeded` unreachable for exactly the input it was
            // written to describe. Exhaustion is the cause; the timeout (when
            // it still happens at all) is a consequence.
            // wayland#1082. `drain_pipe` already does the right thing: it
            // reserves against the shared budget, KEEPS the partial grant, and
            // signals `exceeded_event` so the waiter tears the job down. This
            // was the one step that undid all of it — turning that retained
            // head into an error and handing the caller none of the command's
            // own output. A command that produced megabytes got back a hundred
            // bytes of error text. #1071 fixed the same inversion for the
            // shared drain; this is the AppContainer backend's copy of it.
            //
            // Marker placement follows the PER-STREAM flags. `exceeded_event`
            // is only ever set by `drain_pipe` on a stream that actually
            // crossed the ceiling, so a wakeup with both flags clear cannot
            // mean "some stream overflowed" — it means a reader thread died and
            // the `unwrap_or_default()` above turned that into "no output, no
            // overflow". That is worth saying out loud rather than guessing
            // which pipe flooded and attaching a marker to the wrong one.
            let exceeded_any = output_exceeded_wakeup || stdout_exceeded || stderr_exceeded;
            if stdout_exceeded {
                let kept = stdout.len();
                stdout.extend_from_slice(&super::super::super::truncation_marker(kept));
            }
            if stderr_exceeded {
                let kept = stderr.len();
                stderr.extend_from_slice(&super::super::super::truncation_marker(kept));
            }
            if output_exceeded_wakeup && !stdout_exceeded && !stderr_exceeded {
                stderr.extend_from_slice(
                    b"\n[wcore-sandbox: the output ceiling was crossed but neither reader \
                      reported it, which means a reader thread failed and its output was \
                      lost. Treat this result as INCOMPLETE.]\n",
                );
            }
            // Exhaustion still takes precedence over the timeout, and that
            // ordering is load-bearing: the abuse shape this exists for — flood
            // the pipe, then sit there — trips both, and when the timeout won,
            // the overflow was reported as a Timeout instead. Now that crossing
            // the cap yields OUTPUT rather than an error, the timeout must not
            // fire on top of it and throw that output away again.
            if timed_out && !exceeded_any {
                return Err(SandboxError::Timeout);
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
    join_execution_and_cleanup(execution, cleanup)
}

/// Marker every post-execution cleanup fault carries into the caller's stderr.
///
/// It exists so the *reader* of a tool result — a human or an agent — can tell
/// "your command did not run" apart from "your command ran, its effects have
/// already happened, and only the sandbox's own teardown failed". The
/// distinction is the whole point: an agent told a non-idempotent command
/// failed will re-issue it, and the second run is real damage.
pub(super) const CLEANUP_FAULT_PREFIX: &str = "wcore-sandbox: the command RAN TO COMPLETION and \
     its effects have already happened; only post-execution sandbox cleanup failed, so do NOT \
     retry the command. Cleanup fault: ";

/// Combine the execution outcome with the post-execution ACL/profile teardown
/// outcome.
///
/// A teardown fault is NOT an execution failure. Before this existed the join
/// was `(_, Err(cleanup_error)) => Err(cleanup_error)`, which threw away a
/// successful `SandboxOutput` — exit code, stdout and stderr — and reported the
/// teardown fault as though the command had never run. Under machine-wide ACL
/// mutation-lock contention that is exactly what happened: the child had
/// already written its files and the caller was told "Failed to execute
/// command".
///
/// The rules:
/// * teardown clean — pass the execution result through untouched;
/// * teardown faulted, command succeeded — return the REAL exit code and
///   output, with the fault appended to stderr behind [`CLEANUP_FAULT_PREFIX`];
/// * teardown faulted, command also failed — return the EXECUTION error, whose
///   typed variant (`Timeout`, `OutputLimitExceeded`, …) callers match on. The
///   teardown fault is logged rather than substituted, because the execution
///   failure is the cause and the teardown fault is a consequence of it.
///
/// Either way the fault is logged at `error` level, so a stranded lease is
/// never silent just because the command it belonged to succeeded.
pub(super) fn join_execution_and_cleanup(
    execution: Result<SandboxOutput>,
    cleanup: Result<()>,
) -> Result<SandboxOutput> {
    let Err(cleanup_error) = cleanup else {
        return execution;
    };
    tracing::error!(
        target: "wcore_sandbox",
        error = %cleanup_error,
        command_succeeded = execution.is_ok(),
        "AppContainer post-execution cleanup failed; the command's own outcome is unaffected"
    );
    match execution {
        Ok(mut output) => {
            output
                .stderr
                .extend_from_slice(format!("\n{CLEANUP_FAULT_PREFIX}{cleanup_error}\n").as_bytes());
            Ok(output)
        }
        Err(execution_error) => Err(execution_error),
    }
}
