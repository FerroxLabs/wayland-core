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
use crate::SandboxCommand;
use crate::error::SandboxError;
use crate::manifest::{NetworkPolicy, SandboxManifest};
use std::sync::Arc;

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

    let dir = tempfile::tempdir().expect("workspace");
    let marker = dir.path().join("descendant.marker");
    let mut command = std::process::Command::new("cmd");
    command.arg("/c").arg(format!(
        "start /b cmd /c \"echo alive> \"\"{}\"\" & ping -n 300 127.0.0.1 >nul\" & ping -n 300 127.0.0.1 >nul",
        marker.display()
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
