//! AppContainer command-line, program-resolution and environment helpers (F20-03 Task 1A split of `windows_impl`).
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

use super::handles::*;
use super::process::*;
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

pub(super) fn widen(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(super) fn widen_os(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Windows cmdline-quoting per the MSVC C runtime / `CommandLineToArgvW`
/// round-trip rules (Daniel Colascione's algorithm). Quotes are only
/// added when needed (whitespace, embedded `"`, newline, or empty
/// string); backslashes are doubled when followed by `"` or by the
/// closing quote.
pub(super) fn quote_arg(arg: &str) -> String {
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
pub(super) enum BareShell {
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
pub(super) fn classify_bare_shell(name: &str) -> Option<BareShell> {
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
pub(super) fn is_unc_or_device_path(p: &str) -> bool {
    p.starts_with("\\\\") || p.starts_with("//")
}

/// True only for the Windows VERBATIM DISK form `\\?\X:\...` — an
/// extended-length spelling of an ordinary local drive-letter path.
/// `std::fs::canonicalize` returns this form for EVERY local path on
/// Windows, so the fs-allowlist guard must treat it as local, not as a
/// UNC/device path. Genuine UNC (`\\?\UNC\...`), device (`\\.\...`),
/// and other verbatim (`\\?\...`) prefixes are NOT VerbatimDisk and stay
/// rejected. The OS path parser handles slash/case variants for us.
#[cfg(test)]
pub(super) fn is_verbatim_disk_path(path: &std::path::Path) -> bool {
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
pub(super) fn resolve_program(program: &str) -> Result<Vec<u16>> {
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
pub(super) unsafe fn query_process_integrity_rid(process_handle: HANDLE) -> Result<u32> {
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
    let _ =
        unsafe { GetTokenInformation(token, TokenIntegrityLevel, ptr::null_mut(), 0, &mut needed) };
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
pub(super) const SECURITY_MANDATORY_LOW_RID: u32 = 0x1000;

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
pub(super) const SANDBOX_ACTIVE_PROCESS_LIMIT: u32 = 512;

pub(super) fn system_directory() -> Result<Vec<u16>> {
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
pub(super) fn build_env_block(pairs: &[(String, String)]) -> Result<Vec<u16>> {
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
pub(super) fn is_trace_safe_env_key(k: &str) -> bool {
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
