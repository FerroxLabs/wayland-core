//! AppContainer `windows_impl` unit tests (F20-03 Task 1A split).
#![allow(unused_imports)]

use super::command::*;
use super::handles::*;
use super::process::*;
use super::*;

use super::*;

// Compile-debt restore: the `#[cfg(test)]` bodies below reference these types
// by name but the glob imports above do not re-export them (a `use` is private
// to its module). Import them from their real crate module paths so this module
// builds on msvc. `#![allow(unused_imports)]` above tolerates any that a given
// case does not touch.
use crate::ResourceLimitEnforcement;
use crate::SandboxCommand;
use crate::SandboxOutput;
use crate::error::SandboxError;
use crate::manifest::{NetworkPolicy, SandboxManifest};
use std::sync::Arc;
use std::time::Duration;
// Trait-in-scope import, NOT a type import: `execute` and `is_available` are
// `SandboxBackend` trait methods on `AppContainerBackend`, and the calls below
// do not resolve without the trait in scope. Do not remove as "unused".
use crate::backends::SandboxBackend;

#[test]
fn cancellation_guard_is_sticky_unless_disarmed() {
    let cancelled = Arc::new(JobControl::default());
    drop(JobCancellationGuard::new(Arc::clone(&cancelled)));
    assert!(matches!(
        cancelled.ensure_active(),
        Err(SandboxError::Timeout)
    ));

    let active = Arc::new(JobControl::default());
    let mut guard = JobCancellationGuard::new(Arc::clone(&active));
    guard.disarm();
    drop(guard);
    assert!(active.ensure_active().is_ok());
}

// ---------- quote_arg ----------

#[test]
fn quote_arg_no_special_chars_passes_through() {
    assert_eq!(quote_arg("cmd.exe"), "cmd.exe");
    assert_eq!(quote_arg("/c"), "/c");
    assert_eq!(quote_arg("hello"), "hello");
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

// ---------- resolve_cwd: the lpCurrentDirectory contract ----------
//
// These assert the OBSERVABLE VALUE production hands to
// `CreateProcessAsUserW`'s `lpCurrentDirectory`, decoded back from the actual
// UTF-16 buffer -- not that some string helper was called. `live_cwd_verbatim
// .rs` proves the same fix end-to-end against a real child process.

/// Decode the buffer `resolve_cwd` produces, minus its NUL terminator.
fn lp_current_directory(path: &str) -> String {
    let wide = resolve_cwd(Some(std::path::Path::new(path)))
        .expect("an absolute cwd must resolve")
        .expect("Some(cwd) in must yield Some(buffer) out, never a NULL lpCurrentDirectory");
    assert_eq!(
        wide.last().copied(),
        Some(0),
        "lpCurrentDirectory must be NUL-terminated"
    );
    String::from_utf16(&wide[..wide.len() - 1]).expect("test paths are valid UTF-16")
}

#[test]
fn resolve_cwd_strips_verbatim_disk_prefix() {
    // THE REGRESSION. `std::fs::canonicalize` returns this spelling for every
    // local path on Windows, so a canonicalized cwd arrives here verbatim.
    // Passed through unmodified, the command processor reads the leading `\\`
    // as UNC, refuses it as a current directory, and silently substitutes
    // `C:\Windows` -- the child then runs in the wrong directory with no error
    // raised anywhere. Found by frankforges (wayland-core #254).
    assert_eq!(lp_current_directory(r"\\?\C:\work\repo"), r"C:\work\repo");
    assert_eq!(lp_current_directory(r"\\?\D:\"), r"D:\");
}

#[test]
fn resolve_cwd_leaves_every_other_shape_byte_identical() {
    // Already the spelling Win32 wants.
    assert_eq!(lp_current_directory(r"C:\work\repo"), r"C:\work\repo");
    // Verbatim-UNC and plain UNC name REMOTE objects. Stripping their prefix
    // would change WHICH object is named, so the strip must not touch them --
    // this is the negative that keeps the fix from becoming a path-mangler.
    assert_eq!(
        lp_current_directory(r"\\?\UNC\server\share"),
        r"\\?\UNC\server\share"
    );
    assert_eq!(lp_current_directory(r"\\server\share"), r"\\server\share");
}

#[test]
fn resolve_cwd_keeps_the_absolute_and_null_contract() {
    // No cwd requested => NULL lpCurrentDirectory (inherit the parent's).
    assert!(
        resolve_cwd(None).expect("None is not an error").is_none(),
        "absent cwd must stay absent, not become an empty buffer"
    );
    // A relative cwd would resolve against the PARENT's directory; rejected.
    let err = resolve_cwd(Some(std::path::Path::new(r"relative\dir")))
        .expect_err("a relative cwd must be rejected");
    assert!(matches!(err, SandboxError::ExecFailed(_)), "got {err:?}");
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
    let err =
        build_env_block(&[("PATH".to_string(), "C:\\foo\nC:\\evil".to_string())]).unwrap_err();
    assert!(matches!(err, SandboxError::ExecFailed(_)));
}

#[test]
fn build_env_block_allows_lf_in_non_security_value() {
    // Non-security keys CAN carry newlines (some tools pass
    // formatted multiline messages via env). Only PATH / COMSPEC /
    // PATHEXT / SYSTEMROOT / WINDIR reject them.
    let block =
        build_env_block(&[("LOG_MESSAGE".to_string(), "line1\nline2".to_string())]).unwrap();
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

/// Explicit native acceptance test. It is ignored by the ordinary unit
/// suite so a missing opt-in cannot be misreported as a passing spawn.
#[tokio::test]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn echo_runs_live() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1")
    );
    let b = AppContainerBackend::new();
    assert!(b.is_available(), "AppContainer must be available");
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

/// Regression for #520 (dups #453 / #500): a command whose output far
/// exceeds the ~4 KB pipe buffer must be captured in full. Before the
/// concurrent-drain fix the parent waited for the child to exit before
/// reading a byte, so the child blocked in `WriteFile` once the buffer
/// filled — the wait timed out and the drain returned truncated/empty
/// output. It uses the same explicit ignored-test acceptance gate as
/// `echo_runs_live`.
#[tokio::test]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn large_output_survives_live() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1")
    );
    let b = AppContainerBackend::new();
    assert!(b.is_available(), "AppContainer must be available");
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(20)),
        ..Default::default()
    };
    // ~4000 lines * ~32 bytes ≈ 128 KB, far past the 4 KB pipe buffer.
    // On the pre-fix serial drain this deadlocks the child and times out.
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "for /L %i in (1,1,4000) do @echo ABCDEFGHIJKLMNOPQRSTUVWXYZ0123".into(),
                ],
                cwd: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(
        out.stdout.len() > 64 * 1024,
        "captured only {} bytes — pipe drain truncated (#520 regression)",
        out.stdout.len()
    );
}

/// wayland#1082 (LIVE, AppContainer) — **crossing the output cap must TRUNCATE,
/// not discard.**
///
/// `large_output_survives_live` above uses ~128 KB, far UNDER the 8 MiB cap, so
/// it grades the pipe drain and says nothing about the ceiling. This is the
/// ceiling.
///
/// `drain_pipe` already does the right thing: it reserves against the shared
/// budget, KEEPS the partial grant, and signals `exceeded_event` so the waiter
/// tears the job down. The defect is the last step — the caller turned all of
/// that retained head into `Err(OutputLimitExceeded)`, so a command that
/// produced megabytes handed back an error and none of its own output. #1071
/// fixed the same inversion for the shared drain; this is the AppContainer
/// backend's copy of it.
#[tokio::test]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn output_past_the_cap_is_truncated_not_discarded_live() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1")
    );
    let b = AppContainerBackend::new();
    assert!(b.is_available(), "AppContainer must be available");
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(90)),
        ..Default::default()
    };

    // CONTROL FIRST: a small command must come back intact and unmarked. Without
    // it, the assertions below could pass on a backend that truncates
    // everything, which would prove nothing.
    let small = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/c".into(), "echo hello-1082".into()],
                cwd: None,
            },
        )
        .await
        .expect("control: a small command must run");
    let small_out = String::from_utf8_lossy(&small.stdout).into_owned();
    assert!(
        small_out.contains("hello-1082"),
        "control: small output must survive intact: {small_out:?}"
    );
    assert!(
        !small_out.contains("OUTPUT TRUNCATED"),
        "control: a small command must not be marked truncated: {small_out:?}"
    );

    // ~400k lines x 32 bytes = ~12.8 MB, comfortably past the 8 MiB ceiling.
    let started = std::time::Instant::now();
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "for /L %i in (1,1,400000) do @echo ABCDEFGHIJKLMNOPQRSTUVWXYZ0123".into(),
                ],
                cwd: None,
            },
        )
        .await
        .expect("crossing the cap must return the truncated output, not an error");
    let elapsed = started.elapsed();

    let kept = out.stdout.len();
    eprintln!(
        "MEASURED kept={kept} bytes elapsed={elapsed:?} exit={}",
        out.exit_code
    );

    // THE DEFECT: this whole buffer used to be thrown away.
    assert!(
        kept > 1024 * 1024,
        "the bytes that fit must be KEPT, not discarded — got {kept}"
    );
    assert!(
        kept <= crate::backends::BUFFERED_OUTPUT_LIMIT_BYTES + 4096,
        "the cap must still bound host memory — got {kept}"
    );
    let tail = String::from_utf8_lossy(&out.stdout[kept.saturating_sub(512)..]).into_owned();
    assert!(
        tail.contains("OUTPUT TRUNCATED"),
        "the reader must be TOLD the output was cut: {tail:?}"
    );
    assert!(
        tail.contains("STOPPED"),
        "the marker must say the command did not run to completion: {tail:?}"
    );
    // The exceeded_event trip wire already exists on this backend, so crossing
    // the cap must stop the child rather than run out the 90s timeout.
    assert!(
        elapsed < Duration::from_secs(75),
        "crossing the cap must stop the child promptly — took {elapsed:?}"
    );
}

// Live integrity-boundary verification lives in
// `crates/wcore-sandbox/tests/live_integrity.rs` because it needs
// to invoke a sibling binary target (`il_probe`) via
// `CARGO_BIN_EXE_il_probe`, which is only set for INTEGRATION
// tests. The integration test spawns `il_probe.exe` through this
// backend and asserts the printed integrity level is `Low` —
// proof at the OS layer that the explicit `SetTokenInformation`
// call actually pinned the child below Medium.

/// Required Windows live acceptance: an owned process tree under a Job Object
/// is torn down BEFORE workspace cleanup. `KILL_ON_JOB_CLOSE` reaps the whole
/// tree when the last job handle is released, so teardown must precede cleanup.
/// The identity is present and non-skipping (it spawns a real process and
/// attaches a real Job Object, failing if either fails); native process-absence
/// verification is validated on Windows in plan 20-08.
#[test]
fn required_live_job_teardown_precedes_workspace_cleanup() {
    use crate::backends::process_tree::{ProcessTreeGuard, isolate_std};

    use std::os::windows::process::CommandExt;

    let dir = tempfile::tempdir().expect("workspace");
    let marker = dir.path().join("descendant.marker");

    // WHAT THE OLD FORM DID WRONG. It embedded the marker's ABSOLUTE PATH inside
    // an already-quoted argument using doubled quotes
    // (`cmd /c "echo alive> ""C:\...\descendant.marker"" & ..."`) and passed it
    // through `Command::arg`, which applies std's `CommandLineToArgvW` quoting
    // ON TOP of the cmd escaping. Quote parity broke and cmd.exe reported the
    // whole string as a command name:
    //   '"echo alive> ""C:\...marker"" & ping ..."' is not recognized ...
    //
    // THE REPAIR REMOVES THE NESTING RATHER THAN ESCAPING IT HARDER. The child's
    // working directory is the temp directory, so the marker is a BARE relative
    // name with no path and no quotes, and the line is handed to cmd verbatim
    // via `raw_arg` so std does not re-quote it. The polling below still uses the
    // absolute path, unchanged.
    //
    // The detach and hold primitives are the ones `tests/hard_process_
    // containment_windows.rs` measured: `start ""` gives `start` an empty title
    // so it cannot consume the program token, `/b` keeps it in this console,
    // `/d` disables AutoRun, `/s` makes each cmd take everything between its
    // first and last quote literally, and the hold is a BARE `for /L` cmd
    // builtin — every external exe (ping/choice/timeout) exits in ~80 ms under a
    // Low-IL restricted token, and a parenthesized `(for /L ...)` fails to parse
    // under `cmd /d /s /c`. Single `%i`, not batch `%%i`.
    let hold = "for /L %i in (1,1,8000000) do @rem";
    let mut command = std::process::Command::new("cmd");
    command.current_dir(dir.path());
    command.raw_arg(format!(
        "/d /s /c \"start \"\" /b cmd /d /s /c \"echo alive>descendant.marker & {hold}\" & {hold}\""
    ));
    isolate_std(&mut command);
    let mut child = command.spawn().expect("spawn owned process tree");
    let mut guard =
        ProcessTreeGuard::new(Some(child.id())).expect("own the process tree via Job Object");

    let mut ran = false;
    for _ in 0..1000 {
        if marker.exists() {
            ran = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(ran, "owned descendant must run before teardown");

    // Terminal Job Object teardown BEFORE workspace cleanup.
    guard.disarm();
    let _ = child.kill();
    let _ = child.wait();
    // Workspace cleanup runs only after the owned tree is torn down.
    drop(dir);
}

/// ANTI-SWAP REGRESSION PROOF for the Windows retained-cwd binding.
///
/// WHICH MECHANISM THIS PROVES: the OS-enforced NAME PIN established by
/// [`bind_retained_cwd`] — a handle-relative reopen of the retained object whose
/// share mode omits `FILE_SHARE_DELETE`, held for the whole bound execution.
/// `CreateProcess` accepts only a pathname, so the binding is sound only while
/// that pathname cannot be redirected; this test constructs the redirection the
/// guarantee exists to defeat and proves the child still operated on the object
/// the authority retained.
///
/// IT FAILS IF THE BINDING IS EVER DOWNGRADED TO AN UNGUARDED PATHNAME
/// RE-RESOLVE, in two independent ways:
/// - part one: with the bind held, the substitution SUCCEEDS instead of being
///   refused, and the child's artifact is then absent from the retained object;
/// - part two: `execute_with_cwd_authority` stops refusing an authority whose
///   name cannot be pinned, i.e. it spawned without establishing the pin.
///
/// IT ASSERTS THE INVARIANT, NEVER AN ERROR SHAPE. No error code, error kind or
/// numeric OS status appears in any assertion: encoding today's failure shape
/// would enshrine it. What is asserted is that the retained object is what the
/// child worked in.
///
/// It spawns an ordinary child rather than an AppContainer one, so it always
/// runs — it is never skipped by the live-acceptance environment gate, and can
/// therefore never report a vacuous green.
#[tokio::test]
async fn windows_retained_cwd_bind_survives_a_pathname_substitution() {
    use crate::DirectoryAuthority;

    let owner_dir = tempfile::tempdir().expect("owner");
    let workspace = owner_dir.path().join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let decoy = owner_dir.path().join("decoy");
    std::fs::create_dir(&decoy).expect("decoy");

    // The delegated dispatch path retains its checkout observationally; that is
    // the authority this binding receives in production.
    let authority = DirectoryAuthority::open_observational(&workspace).expect("retain workspace");

    // ---- part one: the bind defeats the substitution, and the child lands in
    // ---- the RETAINED object.
    let (lease, bound) = bind_retained_cwd(&authority).expect("pin the retained workspace name");

    // The substitution the guarantee exists to defeat: redirect the bound
    // pathname at a different directory object. Both redirection primitives are
    // attempted — rename the bound name away, and unlink it so a decoy could be
    // recreated in its place.
    let moved = owner_dir.path().join("workspace-moved");
    let renamed = std::fs::rename(&workspace, &moved).is_ok();
    let unlinked = std::fs::remove_dir(&workspace).is_ok();
    assert!(
        !renamed && !unlinked,
        "the bound working-directory name was redirected while the bind was held \
         (renamed={renamed}, unlinked={unlinked}) — the binding is no longer pinned \
         and has degraded to an unguarded pathname re-resolve"
    );

    // Bind by path exactly as the production spawn does, and have the child
    // write a marker into whatever object that pathname reaches.
    let status = std::process::Command::new("cmd")
        .args(["/d", "/s", "/c", "echo bound>marker.txt"])
        .current_dir(&bound)
        .status()
        .expect("spawn the bound child");
    assert!(status.success(), "the bound child must run");

    // THE INVARIANT: the marker is reachable THROUGH THE RETAINED HANDLE, so the
    // child operated on the object the authority retained — not on a substitute
    // installed at the same pathname.
    let retained_entries = authority.child_names().expect("enumerate retained object");
    assert!(
        retained_entries.iter().any(|name| name == "marker.txt"),
        "the child's artifact is absent from the RETAINED object (saw {retained_entries:?}) — \
         the child worked somewhere other than the retained workspace"
    );
    assert!(
        std::fs::read_dir(&decoy)
            .expect("enumerate decoy")
            .next()
            .is_none(),
        "the child wrote into the decoy object — the pathname was redirected"
    );
    drop(lease);

    // ---- part two: the production entry point refuses when the name cannot be
    // ---- pinned, rather than spawning against a re-resolvable pathname.
    //
    // A DELETE-bearing authority cannot be pinned: the lease's share mode would
    // have to permit the delete access that handle was already granted, and it
    // deliberately does not. If `execute_with_cwd_authority` ever stops
    // establishing the pin, this call stops failing.
    let unpinnable =
        DirectoryAuthority::open(&decoy).expect("retain decoy as a mutating authority");
    let refusal = AppContainerBackend::new()
        .execute_with_cwd_authority(
            &SandboxManifest {
                timeout: Some(Duration::from_secs(10)),
                ..Default::default()
            },
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "echo unbound>escaped.txt".into(),
                ],
                cwd: Some(decoy.clone()),
            },
            unpinnable,
        )
        .await;
    assert!(
        refusal.is_err(),
        "an authority whose name cannot be pinned must be refused, not spawned unbound"
    );
    assert!(
        !decoy.join("escaped.txt").exists(),
        "the refused execution still ran a child against the unpinned pathname"
    );
}

// ---------- the readiness seam ----------

/// THE regression for the `--json-stream` `ready` frame.
///
/// `SandboxRegistry::required_for_session` is resolved inside agent bootstrap,
/// BEFORE the `ready` frame is written. It used to decide the Windows backend
/// by running `AppContainerBackend::is_available()` — a real guarded
/// `cmd.exe /c exit 0` through the whole pipeline, bounded by a 15s wall-clock
/// guard because `CreateAppContainerProfile` / `CreateProcessAsUserW` can stall
/// on an AV image scan or a slow profile-service RPC. That guard is longer than
/// the host's ready deadline, so on a first launch that hit the stall the
/// Desktop app got no `ready` frame at all.
///
/// This asserts the wiring, not a helper: it calls the production selector and
/// then reads the production process-global probe cache. If selection probes
/// again, the cache is populated and this goes red.
///
/// The cache is process-global and `nextest` runs each test in its own process,
/// so the cold precondition is real. The final block is the anti-vacuity
/// control: it drives the SAME cache through the production `is_available()` to
/// prove the observation would have caught a probe if one had happened.
#[test]
fn session_selection_reaches_ready_without_running_the_appcontainer_probe() {
    // `WAYLAND_SANDBOX` would route selection down the docker / refusal arms
    // instead of the platform cascade under test.
    assert!(
        std::env::var("WAYLAND_SANDBOX").is_err(),
        "this test drives the default platform cascade; WAYLAND_SANDBOX must be unset"
    );
    assert_eq!(
        settled_verdict(),
        None,
        "precondition: the probe cache must be cold at the start of this process — \
         run this test with `cargo nextest run` (process per test)"
    );

    let registry =
        crate::SandboxRegistry::required_for_session(None).expect("session selection must resolve");

    // The Windows default is the relaxed Job Object backend; AppContainer stays
    // reachable only through `WAYLAND_SANDBOX=appcontainer`, which this test
    // asserts is unset. What matters here is that selection resolves to a REAL
    // backend rather than the fail-closed placeholder, and does so without
    // touching the probe.
    assert_eq!(
        registry.backend_name(),
        "windows_job_object",
        "the session must take the real Windows backend, not a fail-closed placeholder"
    );
    assert_eq!(
        settled_verdict(),
        None,
        "session selection ran the AppContainer real-spawn probe — that is the 15s guard \
         sitting on the `ready` path this fix removed"
    );

    // Anti-vacuity: the verdict IS reachable through the production path, so
    // "still None above" is an observation, not an inability to observe.
    // Driven off AppContainer directly, because the session no longer selects
    // it and `registry.is_available()` would now settle nothing.
    let available = AppContainerBackend::new().is_available();
    assert_eq!(
        settled_verdict(),
        Some(available),
        "the production availability path must settle the same cache this test read"
    );

    // …and the containment predicates are driven by that same settled verdict,
    // not by a constant. Whichever way this host probed, the claims must agree
    // with it — a hardcoded `true` disagrees on a host whose probe fails.
    let backend = AppContainerBackend::new();
    println!("OBSERVED: AppContainer probed available={available} on this host");
    assert_eq!(
        backend.enforces_read_deny(),
        available,
        "the read-deny claim must track the settled verdict"
    );
    assert_eq!(
        backend.binds_cwd_authority(),
        available,
        "the cwd-authority claim must track the settled verdict"
    );
    assert_eq!(
        backend.owns_descendants_hard(),
        available,
        "the descendant-ownership claim must track the settled verdict"
    );
}

/// Both arms of the containment claim, reachable on ANY host because the
/// mapping is pure in the verdict.
///
/// "Unknown" must still claim: a session that has not run a command yet has
/// learned nothing about this host, and withdrawing on no evidence would make
/// every fresh process report itself uncontained. Only a settled negative
/// withdraws — and because `ProbeCache::settled` keeps a negative until a probe
/// succeeds, that answer cannot flip back on an unchanged machine.
#[test]
fn the_containment_claim_is_withdrawn_only_by_a_settled_negative_verdict() {
    assert!(
        containment_claim(None),
        "an unprobed backend must not withdraw its claim on no evidence"
    );
    assert!(containment_claim(Some(true)));
    assert!(
        !containment_claim(Some(false)),
        "a settled-unavailable backend must withdraw the containment claim"
    );
}

// ---------- the refusal must say WHY ----------

/// The refusal has to carry the probe's own cause, not point at a log.
///
/// It used to read "the cause was logged by `probe_appcontainer_available`".
/// That log is a `tracing::error!`, and CI installs no subscriber at that
/// level, so the sentence was an assertion that evidence existed followed by a
/// refusal to produce it. Both self-hosted Windows runners refuse to sandbox
/// and this is why nobody could say what either of them actually hit.
///
/// Pure in the cause, so both arms are reachable from a host whose AppContainer
/// works fine.
#[test]
fn the_refusal_names_the_failing_call_instead_of_pointing_at_a_log() {
    let with_cause = compose_unavailable_refusal(Some("CreateProcessAsUserW: 0x5"));
    assert!(
        with_cause.contains("CreateProcessAsUserW: 0x5"),
        "the refusal dropped the one string that identifies the failure: {with_cause}"
    );
    assert!(
        with_cause.contains("sandbox UNAVAILABLE and unsandboxed execution is not permitted"),
        "the fail-closed sentence other surfaces assert on must survive: {with_cause}"
    );
    assert!(
        !with_cause.contains("the cause was logged"),
        "the refusal still defers to a log the reader cannot see: {with_cause}"
    );

    // The no-cause arm must read as a bug in our bookkeeping, NOT as a fact
    // about the operator's machine — the distinction decides whether they go
    // change Windows policy or file against us.
    let without = compose_unavailable_refusal(None);
    assert!(
        without.contains("No cause was recorded"),
        "the unrecorded-cause arm must say so plainly: {without}"
    );
    assert!(
        without.contains("defect in the probe"),
        "an unrecorded cause must be attributed to us, not to the host: {without}"
    );
    assert_ne!(
        with_cause, without,
        "the two arms must be distinguishable, otherwise the cause is decorative"
    );
}

/// A transient probe failure must NOT hard-refuse the user's command.
///
/// This is the RC defect. Every self-hosted Windows CI failure was one
/// sentence — "sandbox UNAVAILABLE … the AppContainer real-spawn probe failed"
/// — across `wcore-cli` and `wcore-swarm` alike, and it was never the host:
/// the same box probes available as a user, as NetworkService, from `C:`, and
/// across 40 concurrent separate-process cold probes. CI's own retries then
/// passed, which is what a lost profile-creation race looks like.
#[test]
fn a_transient_probe_failure_is_retried_rather_than_refusing_the_command() {
    let mut calls = 0;
    let mut slept = Vec::new();
    let available = probe_with_retry(
        || {
            calls += 1;
            if calls < 3 {
                ProbeAttempt::Failed {
                    reason: "CreateAppContainerProfile: 0x800705aa".to_owned(),
                    retryable: true,
                }
            } else {
                ProbeAttempt::Available
            }
        },
        |d| slept.push(d),
    );
    assert!(
        available,
        "two transient failures then success must resolve to AVAILABLE — refusing here \
         is precisely the CI defect"
    );
    assert_eq!(
        calls, 3,
        "it must actually re-attempt, not return the first answer"
    );
    assert_eq!(
        slept.len(),
        2,
        "each retry must back off before re-attempting"
    );
    assert!(
        slept[1] > slept[0],
        "backoff must grow, so a busy host is not hammered: {slept:?}"
    );
}

/// Fail-closed is preserved: retries do not invent availability.
#[test]
fn a_persistent_failure_still_refuses_after_the_attempts_are_spent() {
    let mut calls = 0;
    let available = probe_with_retry(
        || {
            calls += 1;
            ProbeAttempt::Failed {
                reason: "CreateProcessAsUserW: 0x5".to_owned(),
                retryable: true,
            }
        },
        |_| {},
    );
    assert!(
        !available,
        "a host that genuinely cannot sandbox must still be refused — retry must never \
         become a bypass"
    );
    assert_eq!(
        calls, PROBE_ATTEMPTS,
        "it must spend exactly the budgeted attempts"
    );
    let refusal = compose_unavailable_refusal(Some("CreateProcessAsUserW: 0x5"));
    assert!(refusal.contains("CreateProcessAsUserW: 0x5"));
}

/// A STALL must not be retried — #125 was a ~120s hang per command, and three
/// stacked 15s guards would triple it while curing nothing.
#[test]
fn a_stalled_probe_is_not_retried_because_that_multiplies_the_hang() {
    let mut calls = 0;
    let mut slept = Vec::new();
    let available = probe_with_retry(
        || {
            calls += 1;
            ProbeAttempt::Failed {
                reason: "the probe exceeded its 15s hard wall-clock guard".to_owned(),
                retryable: false,
            }
        },
        |d| slept.push(d),
    );
    assert!(!available);
    assert_eq!(
        calls, 1,
        "a wall-clock stall must be answered once, not re-attempted — retrying a wedged \
         host is the #125 hang multiplied"
    );
    assert!(
        slept.is_empty(),
        "a non-retryable outcome must not sleep at all"
    );
}

/// Drives the REAL production probe and prints whatever this host reports.
///
/// This is the diagnostic that closes the open Windows question. It is not a
/// pass/fail gate on the sandbox working — a host that cannot sandbox is
/// allowed, and this test still passes there. What it does NOT allow is a
/// failure with no recorded cause, which is the state that left both runners
/// undiagnosable.
///
/// The `OBSERVED:` line follows the convention above so the cause lands in the
/// CI log of every Windows leg, on every runner, without anyone attaching a
/// debugger or installing a tracing subscriber.
#[tokio::test]
async fn a_failed_probe_records_a_cause_the_operator_can_actually_read() {
    // Drive the PRODUCTION path. `execute` runs the guarded probe through
    // `spawn_blocking` itself, so this is what an operator's first command does
    // — not a hand-rolled probe call that could diverge from it.
    let backend = AppContainerBackend::new();
    let outcome = backend
        .execute(
            &SandboxManifest::default(),
            SandboxCommand {
                argv: vec![
                    "cmd.exe".to_string(),
                    "/c".to_string(),
                    "exit 0".to_string(),
                ],
                cwd: None,
            },
        )
        .await;

    let available = settled_verdict();
    println!("OBSERVED: AppContainer settled verdict on this host: {available:?}");

    match outcome {
        Ok(out) => {
            // This host sandboxes fine. Nothing to diagnose; assert only that
            // we did not somehow refuse-and-succeed.
            println!(
                "OBSERVED: AppContainer executed normally, exit_code={}",
                out.exit_code
            );
        }
        Err(err) => {
            let text = err.to_string();
            println!("OBSERVED: AppContainer refusal text: {text}");

            // Only the availability refusal carries a probe cause; a different
            // execution error (bad path, denied cwd) is not this contract.
            if text.contains("sandbox UNAVAILABLE") {
                assert!(
                    !text.contains("No cause was recorded"),
                    "the probe failed but recorded no cause — exactly the undiagnosable \
                     state this change exists to remove: {text}"
                );
                assert!(
                    text.contains("Cause, verbatim from the probe:"),
                    "a failed probe must hand the operator its cause: {text}"
                );
                assert!(
                    !text.contains("the cause was logged"),
                    "the refusal still defers to a log CI never emits: {text}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// W-B: a post-execution cleanup fault must not be reported as an execution
// failure.
//
// The join at the bottom of `execute_blocking` used to be
// `(_, Err(cleanup_error)) => Err(cleanup_error)`, which discarded a successful
// `SandboxOutput` — exit code, stdout and stderr — whenever the AppContainer
// ACL/profile teardown faulted. Under machine-wide mutation-lock contention
// that is the observed production behaviour: the child had ALREADY written its
// files, and the caller was told the command failed. An agent believes that
// report and re-issues the command; for a non-idempotent command the retry is
// real damage.
//
// These cases pin the join itself, so the contract is assertable without
// standing up contention. The live counterpart, which drives a real child, a
// real file on disk and a real mutation-lock timeout, is
// `cleanup_timeout_reports_the_completed_command_as_completed` below.
// ---------------------------------------------------------------------------

fn cleanup_fault() -> SandboxError {
    SandboxError::ExecFailed("timed out acquiring AppContainer ACL mutation lock".into())
}

fn succeeded() -> SandboxOutput {
    SandboxOutput {
        exit_code: 0,
        stdout: b"side-effect-landed".to_vec(),
        stderr: b"child-stderr".to_vec(),
        resource_limits: ResourceLimitEnforcement::Enforced,
    }
}

#[test]
fn cleanup_fault_on_success_reports_success_and_the_true_exit_code() {
    let joined = join_execution_and_cleanup(Ok(succeeded()), Err(cleanup_fault()))
        .expect("a teardown fault must not turn a completed command into an execution failure");
    assert_eq!(
        joined.exit_code, 0,
        "the child's real exit code must survive"
    );
    assert_eq!(
        joined.stdout, b"side-effect-landed",
        "the child's real stdout must survive"
    );
}

#[test]
fn cleanup_fault_on_success_surfaces_the_fault_distinctly() {
    let joined = join_execution_and_cleanup(Ok(succeeded()), Err(cleanup_fault())).unwrap();
    let stderr = String::from_utf8(joined.stderr).unwrap();
    assert!(
        stderr.starts_with("child-stderr"),
        "the child's own stderr must be preserved ahead of the annotation: {stderr:?}"
    );
    assert!(
        stderr.contains(CLEANUP_FAULT_PREFIX),
        "the teardown fault must be surfaced as a teardown fault, not swallowed: {stderr:?}"
    );
    assert!(
        stderr.contains("timed out acquiring AppContainer ACL mutation lock"),
        "the operator must get the real cleanup cause: {stderr:?}"
    );
    assert!(
        stderr.contains("do NOT retry"),
        "the annotation exists to stop a retry of an already-applied command: {stderr:?}"
    );
}

#[test]
fn a_non_zero_exit_still_reports_its_own_code_when_cleanup_faults() {
    let ran = SandboxOutput {
        exit_code: 3,
        stdout: Vec::new(),
        stderr: Vec::new(),
        resource_limits: ResourceLimitEnforcement::Enforced,
    };
    let joined = join_execution_and_cleanup(Ok(ran), Err(cleanup_fault())).unwrap();
    assert_eq!(
        joined.exit_code, 3,
        "a command that ran and failed on its own terms must keep its exit code"
    );
}

#[test]
fn a_real_execution_failure_keeps_its_typed_variant_when_cleanup_also_faults() {
    // Callers match on `Timeout` / `OutputLimitExceeded`. Substituting the
    // teardown fault (the old behaviour) made those variants unreachable
    // whenever teardown faulted too.
    let joined = join_execution_and_cleanup(Err(SandboxError::Timeout), Err(cleanup_fault()));
    assert!(
        matches!(joined, Err(SandboxError::Timeout)),
        "the execution failure is the cause and must not be replaced by its consequence: \
         {joined:?}"
    );
}

#[test]
fn a_clean_teardown_passes_the_execution_result_through_untouched() {
    let joined = join_execution_and_cleanup(Ok(succeeded()), Ok(())).unwrap();
    assert_eq!(
        joined.stderr, b"child-stderr",
        "no annotation without a fault"
    );
    assert!(matches!(
        join_execution_and_cleanup(Err(SandboxError::Timeout), Ok(())),
        Err(SandboxError::Timeout)
    ));
}

/// W-B, live on real hardware, through the real backend, with a real side
/// effect graded from bytes on disk OUTSIDE the sandbox.
///
/// The lever is the one the field report named: a second process holding the
/// machine-wide `Global\WaylandCore.AppContainerAclLease.v1.<sha>` mutex. The
/// helper waits for a rendezvous file that the SANDBOXED CHILD itself writes,
/// so the mutex is free while the identity is being set up and taken by the
/// time teardown runs — which is precisely the window in which the product used
/// to report "Failed to execute command" for a command that had already
/// written its file.
///
/// Grading:
///   * the marker file must exist with the expected bytes (world state, read by
///     this process, not by the sandboxed child);
///   * the call must return `Ok` with the child's real exit code;
///   * stderr must carry the cleanup fault, distinctly labelled.
///
/// Against the pre-fix join this FAILS at the first assertion with the teardown
/// error substituted for the result.
///
/// Residual, deliberately not hidden: a timed-out teardown strands one ACL
/// grant and one AppContainer profile whose lease stays `GrantActive`. That is
/// pre-existing behaviour of the timeout path, not something this test or this
/// fix introduces, and it is what the lease-recovery sweep exists for.
#[tokio::test]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn cleanup_timeout_reports_the_completed_command_as_completed() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1")
    );
    let backend = AppContainerBackend::new();
    assert!(backend.is_available(), "AppContainer must be available");

    // %PUBLIC% for the same reason the rest of the live ACL suite uses it: a
    // shallow, AppContainer-traversable ancestor chain, writable unelevated.
    let root = std::path::PathBuf::from(std::env::var_os("PUBLIC").expect("PUBLIC"))
        .join(format!("wcore-wb-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let side_effect = root.join("side-effect.txt");
    let go = root.join("child-running.txt");
    let held = root.join("mutex-held.txt");
    let release = root.join("release.txt");

    // Substring filter, NOT `--exact`: `--exact` matches the fully-qualified
    // module path, so the bare name selects zero tests and the helper exits
    // having acquired nothing.
    let mut helper = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["mutation_lock_helper_entry", "--nocapture"])
        .env("WCORE_MUTEX_HELPER_MARKER", &held)
        .env("WCORE_MUTEX_HELPER_GO", &go)
        .env("WCORE_MUTEX_HELPER_RELEASE", &release)
        .spawn()
        .expect("spawn mutation-lock holder");

    // Rendezvous, not sleeps: the child may only exit once the helper has
    // actually taken the mutex, so the teardown timeout is forced rather than
    // hoped for. (`timeout /t` is unusable here — it refuses a redirected
    // stdin — and `ping` needs network DLLs an AppContainer child cannot map.)
    let finish = root.join("finish.txt");
    let watcher = {
        let (held, finish) = (held.clone(), finish.clone());
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            while !held.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            std::fs::write(&finish, b"go").ok();
        })
    };

    // Child: land the side effect, release the helper onto the mutex, then spin
    // until the watcher confirms the mutex is held.
    // `for /L` with step 0 is the cmd command-line spin idiom; `goto`/labels
    // need a batch context and do not exist under `cmd /c`.
    let script = format!(
        "echo LANDED>\"{}\" & echo go>\"{}\" & for /L %i in (1,0,2) do @if exist \"{}\" exit 0",
        side_effect.display(),
        go.display(),
        finish.display()
    );
    let manifest = SandboxManifest {
        fs_write_allow: vec![root.clone()],
        // Comfortably past the child's ~8s idle; the 15s teardown timeout is
        // separate and is what this test is actually driving.
        timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    };
    let outcome = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/c".into(), script],
                cwd: None,
            },
        )
        .await;

    std::fs::write(&release, b"go").ok();
    let _ = helper.wait();
    let _ = watcher.join();

    // World state FIRST, so the grade never depends on the product's own
    // report of itself.
    let landed = std::fs::read(&side_effect);
    println!(
        "OBSERVED side_effect={} go={} held={} outcome={:?}",
        landed.is_ok(),
        go.exists(),
        held.exists(),
        outcome.as_ref().map(|o| o.exit_code)
    );
    assert!(
        held.exists(),
        "the contention lever never engaged — the helper never took the mutex, \
         so this run proves nothing about the teardown path"
    );
    let landed = landed.expect("the sandboxed child must have written its side effect");
    assert!(
        String::from_utf8_lossy(&landed).contains("LANDED"),
        "side effect on disk is {landed:?}"
    );

    let out = outcome.expect(
        "the command RAN and its side effect is on disk; reporting an execution failure here is \
         the defect — an agent retries a non-idempotent command on this report",
    );
    assert_eq!(out.exit_code, 0, "the child's real exit code must survive");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains(CLEANUP_FAULT_PREFIX),
        "the teardown fault must still be surfaced, distinctly: {stderr:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}
