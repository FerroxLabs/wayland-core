//! Windows AppContainer + Job Objects backend.
//!
//! Tier 0 default on Windows per cross-platform strategy. Builds a per-engine
//! AppContainer profile, derives a restricted token from the current process
//! (with `BUILTIN\Administrators` / `BUILTIN\Users` / `Authenticated Users`
//! SIDs explicitly disabled so an elevated parent doesn't grant the child
//! group-membership-based access), pins the child's integrity level to Low
//! via an explicit `SetTokenInformation` call, places the child in a Job
//! Object with memory/CPU/active-process/breakaway/priority caps AND a UI
//! restrictions set (no clipboard, no desktop, no inheriting USER handles,
//! no shutdown). Image load goes through `CreateProcessAsUserW` with
//! `STARTUPINFOEXW` carrying `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`
//! and a `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` scoped to exactly the two
//! stdout/stderr write ends — every other inheritable handle in the parent
//! process is excluded.
//!
//! Pipeline:
//!   1. CreateAppContainerProfile (or DeriveAppContainerSidFromAppContainerName
//!      if profile already exists). Profile is NEVER deleted by this backend —
//!      it's reused across spawns within the same process, and persists across
//!      restarts (idempotent on `ERROR_ALREADY_EXISTS`).
//!   2. OpenProcessToken + CreateRestrictedToken(DISABLE_MAX_PRIVILEGE,
//!      SidsToDisable=[Administrators, Users, Authenticated Users]).
//!   3. SetTokenInformation(TokenIntegrityLevel, S-1-16-4096 Low).
//!   4. CreateJobObjectW + SetInformationJobObject:
//!        - Extended limits: KILL_ON_JOB_CLOSE, ACTIVE_PROCESS=512,
//!          DIE_ON_UNHANDLED_EXCEPTION, PRIORITY_CLASS=BELOW_NORMAL,
//!          (BREAKAWAY_OK=0 / SILENT_BREAKAWAY_OK=0 default), plus
//!          PROCESS_MEMORY / PROCESS_TIME from manifest if set.
//!        - Basic UI restrictions: HANDLES, READCLIPBOARD, WRITECLIPBOARD,
//!          SYSTEMPARAMETERS, DISPLAYSETTINGS, GLOBALATOMS, DESKTOP,
//!          EXITWINDOWS.
//!   5. CreatePipe x2 (stdout + stderr) with inheritable write ends.
//!   6. Build STARTUPINFOEXW attribute list with SECURITY_CAPABILITIES and
//!      HANDLE_LIST=[stdout_w, stderr_w].
//!   7. CreateProcessAsUserW with CREATE_SUSPENDED + EXTENDED_STARTUPINFO_PRESENT.
//!   8. AssignProcessToJobObject (BEFORE ResumeThread).
//!   9. ResumeThread.
//!  10. WaitForSingleObject with manifest timeout (defaults to 60s if None).
//!  11. GetExitCodeProcess.
//!  12. ReadFile drain of both pipe read-ends until EOF.
//!  13. CloseHandle on every owned HANDLE; DeleteProcThreadAttributeList; FreeSid.
//!
//! Resource limits ENFORCED by the Windows kernel via Job Objects — backend
//! returns `ResourceLimitEnforcement::Enforced`.
//!
//! Filesystem allowlists (`fs_read_allow`/`fs_write_allow`) ARE wired to
//! AppContainer DACLs (R61). AppContainer SIDs deny access to user-profile
//! paths by default, so before `CreateProcess` the backend adds an
//! ACCESS_ALLOWED ACE for the AppContainer package SID to each allowlisted
//! path's existing DACL (read+execute for `fs_read_allow`, +write for
//! `fs_write_allow`) via `GetNamedSecurityInfoW` → `SetEntriesInAclW` →
//! `SetNamedSecurityInfoW` — merging into, never replacing, the path's DACL.
//! The grant is REVOKED when the spawn finishes (success/timeout/error) by a
//! RAII `DaclGrantGuard`. Only a hard crash between grant and revoke can leak
//! an ACE, and it would grant a per-PID AppContainer profile SID that is dead
//! once the process exits. Paths must be absolute and local — UNC/device paths
//! are rejected so a remote share's DACL is never touched. (`NetworkPolicy::
//! AllowHosts` WFP DNS gating remains queued separately.)

// The probe cache is consumed only by the Windows backend; it is also
// exercised by unit tests on every platform. Gate it to exactly those two so
// it is neither dead code on non-Windows lib builds nor duplicated.
#[cfg(any(windows, test))]
use std::time::{Duration, Instant};

/// How long a *negative* AppContainer probe verdict is trusted before a fresh
/// probe is warranted. A positive verdict is sticky for the process lifetime;
/// a negative one is cached only briefly so a transient stall (AV image scan,
/// disk contention, slow profile-service RPC) self-heals after the window
/// instead of re-running the (now wall-clock-guarded) probe on every command.
/// (FerroxLabs/wayland-core#125)
///
/// Only the Windows backend consumes this; the cache tests supply their own
/// TTL, so it is `cfg(windows)` (not `test`) to stay dead-code-free elsewhere.
#[cfg(windows)]
const NEGATIVE_PROBE_TTL: Duration = Duration::from_secs(30);

/// Temporal cache verdict for the AppContainer real-spawn probe.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeVerdict {
    /// Never probed (or the last negative verdict has expired).
    Unknown,
    /// Probe succeeded — sticky for the process lifetime.
    Available,
    /// Probe failed; do not re-probe until this instant.
    UnavailableUntil(Instant),
}

/// Availability cache for the AppContainer probe.
///
/// Positive results stick (the sandbox stays available once proven); negative
/// results are cached with a short TTL so a transient failure neither
/// permanently disables isolation (a silent security regression) nor forces a
/// full probe on every command (the ~120s-per-Bash hang of
/// FerroxLabs/wayland-core#125). This temporal logic is platform-independent
/// and unit-tested on all targets; the Windows backend drives it with the real
/// Win32 probe.
#[cfg(any(windows, test))]
struct ProbeCache {
    verdict: ProbeVerdict,
}

#[cfg(any(windows, test))]
impl ProbeCache {
    const fn new() -> Self {
        Self {
            verdict: ProbeVerdict::Unknown,
        }
    }

    /// A cached verdict usable *without* re-probing, or `None` when a fresh
    /// probe is warranted (never probed, or a negative verdict has expired).
    fn cached(&self, now: Instant) -> Option<bool> {
        match self.verdict {
            ProbeVerdict::Available => Some(true),
            ProbeVerdict::UnavailableUntil(until) if now < until => Some(false),
            ProbeVerdict::Unknown | ProbeVerdict::UnavailableUntil(_) => None,
        }
    }

    /// Record a fresh probe result. A success is sticky; a failure is trusted
    /// for `neg_ttl` before the next `cached()` call will re-probe.
    ///
    /// A negative NEVER downgrades a sticky `Available`: the probe runs
    /// outside the cache lock, so a concurrent stalled probe can finish
    /// (and time out) after a successful one — the proven-working verdict
    /// must win regardless of record order.
    fn record(&mut self, available: bool, now: Instant, neg_ttl: Duration) {
        if !available && self.verdict == ProbeVerdict::Available {
            return;
        }
        self.verdict = if available {
            ProbeVerdict::Available
        } else {
            ProbeVerdict::UnavailableUntil(now + neg_ttl)
        };
    }
}

#[cfg(test)]
mod probe_cache_tests {
    use super::{Duration, Instant, ProbeCache};

    #[test]
    fn unknown_forces_a_probe() {
        let c = ProbeCache::new();
        assert_eq!(c.cached(Instant::now()), None);
    }

    #[test]
    fn positive_is_sticky() {
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        c.record(true, t0, Duration::from_secs(30));
        assert_eq!(c.cached(t0), Some(true));
        // Still available far in the future — never re-probes.
        assert_eq!(c.cached(t0 + Duration::from_secs(3600)), Some(true));
    }

    #[test]
    fn negative_is_cached_then_self_heals() {
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        let ttl = Duration::from_secs(30);
        c.record(false, t0, ttl);
        // Within the TTL: cheap negative, no re-probe.
        assert_eq!(c.cached(t0 + Duration::from_secs(10)), Some(false));
        // At/after the TTL: verdict expires → re-probe (self-heal).
        assert_eq!(c.cached(t0 + ttl), None);
        assert_eq!(c.cached(t0 + Duration::from_secs(31)), None);
    }

    #[test]
    fn late_negative_never_downgrades_a_sticky_positive() {
        // Two concurrent probes race the first fill: A succeeds and records
        // Available; B (stalled, timed out) records its failure LAST. The
        // proven-working verdict must survive.
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        c.record(true, t0, Duration::from_secs(30));
        c.record(false, t0 + Duration::from_secs(1), Duration::from_secs(30));
        assert_eq!(c.cached(t0 + Duration::from_secs(2)), Some(true));
        assert_eq!(c.cached(t0 + Duration::from_secs(3600)), Some(true));
    }

    #[test]
    fn negative_then_positive_recovers_and_sticks() {
        let mut c = ProbeCache::new();
        let t0 = Instant::now();
        c.record(false, t0, Duration::from_secs(30));
        // A later successful probe upgrades to sticky-available.
        c.record(true, t0 + Duration::from_secs(31), Duration::from_secs(30));
        assert_eq!(c.cached(t0 + Duration::from_secs(3600)), Some(true));
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::super::SandboxBackend;
    use super::{NEGATIVE_PROBE_TTL, ProbeCache};
    use crate::error::{Result, SandboxError};
    use crate::manifest::{NetworkPolicy, SandboxManifest};
    use crate::{ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::mem;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::ptr;
    use std::sync::mpsc;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Authorization::{
        DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, REVOKE_ACCESS,
        SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
        TRUSTEE_IS_UNKNOWN,
    };
    use windows_sys::Win32::Security::Isolation::{
        CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
    };
    use windows_sys::Win32::Security::{
        ACL, AllocateAndInitializeSid, CreateRestrictedToken, DACL_SECURITY_INFORMATION,
        DISABLE_MAX_PRIVILEGE, FreeSid, GetLengthSid, GetSidSubAuthority, GetSidSubAuthorityCount,
        GetTokenInformation, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
        SID_IDENTIFIER_AUTHORITY, SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY,
        TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
    };

    /// `SE_GROUP_INTEGRITY` from `winnt.h`. Not re-exported by windows-sys
    /// (versions ≤ 0.59); defined locally per the Windows SDK header.
    const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;
    /// `SUB_CONTAINERS_AND_OBJECTS_INHERIT` (`accctrl.h`) — not re-exported by
    /// windows-sys 0.59. `CONTAINER_INHERIT_ACE (0x2) | OBJECT_INHERIT_ACE (0x1)`:
    /// the ACE propagates to child directories and files.
    const SUB_CONTAINERS_AND_OBJECTS_INHERIT: u32 = 0x3;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, ReadFile,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PRIORITY_CLASS,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME,
        JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOB_OBJECT_UILIMIT_DESKTOP,
        JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
        JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
        JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
        JOB_OBJECT_UILIMIT_WRITECLIPBOARD, JOBOBJECT_BASIC_UI_RESTRICTIONS,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicUIRestrictions,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, CREATE_SUSPENDED, CreateProcessAsUserW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
        GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
        WaitForSingleObject,
    };

    /// `HRESULT_FROM_WIN32` is a C macro in `winerror.h`; `windows-sys` doesn't
    /// re-export it. Inlined here for positive Win32 error codes (the only kind
    /// we pass — `ERROR_ALREADY_EXISTS` etc.).
    #[inline]
    const fn hresult_from_win32(code: u32) -> i32 {
        ((code & 0xFFFF) | 0x80070000) as i32
    }

    /// Per-process AppContainer profile name. Includes PID so two engine
    /// instances on the same host do not collide. The backend never deletes
    /// this profile — it persists for the process lifetime (and beyond, on
    /// crash) and is reused across spawns idempotently via
    /// `ERROR_ALREADY_EXISTS` + `DeriveAppContainerSidFromAppContainerName`.
    /// Concurrent in-process spawns therefore share the same SID safely.
    fn profile_name_str() -> String {
        format!("WCoreSandbox-{}", std::process::id())
    }

    fn profile_name_w() -> Vec<u16> {
        widen(&profile_name_str())
    }

    fn widen(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn widen_os(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Windows cmdline-quoting per the MSVC C runtime / `CommandLineToArgvW`
    /// round-trip rules (Daniel Colascione's algorithm). Quotes are only
    /// added when needed (whitespace, embedded `"`, newline, or empty
    /// string); backslashes are doubled when followed by `"` or by the
    /// closing quote.
    fn quote_arg(arg: &str) -> String {
        let needs_quote = arg.is_empty()
            || arg
                .chars()
                .any(|c| matches!(c, ' ' | '\t' | '"' | '\n' | '\x0b'));
        if !needs_quote {
            return arg.to_string();
        }
        let mut out = String::with_capacity(arg.len() + 2);
        out.push('"');
        let mut backslashes = 0usize;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    for _ in 0..(backslashes * 2 + 1) {
                        out.push('\\');
                    }
                    out.push('"');
                    backslashes = 0;
                }
                other => {
                    for _ in 0..backslashes {
                        out.push('\\');
                    }
                    out.push(other);
                    backslashes = 0;
                }
            }
        }
        for _ in 0..(backslashes * 2) {
            out.push('\\');
        }
        out.push('"');
        out
    }

    /// Classification of a bare (non-absolute) `argv[0]`. Only `cmd` is
    /// runnable under the Low-integrity restricted-token AppContainer; every
    /// other shell is recognized solely so the resolver can return a clear,
    /// actionable error instead of a cryptic `CreateProcessAsUserW 0x2`
    /// (file-not-found, #323) or `0xC0000135` (DLL-not-found, #324) at spawn
    /// time.
    #[derive(PartialEq, Eq, Debug)]
    enum BareShell {
        /// `cmd` / `cmd.exe` — lives in `System32`, imports only the minimal
        /// `System32` DLL set, and is the one shell that loads under this
        /// sandbox token.
        Cmd,
        /// `powershell` / `pwsh` — NOT in `System32` (Windows PowerShell is in
        /// `System32\WindowsPowerShell\v1.0\`, pwsh in `Program Files`), and
        /// requires .NET / GAC assemblies that do not load under the Low-IL
        /// restricted token (#324).
        PowerShell,
        /// `bash` / `sh` — git-bash needs `msys-2.0.dll` from `Program Files`,
        /// and even static busybox-w32 links network/auth/UI DLLs the Low-IL
        /// token cannot load (#324). Not resolvable from `System32` either.
        Unsupported,
    }

    /// Classify a bare executable name against the canonical Windows shells.
    /// Returns `None` for anything not recognized as a shell at all (those are
    /// rejected with the generic "pass an absolute path" message). Resolution
    /// of `cmd` goes through `GetSystemDirectoryW` (always `C:\Windows\System32`,
    /// excluding CWD/PATH) — never `SearchPathW` — so a caller can never pull
    /// something from `PATH` whose resolution is operator- or LLM-influenceable.
    ///
    /// Both `cmd` and `cmd.exe` map to `Cmd` because Windows callers
    /// conventionally omit `.exe`; the resolver appends it when concatenating
    /// against `System32\`.
    fn classify_bare_shell(name: &str) -> Option<BareShell> {
        match name.to_ascii_lowercase().as_str() {
            "cmd" | "cmd.exe" => Some(BareShell::Cmd),
            "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" => Some(BareShell::PowerShell),
            "bash" | "bash.exe" | "sh" | "sh.exe" => Some(BareShell::Unsupported),
            _ => None,
        }
    }

    /// Returns true for any UNC / device path: `\\server\share\…`, `\\?\…`,
    /// `\\.\…`, plus the forward-slash variants Rust's `Path` also accepts on
    /// Windows. These are rejected outright because `Path::is_absolute()`
    /// returns true for them, and naively passing them to
    /// `CreateProcessAsUserW`'s image-load path triggers SMB / device-driver
    /// access in the PARENT's security context — an NTLM-relay vector that
    /// happens BEFORE the AppContainer token's network policy applies.
    fn is_unc_or_device_path(p: &str) -> bool {
        p.starts_with("\\\\") || p.starts_with("//")
    }

    /// True only for the Windows VERBATIM DISK form `\\?\X:\...` — an
    /// extended-length spelling of an ordinary local drive-letter path.
    /// `std::fs::canonicalize` returns this form for EVERY local path on
    /// Windows, so the fs-allowlist guard must treat it as local, not as a
    /// UNC/device path. Genuine UNC (`\\?\UNC\...`), device (`\\.\...`),
    /// and other verbatim (`\\?\...`) prefixes are NOT VerbatimDisk and stay
    /// rejected. The OS path parser handles slash/case variants for us.
    fn is_verbatim_disk_path(path: &std::path::Path) -> bool {
        use std::path::{Component, Prefix};
        matches!(
            path.components().next(),
            Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::VerbatimDisk(_))
        )
    }

    /// Resolve a program reference into an absolute UTF-16 path suitable for
    /// `lpApplicationName`. Hard-fails on any failure — the caller must
    /// propagate the error rather than fall back to a NULL `lpApplicationName`
    /// (the original 0xcb regression we already fixed once).
    ///
    /// Rejection rules (each surfaces a distinct error message):
    ///   1. Empty.
    ///   2. UNC / device path (`\\server\…`, `\\?\…`, `\\.\…`) — NTLM-relay
    ///      vector.
    ///   3. Bare name not in the shell allowlist — pass an absolute path.
    ///   4. Absolute path that doesn't exist OR is unreadable.
    ///   5. Absolute path that is a directory, not a file.
    ///
    /// Resolution rules:
    ///   * Absolute file → validated via `try_exists()` + `metadata()`,
    ///     returned widened.
    ///   * Bare `cmd` / `cmd.exe` → pinned to `C:\Windows\System32\cmd.exe`,
    ///     whose existence is then validated (the bare-shell branch used to
    ///     skip the existence check the absolute branch performs, so an
    ///     unresolvable shell surfaced only as a cryptic spawn-time `0x2` —
    ///     #323).
    ///   * Bare `powershell` / `pwsh` → rejected with a clear message: these
    ///     do NOT live in `System32` (the old code pinned them there, yielding
    ///     `0x2`/file-not-found, #323) and cannot load their .NET/GAC
    ///     dependencies under the Low-IL restricted-token AppContainer
    ///     (`0xC0000135`, #324). The message names the real install locations
    ///     and the supported alternative.
    ///   * Bare `bash` / `sh` → rejected with a clear message: git-bash and
    ///     busybox link DLLs the Low-IL token cannot load (#324), and they are
    ///     not in `System32` to begin with.
    fn resolve_program(program: &str) -> Result<Vec<u16>> {
        if program.is_empty() {
            return Err(SandboxError::ExecFailed("argv[0] is empty".into()));
        }
        if is_unc_or_device_path(program) {
            return Err(SandboxError::ExecFailed(format!(
                "argv[0] {program:?} is a UNC or device path; rejected to prevent \
                 NTLM relay / SMB credential disclosure during image load"
            )));
        }
        let p = std::path::Path::new(program);
        if p.is_absolute() {
            match p.try_exists() {
                Ok(true) => {
                    let md = p.metadata().map_err(|e| {
                        SandboxError::ExecFailed(format!(
                            "argv[0] {program:?} metadata read failed: {e}"
                        ))
                    })?;
                    if md.file_type().is_dir() {
                        return Err(SandboxError::ExecFailed(format!(
                            "argv[0] {program:?} is a directory, not an executable"
                        )));
                    }
                    return Ok(widen_os(p.as_os_str()));
                }
                Ok(false) => {
                    return Err(SandboxError::ExecFailed(format!(
                        "argv[0] {program:?} does not exist"
                    )));
                }
                Err(e) => {
                    return Err(SandboxError::ExecFailed(format!(
                        "argv[0] {program:?} is unreadable: {e}"
                    )));
                }
            }
        }
        match classify_bare_shell(program) {
            Some(BareShell::Cmd) => {}
            Some(BareShell::PowerShell) => {
                return Err(SandboxError::ExecFailed(format!(
                    "argv[0] {program:?}: PowerShell is not supported under the Windows \
                     AppContainer sandbox. powershell.exe lives in \
                     C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\ and pwsh.exe in \
                     C:\\Program Files\\PowerShell\\7\\ (neither is in System32), and both \
                     require .NET / GAC assemblies that cannot load under the sandbox's \
                     Low-integrity restricted token (they fail with STATUS_DLL_NOT_FOUND \
                     0xC0000135). Use cmd as the sandbox shell, or pass an absolute path to \
                     a sandbox-compatible executable."
                )));
            }
            Some(BareShell::Unsupported) => {
                return Err(SandboxError::ExecFailed(format!(
                    "argv[0] {program:?}: this shell is not supported under the Windows \
                     AppContainer sandbox. git-bash requires msys-2.0.dll from \
                     C:\\Program Files\\Git, and even static busybox-w32 links \
                     network/auth/UI DLLs (Secur32, WS2_32, bcrypt, USER32) that cannot \
                     load under the sandbox's Low-integrity restricted token \
                     (STATUS_DLL_NOT_FOUND 0xC0000135). Use cmd as the sandbox shell, or \
                     pass an absolute path to a sandbox-compatible executable."
                )));
            }
            None => {
                return Err(SandboxError::ExecFailed(format!(
                    "argv[0] {program:?} is not an absolute path and is not a recognized \
                     sandbox shell. The only bare shell the AppContainer sandbox can run is \
                     cmd (cmd.exe). Pass the absolute path to the executable."
                )));
            }
        }
        // Bare `cmd` / `cmd.exe`: pin to System32\cmd.exe and validate it
        // exists, mirroring the absolute-path branch's existence check so an
        // unresolvable shell yields a descriptive error naming the path rather
        // than a cryptic CreateProcessAsUserW 0x2 at spawn time (#323).
        let sysdir = system_directory()?;
        let mut buf = sysdir;
        if !buf.ends_with(&[b'\\' as u16]) {
            buf.push(b'\\' as u16);
        }
        for u in OsStr::new(program).encode_wide() {
            buf.push(u);
        }
        if !program.to_ascii_lowercase().ends_with(".exe") {
            for u in OsStr::new(".exe").encode_wide() {
                buf.push(u);
            }
        }
        // Validate existence on the widened (NUL-free) path before returning.
        let resolved = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buf));
        match resolved.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                return Err(SandboxError::ExecFailed(format!(
                    "argv[0] {program:?} resolved to {} which does not exist",
                    resolved.display()
                )));
            }
            Err(e) => {
                return Err(SandboxError::ExecFailed(format!(
                    "argv[0] {program:?} resolved to {} which is unreadable: {e}",
                    resolved.display()
                )));
            }
        }
        buf.push(0);
        Ok(buf)
    }

    /// Returns `C:\Windows\System32` as UTF-16 without the trailing NUL.
    /// Query a child process's token integrity level (RID of the last
    /// sub-authority on the `TokenIntegrityLevel` SID). Returns
    /// `SECURITY_MANDATORY_LOW_RID = 0x1000` for a properly-pinned
    /// AppContainer child. Used as a runtime invariant check
    /// post-spawn — OS-layer proof that the kernel honored our
    /// explicit `SetTokenInformation(IntegrityLevel=Low)` call before
    /// image load.
    unsafe fn query_process_integrity_rid(process_handle: HANDLE) -> Result<u32> {
        let mut token: HANDLE = ptr::null_mut();
        if unsafe { OpenProcessToken(process_handle, TOKEN_QUERY, &mut token) } == 0 {
            return Err(SandboxError::ExecFailed(format!(
                "OpenProcessToken(child, TOKEN_QUERY): {:#x}",
                unsafe { GetLastError() }
            )));
        }
        let _token_guard = OwnedHandle::new(token);

        let mut needed: u32 = 0;
        // Sizing probe (ignored return — we look at `needed`).
        let _ = unsafe {
            GetTokenInformation(token, TokenIntegrityLevel, ptr::null_mut(), 0, &mut needed)
        };
        if needed == 0 {
            return Err(SandboxError::ExecFailed(format!(
                "GetTokenInformation sizing: {:#x}",
                unsafe { GetLastError() }
            )));
        }
        let mut buf: Vec<u8> = vec![0u8; needed as usize];
        if unsafe {
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                buf.as_mut_ptr() as _,
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(SandboxError::ExecFailed(format!(
                "GetTokenInformation: {:#x}",
                unsafe { GetLastError() }
            )));
        }
        let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
        let sid = label.Label.Sid;
        if sid.is_null() {
            return Err(SandboxError::ExecFailed("integrity SID is null".into()));
        }
        let count_ptr = unsafe { GetSidSubAuthorityCount(sid as _) };
        if count_ptr.is_null() {
            return Err(SandboxError::ExecFailed(
                "GetSidSubAuthorityCount returned null".into(),
            ));
        }
        let count = unsafe { *count_ptr };
        if count == 0 {
            return Err(SandboxError::ExecFailed(
                "integrity SID has no sub-authorities".into(),
            ));
        }
        let rid_ptr = unsafe { GetSidSubAuthority(sid as _, (count - 1) as u32) };
        if rid_ptr.is_null() {
            return Err(SandboxError::ExecFailed(
                "GetSidSubAuthority returned null".into(),
            ));
        }
        Ok(unsafe { *rid_ptr })
    }

    /// `SECURITY_MANDATORY_LOW_RID` from the SDK — the SubAuthority of
    /// the Low Integrity Level SID (`S-1-16-4096`).
    const SECURITY_MANDATORY_LOW_RID: u32 = 0x1000;

    /// Job Object `ActiveProcessLimit` — the maximum number of concurrently
    /// active processes in the sandbox job (#321, #322). High enough for a
    /// shell plus a parallel build's worker processes, low enough to bound a
    /// runaway fork. This is NOT the primary fork-bomb guard: `KILL_ON_JOB_CLOSE`
    /// plus the optional per-process memory cap are. It is a defense-in-depth
    /// ceiling.
    ///
    /// The previous value of 1 was the root cause of BOTH #322 (the cap itself)
    /// AND #321 (reported as "AppContainer cannot spawn child processes — Bash
    /// runs only cmd builtins"). cmd.exe is process #1 in the job and is
    /// assigned to the job before `ResumeThread`; with a cap of 1 every child
    /// it tries to launch is process #2, which the kernel denies before the
    /// child's image runs. cmd builtins (`echo`, `dir`) work because they
    /// execute IN cmd's process; external programs (`git`, `node`, even
    /// `cmd /c <abs cmd.exe> /c exit 42`) fail at the fork. #321's restricted-
    /// token/CSRSS theory does not hold: the spawn is rejected by the job cap
    /// before the child token is ever used, and the restricted token is left
    /// untouched here so the `live_integrity.rs` boundary assertions still hold.
    const SANDBOX_ACTIVE_PROCESS_LIMIT: u32 = 512;

    fn system_directory() -> Result<Vec<u16>> {
        let needed = unsafe { GetSystemDirectoryW(ptr::null_mut(), 0) };
        if needed == 0 {
            return Err(SandboxError::ExecFailed(format!(
                "GetSystemDirectoryW probe: {:#x}",
                unsafe { GetLastError() }
            )));
        }
        let mut buf: Vec<u16> = vec![0u16; needed as usize];
        let written = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) };
        if written == 0 || written as usize >= buf.len() {
            return Err(SandboxError::ExecFailed(format!(
                "GetSystemDirectoryW: written={} buf={} last_err={:#x}",
                written,
                buf.len(),
                unsafe { GetLastError() }
            )));
        }
        buf.truncate(written as usize);
        Ok(buf)
    }

    /// Build a double-null-terminated UTF-16 env block from `(K, V)` pairs.
    ///
    /// Per the `CREATE_UNICODE_ENVIRONMENT` contract, the block must be
    /// sorted alphabetically by key (case-insensitively on Windows, since
    /// the OS treats env keys as case-insensitive). Duplicate keys are
    /// collapsed last-wins so manifest-supplied vars override the parent's
    /// seeded values; **the retained key casing is the LAST insert's casing**,
    /// which on Windows is harmless (case-insensitive lookups) but operators
    /// reading the trace logs will see whatever case the manifest used.
    ///
    /// Validation rejects:
    ///   * Empty key.
    ///   * `=` or any ASCII control char (`< 0x20`) or NUL in keys — these
    ///     break the `K=V\0` framing AND open log-injection via `tracing::trace!`
    ///     emission of the key.
    ///   * NUL in values (kernel framing).
    ///   * Newline / CR / TAB in values of security-relevant keys (PATH,
    ///     COMSPEC, PATHEXT, SYSTEMROOT, WINDIR) — downstream parsers (cmd.exe
    ///     `set` output, `[Environment]::GetEnvironmentVariables()`) split on
    ///     LF and would treat injected content as additional entries.
    fn build_env_block(pairs: &[(String, String)]) -> Result<Vec<u16>> {
        let mut map: BTreeMap<String, (String, String)> = BTreeMap::new();
        for (k, v) in pairs {
            if k.is_empty() {
                return Err(SandboxError::ExecFailed("env key is empty".into()));
            }
            if k.chars()
                .any(|c| c == '=' || c == '\0' || (c as u32) < 0x20)
            {
                return Err(SandboxError::ExecFailed(format!(
                    "env key {k:?} contains '=' or a control character or NUL"
                )));
            }
            if v.contains('\0') {
                return Err(SandboxError::ExecFailed(format!(
                    "env value for {k:?} contains NUL"
                )));
            }
            let upper_k = k.to_ascii_uppercase();
            if matches!(
                upper_k.as_str(),
                "PATH" | "COMSPEC" | "PATHEXT" | "SYSTEMROOT" | "WINDIR"
            ) && v.chars().any(|c| matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(SandboxError::ExecFailed(format!(
                    "env value for security-relevant key {k:?} contains a newline or tab"
                )));
            }
            map.insert(upper_k, (k.clone(), v.clone()));
        }
        let mut block: Vec<u16> = Vec::with_capacity(pairs.len() * 32);
        for (k, v) in map.values() {
            for u in OsStr::new(k).encode_wide() {
                block.push(u);
            }
            block.push(b'=' as u16);
            for u in OsStr::new(v).encode_wide() {
                block.push(u);
            }
            block.push(0);
        }
        block.push(0);
        if block.len() == 1 {
            block.push(0);
        }
        Ok(block)
    }

    /// Vars whose VALUES are safe to print in trace logs. Everything outside
    /// this list — especially anything caller-supplied via `manifest.env` —
    /// gets its value redacted as `<{len} bytes redacted>` because the
    /// manifest is the project's explicit secret-bearing surface (e.g.
    /// `AWS_SECRET_ACCESS_KEY`, `*_TOKEN`, `*_KEY`).
    fn is_trace_safe_env_key(k: &str) -> bool {
        matches!(
            k.to_ascii_uppercase().as_str(),
            "ALLUSERSPROFILE"
                | "APPDATA"
                | "COMMONPROGRAMFILES"
                | "COMMONPROGRAMFILES(X86)"
                | "COMMONPROGRAMW6432"
                | "COMSPEC"
                | "HOMEDRIVE"
                | "HOMEPATH"
                | "LOCALAPPDATA"
                | "NUMBER_OF_PROCESSORS"
                | "PATH"
                | "PATHEXT"
                | "PROCESSOR_ARCHITECTURE"
                | "PROCESSOR_ARCHITEW6432"
                | "PROGRAMDATA"
                | "PROGRAMFILES"
                | "PROGRAMFILES(X86)"
                | "PROGRAMW6432"
                | "PUBLIC"
                | "SYSTEMDRIVE"
                | "SYSTEMROOT"
                | "TEMP"
                | "TMP"
                | "USERDOMAIN"
                | "USERNAME"
                | "USERPROFILE"
                | "WINDIR"
        )
    }

    /// Compute the AppContainer's per-profile package storage path:
    /// `%LOCALAPPDATA%\Packages\<profile>\AC`. The kernel creates this
    /// directory automatically when `CreateAppContainerProfile` runs;
    /// it is the only filesystem location an AppContainer-tagged token
    /// has read/write access to without explicit ACL grants.
    fn appcontainer_package_root() -> Option<std::path::PathBuf> {
        let lad = std::env::var_os("LOCALAPPDATA")?;
        let mut p = std::path::PathBuf::from(lad);
        p.push("Packages");
        p.push(profile_name_str());
        p.push("AC");
        Some(p)
    }

    /// RAII helper that closes a HANDLE on drop. Skips closing if the handle
    /// is null or `INVALID_HANDLE_VALUE`.
    struct OwnedHandle(HANDLE);
    impl OwnedHandle {
        fn new(h: HANDLE) -> Self {
            Self(h)
        }
        fn as_raw(&self) -> HANDLE {
            self.0
        }
    }
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                    CloseHandle(self.0);
                }
            }
        }
    }

    /// RAII for a SID allocated with `AllocateAndInitializeSid`.
    struct OwnedSid(*mut core::ffi::c_void);
    impl OwnedSid {
        fn as_psid(&self) -> *mut core::ffi::c_void {
            self.0
        }
    }
    impl Drop for OwnedSid {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() {
                    FreeSid(self.0 as _);
                }
            }
        }
    }

    fn allocate_sid(authority: [u8; 6], subauthorities: &[u32]) -> Result<OwnedSid> {
        let auth = SID_IDENTIFIER_AUTHORITY { Value: authority };
        let mut sub = [0u32; 8];
        for (i, s) in subauthorities.iter().enumerate().take(8) {
            sub[i] = *s;
        }
        let mut sid: *mut core::ffi::c_void = ptr::null_mut();
        let ok = unsafe {
            AllocateAndInitializeSid(
                &auth,
                subauthorities.len() as u8,
                sub[0],
                sub[1],
                sub[2],
                sub[3],
                sub[4],
                sub[5],
                sub[6],
                sub[7],
                &mut sid,
            )
        };
        if ok == 0 || sid.is_null() {
            return Err(SandboxError::ExecFailed(format!(
                "AllocateAndInitializeSid: {:#x}",
                unsafe { GetLastError() }
            )));
        }
        Ok(OwnedSid(sid))
    }

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
    fn probe_cache() -> &'static Mutex<ProbeCache> {
        static CACHE: OnceLock<Mutex<ProbeCache>> = OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(ProbeCache::new()))
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
            let cache = probe_cache();
            {
                let g = cache.lock().expect("probe cache poisoned");
                if let Some(cached) = g.cached(Instant::now()) {
                    return cached;
                }
            }
            let result = probe_appcontainer_available();
            let mut g = cache.lock().expect("probe cache poisoned");
            g.record(result, Instant::now(), NEGATIVE_PROBE_TTL);
            result
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
            // fires first). Abandoning the blocking task on elapse is safe for
            // CLEANUP — it reaps its own child via the KILL_ON_JOB_CLOSE Job
            // Object once its handles drop — but note it is not a cancel: if
            // the stall was in setup (pre-spawn), the child may still run to
            // completion (bounded by the inner wait) AFTER the caller was told
            // Timeout, so a retried mutating command can double-execute. That
            // matches BashTool's pre-existing outer-timeout semantics.
            let ceiling = manifest
                .timeout
                .unwrap_or(Duration::from_secs(60))
                .saturating_add(Duration::from_secs(15));
            let handle = tokio::task::spawn_blocking(move || execute_blocking(&manifest, &cmd));
            match tokio::time::timeout(ceiling, handle).await {
                Ok(joined) => joined.map_err(|e| SandboxError::ExecFailed(format!("join: {e}")))?,
                Err(_elapsed) => Err(SandboxError::Timeout),
            }
        }
    }

    /// Cheap supports-check — does this host's Win32 API surface accept
    /// `DeriveAppContainerSidFromAppContainerName`? Returns true on Win8+
    /// and false elsewhere. Does NOT prove a full spawn will succeed;
    /// callers wanting that guarantee should use `is_available()` on the
    /// backend, which does the real probe.
    fn is_supported_by_runtime() -> bool {
        unsafe {
            let name = profile_name_w();
            let mut sid_ptr: *mut core::ffi::c_void = ptr::null_mut();
            let hr = DeriveAppContainerSidFromAppContainerName(
                name.as_ptr(),
                &mut sid_ptr as *mut _ as _,
            );
            if hr == 0 {
                if !sid_ptr.is_null() {
                    FreeSid(sid_ptr as _);
                }
                true
            } else {
                false
            }
        }
    }

    fn probe_appcontainer_available() -> bool {
        if !is_supported_by_runtime() {
            tracing::error!(
                target: "wcore_sandbox",
                "AppContainer Win32 surface unavailable on this host \
                 (DeriveAppContainerSidFromAppContainerName failed); sandbox disabled."
            );
            return false;
        }
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
        // of the wait cannot hang the caller. Orphaning the thread on timeout
        // is safe — `execute_blocking` reaps its own child and the Job Object
        // carries KILL_ON_JOB_CLOSE, so the child (and its image scan) is torn
        // down when the blocking call finally returns and drops its handles.
        const PROBE_WALL_CLOCK: Duration = Duration::from_secs(15);
        let (tx, rx) = mpsc::channel();
        if std::thread::Builder::new()
            .name("appcontainer-probe".into())
            .spawn(move || {
                let _ = tx.send(execute_blocking(&manifest, &cmd));
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
                tracing::error!(
                    target: "wcore_sandbox",
                    "AppContainer probe thread ended without a result; sandbox disabled."
                );
                false
            }
        }
    }

    fn execute_blocking(manifest: &SandboxManifest, cmd: &SandboxCommand) -> Result<SandboxOutput> {
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

        unsafe {
            // ---- 1. AppContainer SID ----
            let name = profile_name_w();
            let display = widen("Wayland-Core Sandbox");
            let desc = widen("Sandboxed tool execution for Wayland-Core");
            let mut sid_ptr: *mut core::ffi::c_void = ptr::null_mut();
            let create_hr = CreateAppContainerProfile(
                name.as_ptr(),
                display.as_ptr(),
                desc.as_ptr(),
                ptr::null(),
                0,
                &mut sid_ptr as *mut _ as _,
            );
            if create_hr != 0 {
                let already_exists = create_hr == hresult_from_win32(ERROR_ALREADY_EXISTS);
                let derive_hr = DeriveAppContainerSidFromAppContainerName(
                    name.as_ptr(),
                    &mut sid_ptr as *mut _ as _,
                );
                if derive_hr != 0 || sid_ptr.is_null() {
                    return Err(SandboxError::ExecFailed(format!(
                        "AppContainer SID acquisition failed (create={create_hr:#x} derive={derive_hr:#x} already_exists={already_exists})"
                    )));
                }
            }
            // Profile is intentionally LEAKED — see SidGuard's drop. This
            // avoids the concurrent-spawn-deletes-mid-execution race where
            // thread A's SidGuard nukes the profile while thread B's child
            // is still running under the same SID.
            let _sid_guard = SidGuard { sid: sid_ptr };

            // ---- 1b. Filesystem ACL grants (R61) ----
            //
            // Add the AppContainer SID to the DACL of each allowlisted path so
            // the sandboxed child can actually reach the files the manifest
            // grants. `_dacl_guard` is declared AFTER `_sid_guard`, so it drops
            // first (reverse declaration order) and revokes while the SID is
            // still valid. Grants happen BEFORE CreateProcess so the child sees
            // them at image-load time.
            let _dacl_guard = if manifest.fs_read_allow.is_empty()
                && manifest.fs_write_allow.is_empty()
                && manifest.fs_read_deny.is_empty()
            {
                None
            } else {
                let read_paths =
                    grant_appcontainer_dacl(&manifest.fs_read_allow, sid_ptr, ACL_READ_MASK)?;
                // If the write grants fail, the read grants we already applied
                // must be rolled back before we bail.
                let write_paths = match grant_appcontainer_dacl(
                    &manifest.fs_write_allow,
                    sid_ptr,
                    ACL_WRITE_MASK,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        revoke_appcontainer_dacl(&read_paths, sid_ptr);
                        return Err(e);
                    }
                };
                // Apply DENY ACEs after allows. If deny fails, revoke
                // grants before bailing.
                let deny_paths = match deny_appcontainer_dacl(&manifest.fs_read_deny, sid_ptr) {
                    Ok(p) => p,
                    Err(e) => {
                        revoke_appcontainer_dacl(&read_paths, sid_ptr);
                        revoke_appcontainer_dacl(&write_paths, sid_ptr);
                        return Err(e);
                    }
                };
                Some(DaclGrantGuard {
                    read_paths,
                    write_paths,
                    deny_paths,
                    sid: sid_ptr,
                })
            };

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
            let job = OwnedHandle::new(job_raw);

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
            match appcontainer_package_root() {
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
            if ResumeThread(thread.as_raw()) == u32::MAX {
                TerminateProcess(process.as_raw(), 1);
                return Err(SandboxError::ExecFailed(format!(
                    "ResumeThread: {:#x}",
                    GetLastError()
                )));
            }

            let timeout_ms: u32 = match manifest.timeout {
                Some(d) => clamp_timeout_ms(d),
                None => 60_000,
            };

            let wait_res = WaitForSingleObject(process.as_raw(), timeout_ms);
            let mut timed_out = false;
            if wait_res == WAIT_TIMEOUT {
                timed_out = true;
            } else if wait_res != WAIT_OBJECT_0 {
                return Err(SandboxError::ExecFailed(format!(
                    "WaitForSingleObject: {:#x} last_err={:#x}",
                    wait_res,
                    GetLastError()
                )));
            }

            // ---- 12. Exit code + drain ----
            // Capture the child's exit code BEFORE reaping the tree (only
            // meaningful on a clean exit; on timeout it is replaced by the
            // `Timeout` error below).
            let mut exit_code: u32 = 0;
            if !timed_out && GetExitCodeProcess(process.as_raw(), &mut exit_code) == 0 {
                return Err(SandboxError::ExecFailed(format!(
                    "GetExitCodeProcess: {:#x}",
                    GetLastError()
                )));
            }

            // Reap the ENTIRE job tree before draining (#100). The direct child
            // can spawn helpers — most notably a console host (`conhost.exe`) —
            // that outlive it and keep the inherited stdout/stderr write-ends
            // open. A plain `TerminateProcess(child)` leaves them running, so the
            // blocking `drain_pipe` below would never reach EOF and the call
            // would hang far past the timeout (observed as a 120s "command timed
            // out" with no output on disconnected RDP sessions). Terminating the
            // job closes every member's handles so the pipes EOF; bytes already
            // written to the pipe buffers stay readable. The short wait lets the
            // kernel finish closing the handles before we read.
            TerminateJobObject(job.as_raw(), if timed_out { 1 } else { exit_code });
            WaitForSingleObject(process.as_raw(), 2_000);

            let stdout = drain_pipe(stdout_r.as_raw());
            let mut stderr = drain_pipe(stderr_r.as_raw());

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

            Ok(SandboxOutput {
                exit_code: exit_code as i32,
                stdout,
                stderr,
                resource_limits: ResourceLimitEnforcement::Enforced,
            })
        }
    }

    unsafe fn drain_pipe(h: HANDLE) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let mut read: u32 = 0;
            let ok = unsafe {
                ReadFile(
                    h,
                    buf.as_mut_ptr() as _,
                    buf.len() as u32,
                    &mut read,
                    ptr::null_mut(),
                )
            };
            if ok == 0 || read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        out
    }

    fn clamp_timeout_ms(d: Duration) -> u32 {
        let ms = d.as_millis();
        if ms >= INFINITE as u128 - 1 {
            INFINITE - 1
        } else {
            ms as u32
        }
    }

    /// RAII for the AppContainer SID. **The profile itself is intentionally
    /// leaked** — `DeleteAppContainerProfile` is NOT called. Reasons:
    ///
    /// 1. Concurrent in-process spawns share the same profile name
    ///    (`WCoreSandbox-<pid>`). Deleting on one thread's SidGuard drop
    ///    while another thread's child is still running under the same SID
    ///    causes the running child to lose write access to its package
    ///    storage mid-execution. Leaking sidesteps this race entirely.
    /// 2. `CreateAppContainerProfile` is idempotent on `ERROR_ALREADY_EXISTS`
    ///    and `Derive` — subsequent spawns and process restarts reuse the
    ///    same profile cheaply.
    /// 3. The profile's filesystem footprint (`%LOCALAPPDATA%\Packages\
    ///    WCoreSandbox-<pid>\`) is bounded and reused; long-term residue from
    ///    crashed/dead processes is cleaned at process startup if needed in
    ///    a future maintenance pass (not yet wired).
    struct SidGuard {
        sid: *mut core::ffi::c_void,
    }
    impl Drop for SidGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.sid.is_null() {
                    FreeSid(self.sid as _);
                }
            }
        }
    }

    // ===== Filesystem ACL grants (R61) ============================================
    //
    // AppContainer SIDs deny access to user-profile paths by default, so a
    // manifest's `fs_read_allow`/`fs_write_allow` only take effect if we add an
    // ACCESS_ALLOWED ACE for the AppContainer package SID to each path's DACL.
    // We MERGE the ACE into the path's existing DACL (never replace it) and
    // REVOKE it once the child has exited — see `DaclGrantGuard`.

    /// Generic read+execute, granted to `fs_read_allow` paths.
    const ACL_READ_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
    /// Generic read+write+execute, granted to `fs_write_allow` paths.
    const ACL_WRITE_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;

    /// RAII for an ACL produced by `SetEntriesInAclW` / a security descriptor
    /// from `GetNamedSecurityInfoW`. Both are `LocalAlloc`-backed and MUST be
    /// released with `LocalFree` — not the Rust allocator, not `FreeSid`.
    struct LocalFreeGuard(*mut core::ffi::c_void);
    impl Drop for LocalFreeGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() {
                    LocalFree(self.0 as _);
                }
            }
        }
    }

    /// A path is eligible for an ACL grant only if it is absolute AND local —
    /// relative paths are ambiguous, and UNC / `\\?\` / `\\.\` paths could name
    /// a network share whose DACL we must never touch.
    fn acl_path_is_safe(path: &std::path::Path) -> bool {
        if !path.is_absolute() {
            return false;
        }
        // A canonicalized local path arrives as `\\?\X:\...` (verbatim disk),
        // which `is_unc_or_device_path` would otherwise reject. Accept that
        // form explicitly; everything else still goes through the UNC/device
        // rejection.
        if is_verbatim_disk_path(path) {
            return true;
        }
        !is_unc_or_device_path(&path.to_string_lossy())
    }

    /// Build one `EXPLICIT_ACCESS_W` for the AppContainer `sid` with `mask`
    /// permissions and `mode` (`GRANT_ACCESS` / `REVOKE_ACCESS`), inheritable to
    /// child files and directories.
    unsafe fn explicit_access_for_sid(
        sid: *mut core::ffi::c_void,
        mask: u32,
        mode: i32,
    ) -> EXPLICIT_ACCESS_W {
        let mut ea: EXPLICIT_ACCESS_W = unsafe { mem::zeroed() };
        ea.grfAccessPermissions = mask;
        ea.grfAccessMode = mode;
        ea.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
        ea.Trustee.TrusteeForm = TRUSTEE_IS_SID;
        ea.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN;
        ea.Trustee.ptstrName = sid as _;
        ea
    }

    /// Apply one `EXPLICIT_ACCESS_W` (grant or revoke) to `path`'s DACL by
    /// reading the current DACL, merging the entry, and writing it back. Pure
    /// FFI mechanics shared by grant and revoke.
    unsafe fn apply_explicit_access(path: &std::path::Path, ea: &EXPLICIT_ACCESS_W) -> Result<()> {
        let mut path_w: Vec<u16> = widen_os(path.as_os_str());

        // 1. Read the existing DACL (so we merge, never replace).
        let mut old_dacl: *mut ACL = ptr::null_mut();
        let mut sd: *mut core::ffi::c_void = ptr::null_mut();
        let get_rc = unsafe {
            GetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut old_dacl,
                ptr::null_mut(),
                &mut sd,
            )
        };
        if get_rc != 0 {
            return Err(SandboxError::ExecFailed(format!(
                "GetNamedSecurityInfoW for {}: {:#x}",
                path.display(),
                get_rc
            )));
        }
        let _sd_guard = LocalFreeGuard(sd);

        // 2. Merge our entry into the existing DACL.
        let mut new_dacl: *mut ACL = ptr::null_mut();
        let set_rc = unsafe { SetEntriesInAclW(1, ea, old_dacl, &mut new_dacl) };
        if set_rc != 0 {
            return Err(SandboxError::ExecFailed(format!(
                "SetEntriesInAclW for {}: {:#x}",
                path.display(),
                set_rc
            )));
        }
        let _acl_guard = LocalFreeGuard(new_dacl as _);

        // 3. Write the merged DACL back to the object.
        let put_rc = unsafe {
            SetNamedSecurityInfoW(
                path_w.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                new_dacl,
                ptr::null(),
            )
        };
        if put_rc != 0 {
            return Err(SandboxError::ExecFailed(format!(
                "SetNamedSecurityInfoW for {}: {:#x}",
                path.display(),
                put_rc
            )));
        }
        Ok(())
    }

    /// Grant `sid` an ACE (`mask`) on the DACL of each path. Returns the paths
    /// actually granted, so the caller revokes exactly those. On the first hard
    /// failure, revokes what was already granted and returns the error — never
    /// leaves a partial grant behind on the error path.
    unsafe fn grant_appcontainer_dacl(
        paths: &[std::path::PathBuf],
        sid: *mut core::ffi::c_void,
        mask: u32,
    ) -> Result<Vec<std::path::PathBuf>> {
        let mut granted: Vec<std::path::PathBuf> = Vec::new();
        for path in paths {
            // A grant target that doesn't exist can't be ACL'd: GetNamedSecurityInfoW
            // returns ERROR_FILE_NOT_FOUND (0x2), which would abort the entire spawn.
            // The local allowlist intentionally includes optional dev caches
            // (~/.cache, ~/.cargo, ~/.npm, ~/.rustup) that are simply absent on
            // non-developer machines, so skip any path that isn't present rather than
            // failing every sandboxed command. (#321-324)
            if !path.exists() {
                tracing::debug!(
                    target: "wcore_sandbox",
                    path = %path.display(),
                    "skipping AppContainer DACL grant for non-existent path"
                );
                continue;
            }
            if !acl_path_is_safe(path) {
                unsafe { revoke_appcontainer_dacl(&granted, sid) };
                return Err(SandboxError::ExecFailed(format!(
                    "fs allowlist path must be absolute and local (no UNC/device): {}",
                    path.display()
                )));
            }
            let ea = unsafe { explicit_access_for_sid(sid, mask, GRANT_ACCESS) };
            if let Err(e) = unsafe { apply_explicit_access(path, &ea) } {
                unsafe { revoke_appcontainer_dacl(&granted, sid) };
                return Err(e);
            }
            tracing::debug!(
                target: "wcore_sandbox",
                path = %path.display(),
                "granted AppContainer DACL access"
            );
            granted.push(path.clone());
        }
        Ok(granted)
    }

    /// Add a DENY ACE for `sid` on the DACL of each path. The DENY ACE is
    /// placed via `SetEntriesInAclW` with `DENY_ACCESS`; Windows evaluates DENY
    /// before ALLOW, so this overrides any parent-directory grant. Returns the
    /// paths actually denied, so the caller can revoke them in `Drop`. On the
    /// first hard failure, revokes what was already denied and returns the error.
    unsafe fn deny_appcontainer_dacl(
        paths: &[std::path::PathBuf],
        sid: *mut core::ffi::c_void,
    ) -> Result<Vec<std::path::PathBuf>> {
        let mut denied: Vec<std::path::PathBuf> = Vec::new();
        for path in paths {
            // A deny target that doesn't exist needs no ACE (nothing there to read),
            // and GetNamedSecurityInfoW would otherwise fail (0x2) and abort the spawn.
            if !path.exists() {
                tracing::debug!(
                    target: "wcore_sandbox",
                    path = %path.display(),
                    "skipping AppContainer DACL deny for non-existent path"
                );
                continue;
            }
            if !acl_path_is_safe(path) {
                unsafe { revoke_appcontainer_dacl(&denied, sid) };
                return Err(SandboxError::ExecFailed(format!(
                    "fs deny path must be absolute and local (no UNC/device): {}",
                    path.display()
                )));
            }
            let ea = unsafe { explicit_access_for_sid(sid, ACL_READ_MASK, DENY_ACCESS) };
            if let Err(e) = unsafe { apply_explicit_access(path, &ea) } {
                unsafe { revoke_appcontainer_dacl(&denied, sid) };
                return Err(e);
            }
            tracing::debug!(
                target: "wcore_sandbox",
                path = %path.display(),
                "applied AppContainer DACL deny ACE"
            );
            denied.push(path.clone());
        }
        Ok(denied)
    }

    /// Remove the AppContainer `sid`'s ACE from each path's DACL. Best-effort:
    /// a revoke failure is logged but never returned, so it cannot mask the
    /// real execution result. `REVOKE_ACCESS` removes ALL ACEs for the trustee.
    unsafe fn revoke_appcontainer_dacl(paths: &[std::path::PathBuf], sid: *mut core::ffi::c_void) {
        for path in paths {
            let ea = unsafe { explicit_access_for_sid(sid, 0, REVOKE_ACCESS) };
            if let Err(e) = unsafe { apply_explicit_access(path, &ea) } {
                tracing::warn!(
                    target: "wcore_sandbox",
                    path = %path.display(),
                    error = %e,
                    "failed to revoke AppContainer DACL grant; ACE may persist on host until profile is removed"
                );
            }
        }
    }

    /// RAII: revokes the DACL grants and deny ACEs when the spawn finishes
    /// (success, timeout, or error). Declared AFTER `SidGuard` at the call site
    /// so it drops FIRST, while the SID pointer is still valid. The only way a
    /// grant outlives the process is a hard crash/kill between grant and this
    /// drop — and the SID belongs to a per-PID profile (`WCoreSandbox-<pid>`),
    /// so a leaked ACE grants a dead profile, not a live principal.
    struct DaclGrantGuard {
        read_paths: Vec<std::path::PathBuf>,
        write_paths: Vec<std::path::PathBuf>,
        /// Paths where a DENY ACE was added; revoked in Drop so the host DACL
        /// is restored to its pre-spawn state after the sandboxed child exits.
        deny_paths: Vec<std::path::PathBuf>,
        sid: *mut core::ffi::c_void,
    }
    impl Drop for DaclGrantGuard {
        fn drop(&mut self) {
            unsafe {
                revoke_appcontainer_dacl(&self.read_paths, self.sid);
                revoke_appcontainer_dacl(&self.write_paths, self.sid);
                // REVOKE_ACCESS removes all ACEs for this SID, incl. DENY ACEs.
                revoke_appcontainer_dacl(&self.deny_paths, self.sid);
            }
        }
    }

    /// RAII: delete the proc-thread attribute list.
    struct AttrListGuard {
        list: LPPROC_THREAD_ATTRIBUTE_LIST,
    }
    impl Drop for AttrListGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.list.is_null() {
                    DeleteProcThreadAttributeList(self.list);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // ---------- quote_arg ----------

        #[test]
        fn quote_arg_no_special_chars_passes_through() {
            assert_eq!(quote_arg("cmd.exe"), "cmd.exe");
            assert_eq!(quote_arg("/c"), "/c");
            assert_eq!(quote_arg("hello"), "hello");
        }

        // ---------- DACL grant/deny skip absent paths (#321-324 field fix) ----------

        // A path that does not exist must be skipped, not treated as a hard
        // failure: the local allowlist intentionally includes optional dev
        // caches (~/.cache, ~/.cargo, ~/.npm, ~/.rustup) that are absent on
        // non-developer machines. Before the fix, `GetNamedSecurityInfoW`
        // returned ERROR_FILE_NOT_FOUND (0x2) for these, aborting every
        // sandboxed command. The skip happens before the SID is ever used, so a
        // null SID is safe here and proves we never reach the Win32 ACL calls.
        #[test]
        fn grant_skips_nonexistent_path_instead_of_failing() {
            let missing =
                std::path::PathBuf::from(r"C:\__wcore_nonexistent_grant_target__\nope.txt");
            assert!(!missing.exists(), "test precondition: path must not exist");
            let granted =
                unsafe { grant_appcontainer_dacl(&[missing], std::ptr::null_mut(), ACL_READ_MASK) };
            assert!(
                granted.is_ok(),
                "missing grant target must be skipped, got error: {:?}",
                granted.as_ref().err()
            );
            assert!(
                granted.unwrap().is_empty(),
                "an absent path must not be reported as granted"
            );
        }

        #[test]
        fn deny_skips_nonexistent_path_instead_of_failing() {
            let missing = std::path::PathBuf::from(r"C:\__wcore_nonexistent_deny_target__\secret");
            assert!(!missing.exists(), "test precondition: path must not exist");
            let denied = unsafe { deny_appcontainer_dacl(&[missing], std::ptr::null_mut()) };
            assert!(
                denied.is_ok(),
                "missing deny target must be skipped, got error: {:?}",
                denied.as_ref().err()
            );
            assert!(
                denied.unwrap().is_empty(),
                "an absent path must not be reported as denied"
            );
        }

        // ---------- acl_path_is_safe (R61 filesystem grants) ----------

        #[test]
        fn acl_path_is_safe_accepts_absolute_local() {
            assert!(acl_path_is_safe(std::path::Path::new(
                r"C:\Users\Public\work"
            )));
            assert!(acl_path_is_safe(std::path::Path::new(r"D:\data\file.txt")));
        }

        #[test]
        fn acl_path_is_safe_rejects_relative() {
            assert!(!acl_path_is_safe(std::path::Path::new(r"work\file.txt")));
            assert!(!acl_path_is_safe(std::path::Path::new("file.txt")));
        }

        #[test]
        fn acl_path_is_safe_rejects_unc_and_device() {
            // UNC/device paths are absolute but must never have their DACL
            // touched (could be a remote share). The verbatim-disk form
            // `\\?\X:\...` is the one exception and is covered by its own test.
            assert!(!acl_path_is_safe(std::path::Path::new(
                r"\\server\share\file"
            )));
            assert!(!acl_path_is_safe(std::path::Path::new(
                r"\\.\PhysicalDrive0"
            )));
            // Verbatim UNC stays rejected — it names a network share, not a
            // local disk, so it is NOT VerbatimDisk.
            assert!(!acl_path_is_safe(std::path::Path::new(
                r"\\?\UNC\server\share"
            )));
        }

        #[test]
        fn acl_path_is_safe_accepts_verbatim_disk() {
            // `std::fs::canonicalize` returns the verbatim-disk form for local
            // paths on Windows (issue #267: a USB drive canonicalizes to
            // `\\?\E:\...`). These must be accepted, not rejected as UNC.
            assert!(acl_path_is_safe(std::path::Path::new(
                r"\\?\E:\AIWorkspace\Wayland\wcore-temp-1782166469597"
            )));
            assert!(acl_path_is_safe(std::path::Path::new(
                r"\\?\C:\Users\Public\work"
            )));
        }

        #[test]
        fn is_verbatim_disk_path_classifies_prefixes() {
            assert!(is_verbatim_disk_path(std::path::Path::new(r"\\?\D:\data")));
            // Verbatim-UNC, device, and genuine UNC are NOT verbatim-disk.
            assert!(!is_verbatim_disk_path(std::path::Path::new(r"\\?\UNC\s\h")));
            assert!(!is_verbatim_disk_path(std::path::Path::new(r"\\.\COM1")));
            assert!(!is_verbatim_disk_path(std::path::Path::new(
                r"\\server\share"
            )));
            // A plain drive path is Prefix::Disk, not VerbatimDisk; it is
            // accepted by acl_path_is_safe via the is_absolute branch instead.
            assert!(!is_verbatim_disk_path(std::path::Path::new(r"C:\plain")));
        }

        #[test]
        fn quote_arg_empty_string_is_double_quoted() {
            assert_eq!(quote_arg(""), "\"\"");
        }

        #[test]
        fn quote_arg_space_is_quoted() {
            assert_eq!(quote_arg("echo hi"), "\"echo hi\"");
        }

        #[test]
        fn quote_arg_embedded_quote_is_escaped() {
            assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
        }

        #[test]
        fn quote_arg_backslash_before_quote_doubled() {
            assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"");
        }

        #[test]
        fn quote_arg_trailing_backslash_with_quoting_is_doubled() {
            assert_eq!(quote_arg("a \\"), "\"a \\\\\"");
        }

        #[test]
        fn quote_arg_trailing_backslash_without_special_chars_passes_through() {
            assert_eq!(quote_arg("a\\"), "a\\");
        }

        #[test]
        fn quote_arg_only_quote_char() {
            assert_eq!(quote_arg("\""), "\"\\\"\"");
        }

        #[test]
        fn quote_arg_multiple_trailing_backslashes_doubled() {
            // Three trailing backslashes inside a quoted arg → six (each doubled).
            assert_eq!(quote_arg("a \\\\\\"), "\"a \\\\\\\\\\\\\"");
        }

        #[test]
        fn quote_arg_backslashes_before_internal_quote() {
            // Two backslashes followed by a quote: `\\"` → in output, the
            // backslashes count is doubled then a `\\"` is emitted as escape.
            // Input: \\"  → Output: "\\\\\""  (i.e. \\\" with one outer quote pair)
            assert_eq!(quote_arg("\\\\\""), "\"\\\\\\\\\\\"\"");
        }

        // ---------- build_env_block ----------

        #[test]
        fn build_env_block_empty_is_just_double_null() {
            let block = build_env_block(&[]).unwrap();
            assert_eq!(block, vec![0u16, 0u16]);
        }

        #[test]
        fn build_env_block_single_pair_has_double_null_terminator() {
            let block = build_env_block(&[("A".to_string(), "1".to_string())]).unwrap();
            assert_eq!(block, vec![b'A' as u16, b'=' as u16, b'1' as u16, 0, 0]);
        }

        #[test]
        fn build_env_block_sorts_alphabetically() {
            let block = build_env_block(&[
                ("Z".to_string(), "z".to_string()),
                ("A".to_string(), "a".to_string()),
                ("M".to_string(), "m".to_string()),
            ])
            .unwrap();
            let expected: Vec<u16> = "A=a\0M=m\0Z=z\0\0".encode_utf16().collect();
            assert_eq!(block, expected);
        }

        #[test]
        fn build_env_block_case_insensitive_dedup_last_wins() {
            let block = build_env_block(&[
                ("PATH".to_string(), "first".to_string()),
                ("path".to_string(), "second".to_string()),
            ])
            .unwrap();
            let expected: Vec<u16> = "path=second\0\0".encode_utf16().collect();
            assert_eq!(block, expected);
        }

        #[test]
        fn build_env_block_rejects_eq_in_key() {
            let err = build_env_block(&[("BAD=KEY".to_string(), "v".to_string())]).unwrap_err();
            assert!(matches!(err, SandboxError::ExecFailed(_)));
        }

        #[test]
        fn build_env_block_rejects_nul_in_value() {
            let err = build_env_block(&[("K".to_string(), "v\0w".to_string())]).unwrap_err();
            assert!(matches!(err, SandboxError::ExecFailed(_)));
        }

        #[test]
        fn build_env_block_rejects_empty_key() {
            let err = build_env_block(&[("".to_string(), "v".to_string())]).unwrap_err();
            assert!(matches!(err, SandboxError::ExecFailed(_)));
        }

        #[test]
        fn build_env_block_rejects_lf_in_key() {
            let err = build_env_block(&[("PATH\n".to_string(), "v".to_string())]).unwrap_err();
            assert!(matches!(err, SandboxError::ExecFailed(_)));
        }

        #[test]
        fn build_env_block_rejects_tab_in_key() {
            let err = build_env_block(&[("KEY\tNAME".to_string(), "v".to_string())]).unwrap_err();
            assert!(matches!(err, SandboxError::ExecFailed(_)));
        }

        #[test]
        fn build_env_block_rejects_lf_in_path_value() {
            let err = build_env_block(&[("PATH".to_string(), "C:\\foo\nC:\\evil".to_string())])
                .unwrap_err();
            assert!(matches!(err, SandboxError::ExecFailed(_)));
        }

        #[test]
        fn build_env_block_allows_lf_in_non_security_value() {
            // Non-security keys CAN carry newlines (some tools pass
            // formatted multiline messages via env). Only PATH / COMSPEC /
            // PATHEXT / SYSTEMROOT / WINDIR reject them.
            let block = build_env_block(&[("LOG_MESSAGE".to_string(), "line1\nline2".to_string())])
                .unwrap();
            // 13 chars + 1 NUL + 1 terminator NUL = 15 u16s
            assert!(!block.is_empty());
        }

        // ---------- resolve_program ----------

        #[test]
        fn resolve_program_allowlisted_shell_resolves_to_system32() {
            let w = resolve_program("cmd.exe").unwrap();
            let s = String::from_utf16(&w[..w.len() - 1]).unwrap();
            assert!(
                s.to_ascii_lowercase().ends_with("\\system32\\cmd.exe"),
                "expected system32-rooted path, got {s}"
            );
            assert!(std::path::Path::new(&s).exists());
        }

        #[test]
        fn resolve_program_allowlisted_shell_without_exe_extension_resolves() {
            let w = resolve_program("cmd").unwrap();
            let s = String::from_utf16(&w[..w.len() - 1]).unwrap();
            assert!(
                s.to_ascii_lowercase().ends_with("\\system32\\cmd.exe"),
                "expected system32-rooted cmd.exe, got {s}"
            );
        }

        #[test]
        fn resolve_program_bare_name_outside_allowlist_rejected() {
            let err = resolve_program("notepad.exe").unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains("not a recognized") && msg.contains("Pass the absolute path"),
                "expected unrecognized-shell rejection, got {msg}"
            );
        }

        #[test]
        fn classify_bare_shell_buckets() {
            assert_eq!(classify_bare_shell("cmd"), Some(BareShell::Cmd));
            assert_eq!(classify_bare_shell("CMD.EXE"), Some(BareShell::Cmd));
            assert_eq!(
                classify_bare_shell("powershell"),
                Some(BareShell::PowerShell)
            );
            assert_eq!(classify_bare_shell("pwsh.exe"), Some(BareShell::PowerShell));
            assert_eq!(classify_bare_shell("bash"), Some(BareShell::Unsupported));
            assert_eq!(classify_bare_shell("sh.exe"), Some(BareShell::Unsupported));
            assert_eq!(classify_bare_shell("notepad.exe"), None);
        }

        #[test]
        fn resolve_program_bare_powershell_rejected_with_actionable_message() {
            // #323/#324: bare powershell/pwsh used to be pinned to System32
            // (wrong path → cryptic 0x2) and would fail to load under the
            // Low-IL token anyway (0xC0000135). Now rejected up front with a
            // message that names the real locations and the cause.
            for shell in ["powershell", "powershell.exe", "pwsh", "pwsh.exe"] {
                let err = resolve_program(shell).unwrap_err();
                let msg = format!("{err:?}");
                assert!(
                    msg.contains("PowerShell is not supported") && msg.contains("0xC0000135"),
                    "expected actionable PowerShell rejection for {shell}, got {msg}"
                );
            }
        }

        #[test]
        fn resolve_program_bare_bash_rejected_with_actionable_message() {
            // #324: git-bash/busybox cannot load under the sandbox token.
            for shell in ["bash", "bash.exe", "sh", "sh.exe"] {
                let err = resolve_program(shell).unwrap_err();
                let msg = format!("{err:?}");
                assert!(
                    msg.contains("not supported under the Windows AppContainer sandbox")
                        && msg.contains("0xC0000135"),
                    "expected actionable bash rejection for {shell}, got {msg}"
                );
            }
        }

        #[test]
        fn resolve_program_absolute_path_existing_returns_widened() {
            let path = "C:\\Windows\\System32\\cmd.exe";
            let w = resolve_program(path).unwrap();
            let s = String::from_utf16(&w[..w.len() - 1]).unwrap();
            assert_eq!(s, path);
        }

        #[test]
        fn resolve_program_absolute_path_missing_rejected() {
            let err = resolve_program("C:\\does\\not\\exist\\nope-xyzzy.exe").unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains("does not exist"),
                "expected does-not-exist rejection, got {msg}"
            );
        }

        #[test]
        fn resolve_program_empty_rejected() {
            let err = resolve_program("").unwrap_err();
            assert!(matches!(err, SandboxError::ExecFailed(_)));
        }

        #[test]
        fn resolve_program_unc_path_rejected() {
            let err = resolve_program("\\\\evil.com\\share\\cmd.exe").unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains("UNC or device path"),
                "expected UNC rejection, got {msg}"
            );
        }

        #[test]
        fn resolve_program_device_path_rejected() {
            let err = resolve_program("\\\\?\\C:\\Windows\\System32\\cmd.exe").unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains("UNC or device path"),
                "expected device-path rejection, got {msg}"
            );
        }

        #[test]
        fn resolve_program_dos_device_path_rejected() {
            let err = resolve_program("\\\\.\\PhysicalDrive0").unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains("UNC or device path"),
                "expected DOS-device rejection, got {msg}"
            );
        }

        #[test]
        fn resolve_program_directory_rejected() {
            let err = resolve_program("C:\\Windows").unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains("is a directory"),
                "expected directory rejection, got {msg}"
            );
        }

        // ---------- is_trace_safe_env_key ----------

        #[test]
        fn is_trace_safe_recognizes_windows_essentials_and_rejects_others() {
            assert!(is_trace_safe_env_key("PATH"));
            assert!(is_trace_safe_env_key("path"));
            assert!(is_trace_safe_env_key("USERPROFILE"));
            assert!(!is_trace_safe_env_key("AWS_SECRET_ACCESS_KEY"));
            assert!(!is_trace_safe_env_key("OPENAI_API_KEY"));
            assert!(!is_trace_safe_env_key("GITHUB_TOKEN"));
        }

        // ---------- backend behavior ----------

        #[tokio::test]
        async fn allow_hosts_rejected() {
            let b = AppContainerBackend::new();
            let m = SandboxManifest {
                network: NetworkPolicy::AllowHosts(vec!["example.com".into()]),
                ..Default::default()
            };
            let err = b
                .execute(
                    &m,
                    SandboxCommand {
                        argv: vec!["cmd.exe".into()],
                        cwd: None,
                    },
                )
                .await
                .unwrap_err();
            assert!(matches!(err, SandboxError::PolicyNotSupported(_)));
        }

        /// Gated live test — only runs on Windows hosts that have AppContainer
        /// enabled AND the operator has opted in. CI Windows runners set the
        /// env var so the matrix exercises this path.
        #[tokio::test]
        async fn echo_runs_live() {
            if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() {
                return;
            }
            let b = AppContainerBackend::new();
            if !b.is_available() {
                eprintln!("skip: AppContainer not available on this host");
                return;
            }
            let m = SandboxManifest {
                max_memory_bytes: Some(256 * 1024 * 1024),
                max_cpu_secs: Some(10),
                timeout: Some(Duration::from_secs(10)),
                ..Default::default()
            };
            let out = b
                .execute(
                    &m,
                    SandboxCommand {
                        argv: vec!["cmd.exe".into(), "/c".into(), "echo hi".into()],
                        cwd: None,
                    },
                )
                .await
                .unwrap();
            assert_eq!(out.exit_code, 0);
            assert!(matches!(
                out.resource_limits,
                ResourceLimitEnforcement::Enforced
            ));
            assert!(String::from_utf8_lossy(&out.stdout).contains("hi"));
        }

        // Live integrity-boundary verification lives in
        // `crates/wcore-sandbox/tests/live_integrity.rs` because it needs
        // to invoke a sibling binary target (`il_probe`) via
        // `CARGO_BIN_EXE_il_probe`, which is only set for INTEGRATION
        // tests. The integration test spawns `il_probe.exe` through this
        // backend and asserts the printed integrity level is `Low` —
        // proof at the OS layer that the explicit `SetTokenInformation`
        // call actually pinned the child below Medium.
    }
}

#[cfg(not(windows))]
mod stub_impl {
    //! Non-Windows compile-stub. NOT a deferral — the real backend lives in
    //! the `#[cfg(windows)]` module above. This stub exists so the crate
    //! compiles + unit-tests on macOS/Linux dev machines, mirroring the
    //! pattern bwrap/sandbox-exec use for their own foreign platforms.

    use super::super::SandboxBackend;
    use crate::error::{Result, SandboxError};
    use crate::manifest::SandboxManifest;
    use crate::{SandboxCommand, SandboxOutput};
    use async_trait::async_trait;

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

    #[async_trait]
    impl SandboxBackend for AppContainerBackend {
        fn name(&self) -> &'static str {
            "appcontainer_stub"
        }
        fn is_available(&self) -> bool {
            false
        }
        async fn execute(
            &self,
            _manifest: &SandboxManifest,
            _cmd: SandboxCommand,
        ) -> Result<SandboxOutput> {
            Err(SandboxError::ExecFailed(
                "AppContainer backend is Windows-only; this host runs the cfg(not(windows)) stub"
                    .into(),
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn stub_is_unavailable() {
            let b = AppContainerBackend::new();
            assert!(!b.is_available());
            let res = b
                .execute(
                    &SandboxManifest::default(),
                    SandboxCommand {
                        argv: vec!["foo".into()],
                        cwd: None,
                    },
                )
                .await;
            assert!(res.is_err());
        }

        #[tokio::test]
        async fn stub_name() {
            let b = AppContainerBackend::new();
            assert_eq!(b.name(), "appcontainer_stub");
        }
    }
}

#[cfg(not(windows))]
pub use stub_impl::AppContainerBackend;
#[cfg(windows)]
pub use windows_impl::AppContainerBackend;
