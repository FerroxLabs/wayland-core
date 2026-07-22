//! Live retained-directory confinement (macOS sandbox-exec backend).
//!
//! This is the macOS counterpart to the Windows `live_integrity.rs`
//! retained-directory boundary proof and the Linux
//! `bwrap_confines_filesystem_writes_outside_allowlist` check in
//! `backend_integration.rs`, adapted to the sandbox-exec (SBPL deny-default)
//! backend.
//!
//! "Retained directory" is the quarantined working directory the sandbox is
//! permitted to write into — bound here via `SandboxManifest::fs_write_allow`.
//! The proof is a matched pair, matching the shape of the other platforms:
//!   * a write INSIDE the retained directory succeeds and lands on disk
//!     (the sandbox is loose enough to do real work), and
//!   * a write OUTSIDE the retained directory is denied by the deny-default
//!     SBPL profile and never reaches the host filesystem (the boundary is
//!     tight enough to confine escapes).
//!
//! Whole-file `#![cfg(target_os = "macos")]` gating mirrors `live_integrity.rs`
//! (`#![cfg(windows)]`): on other platforms the file compiles to zero tests.
//! The `WAYLAND_SANDBOX_LIVE_MACOS` env opt-in mirrors the
//! `WAYLAND_SANDBOX_LIVE_WINDOWS` gate in `live_integrity.rs`: the test only
//! self-qualifies when the Phase 20 acceptance harness has opted the host into
//! live macOS execution. `is_available()` is a secondary guard so a host
//! without a working sandbox-exec engine skips cleanly rather than failing.

#![cfg(target_os = "macos")]

use std::path::Path;
use std::time::Duration;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::sandbox_exec::SandboxExecBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// Build `/bin/sh -c 'echo <sentinel> > "$1"' -- <target>`.
///
/// The write target is passed as a positional argument (`$1`) rather than
/// interpolated into the script text, so no shell metacharacter in the path is
/// interpreted — the redirection target is exactly `target`.
fn write_command(sentinel: &str, target: &Path) -> SandboxCommand {
    SandboxCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("echo {sentinel} > \"$1\""),
            // $0 — arbitrary program name for the `sh -c` positional slot.
            "wcore-retained".into(),
            // $1 — the write target.
            target.to_string_lossy().into_owned(),
        ],
        cwd: None,
    }
}

/// Retained-directory confinement: the macOS sandbox confines writes to the
/// retained (quarantine) working directory bound via `fs_write_allow`, and
/// denies a write that targets any path outside it.
#[tokio::test]
#[ignore = "live macOS retained-directory acceptance; run via `--run-ignored all` with WAYLAND_SANDBOX_LIVE_MACOS=1"]
async fn required_live_macos_retained_directory_confines_writes() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_MACOS").is_err() {
        eprintln!(
            "skip: WAYLAND_SANDBOX_LIVE_MACOS not set \
             (host has not opted into live macOS execution)"
        );
        return;
    }
    let backend = SandboxExecBackend::new();
    if !backend.is_available() {
        eprintln!("skip: sandbox-exec probe failed on this host");
        return;
    }

    // The retained (quarantine) directory the sandbox is allowed to write to.
    // Canonicalize so the SBPL `(subpath ...)` matches the real path the child
    // writes (macOS symlinks /var -> /private/var, /tmp -> /private/tmp).
    let retained_dir = tempfile::tempdir().expect("create retained directory");
    let retained =
        std::fs::canonicalize(retained_dir.path()).expect("canonicalize retained directory");

    // A directory OUTSIDE the retained root. It exists on the host but is NOT
    // added to the manifest allowlist, so a write into it must be denied by the
    // deny-default profile.
    let outside_dir = tempfile::tempdir().expect("create outside directory");
    let outside =
        std::fs::canonicalize(outside_dir.path()).expect("canonicalize outside directory");

    let manifest = SandboxManifest {
        fs_read_allow: vec![retained.clone()],
        fs_write_allow: vec![retained.clone()],
        timeout: Some(Duration::from_secs(30)),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    // (1) A write INSIDE the retained directory succeeds and lands on disk.
    let kept = retained.join("retained-artifact");
    let inside = backend
        .execute(&manifest, write_command("retained-write-ok", &kept))
        .await
        .expect("sandboxed write into the retained directory must run");
    assert_eq!(
        inside.exit_code,
        0,
        "write into the retained directory must succeed; stderr={:?}",
        String::from_utf8_lossy(&inside.stderr)
    );
    assert!(
        kept.exists(),
        "retained directory must retain the sandboxed write"
    );

    // (2) A write OUTSIDE the retained directory is denied: the child fails and
    // no file appears on the host. The backend itself still returns Ok (the
    // confined child fails, not the spawn) — matching the bwrap confinement
    // test in `backend_integration.rs`.
    let escapee = outside.join("escapee");
    let out = backend
        .execute(&manifest, write_command("escape-attempt", &escapee))
        .await
        .expect("backend must run even though the confined child fails");
    assert_ne!(
        out.exit_code,
        0,
        "a write outside the retained directory must be denied; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !escapee.exists(),
        "an escaping write must never reach the host filesystem"
    );
}
