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

/// Run `argv` under `manifest` and return `(exit_code, stdout)`.
async fn run(
    backend: &SandboxExecBackend,
    manifest: &SandboxManifest,
    argv: &[&str],
) -> (i32, String) {
    let out = backend
        .execute(
            manifest,
            SandboxCommand {
                argv: argv.iter().map(|a| (*a).to_owned()).collect(),
                cwd: None,
            },
        )
        .await
        .expect("backend must run even when the confined child fails");
    (out.exit_code, String::from_utf8_lossy(&out.stdout).into())
}

/// `fs_metadata_read_allow` must buy EXACTLY one thing: the child can `stat`
/// the named file. Not read it, not read its neighbours, not list its
/// directory.
///
/// This is the grant that revives `cargo` on macOS. libgit2 derives the global
/// git config path from `$HOME`, ignores `GIT_CONFIG_GLOBAL`, and treats the
/// EPERM seatbelt returns for an ungranted path as fatal — `failed to stat
/// '<home>/.gitconfig'; class=Config (7)` — so every `cargo new` died there.
/// The grant lets the stat answer while the CONTENTS, which carry the
/// operator's identity and any `[url … insteadOf]` credential rewrite, stay
/// unreadable.
///
/// Written against a synthetic home so it proves the mechanism on any host,
/// including one with no `~/.gitconfig` of its own.
#[tokio::test]
#[ignore = "live macOS metadata-grant acceptance; run via `--run-ignored all` with WAYLAND_SANDBOX_LIVE_MACOS=1"]
async fn required_live_macos_metadata_grant_permits_stat_and_nothing_else() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_MACOS").is_err() {
        eprintln!("skip: WAYLAND_SANDBOX_LIVE_MACOS not set");
        return;
    }
    let backend = SandboxExecBackend::new();
    if !backend.is_available() {
        eprintln!("skip: sandbox-exec probe failed on this host");
        return;
    }

    let home_dir = tempfile::tempdir().expect("create synthetic home");
    let home = std::fs::canonicalize(home_dir.path()).expect("canonicalize synthetic home");
    let granted = home.join(".gitconfig");
    std::fs::write(&granted, "[user]\n\tname = operator\n").expect("write granted file");
    // A NEIGHBOUR in the same directory. Nothing about the grant may reach it.
    let neighbour = home.join(".netrc");
    std::fs::write(&neighbour, "machine example.com password hunter2\n")
        .expect("write neighbour file");

    let manifest = SandboxManifest {
        fs_metadata_read_allow: vec![granted.clone()],
        timeout: Some(Duration::from_secs(30)),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    // POSITIVE CONTROL — the whole point of the channel. `stat -f %z` prints
    // the size, so a success here is `file-read-metadata` actually working and
    // not merely "the command did not crash".
    let (code, stdout) = run(
        &backend,
        &manifest,
        &["/usr/bin/stat", "-f", "%z", granted.to_str().unwrap()],
    )
    .await;
    assert_eq!(
        code, 0,
        "the granted path must be stat-able; that is the entire grant"
    );
    let real_size = std::fs::metadata(&granted).expect("host stat").len();
    assert_eq!(
        stdout.trim(),
        real_size.to_string(),
        "stat must report the real size, not a placeholder: {stdout:?}"
    );

    // NEGATIVE CONTROL 1 — contents of the granted file stay denied.
    let (code, stdout) = run(
        &backend,
        &manifest,
        &["/bin/cat", granted.to_str().unwrap()],
    )
    .await;
    assert_ne!(code, 0, "metadata grant must not permit reading contents");
    assert!(
        !stdout.contains("operator"),
        "granted file's contents leaked: {stdout:?}"
    );

    // NEGATIVE CONTROL 2 — the neighbour is denied for BOTH operations, so the
    // grant is scoped to one file and not to its directory.
    for argv in [
        vec!["/usr/bin/stat", "-f", "%z", neighbour.to_str().unwrap()],
        vec!["/bin/cat", neighbour.to_str().unwrap()],
    ] {
        let (code, stdout) = run(&backend, &manifest, &argv).await;
        assert_ne!(code, 0, "neighbour must stay denied for {argv:?}");
        assert!(
            !stdout.contains("hunter2"),
            "neighbour secret leaked via {argv:?}: {stdout:?}"
        );
    }

    // NEGATIVE CONTROL 3 — enumeration. `file-read-metadata` on the ancestors
    // must not become `readdir` on them, or the grant would disclose every
    // other dotfile in the home directory by name.
    let (code, stdout) = run(&backend, &manifest, &["/bin/ls", home.to_str().unwrap()]).await;
    assert_ne!(
        code, 0,
        "listing the granted file's directory must be denied"
    );
    assert!(
        !stdout.contains(".netrc"),
        "directory enumeration leaked neighbour names: {stdout:?}"
    );

    // NEGATIVE CONTROL 4 — `fs_read_deny` still outranks the grant.
    //
    // This one failed on the first CI cycle and the failure was real, not a
    // flaky test: the backend originally emitted the grant and relied on the
    // later deny to override it. SBPL does not work that way across
    // operations — a `file-read*` deny is less specific than a
    // `file-read-metadata` allow, and `stat` kept succeeding. The backend now
    // withholds the grant instead, and this control is what proves the
    // withholding reaches a real kernel.
    let denied = SandboxManifest {
        fs_read_deny: vec![granted.clone()],
        ..manifest.clone()
    };
    let (code, stdout) = run(
        &backend,
        &denied,
        &["/usr/bin/stat", "-f", "%z", granted.to_str().unwrap()],
    )
    .await;
    assert_ne!(
        code, 0,
        "an explicit read-deny must beat a metadata grant; stdout={stdout:?}"
    );
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test live_integrity_macos`
/// executes 0 of 1 and still exits 0 printing `test result: ok`. This guard is
/// deliberately NOT `#[ignore]`d: three suites in this repo carried a guard that
/// was itself ignored, which made each inert against precisely the scenario it
/// existed for — it could only fire under `--ignored`, by which point the real
/// case were running anyway.
///
/// It always runs, so this binary can never report success on zero executed
/// tests, and it FAILS when a caller sets `WAYLAND_REQUIRE_IGNORED=1 (or WAYLAND_SANDBOX_LIVE_MACOS=1)` to declare a run of the
/// ignored case while passing an invocation that cannot execute any of them.
/// Skipped under nextest, whose `no-tests = "fail"` policy covers the same
/// ground at the invocation site.
/// Also honours `WAYLAND_SANDBOX_LIVE_MACOS`, this suite's own pre-existing
/// live-intent variable, so the CI job that sets it cannot silently run zero
/// cases.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    let declared = std::env::var("WAYLAND_REQUIRE_IGNORED").as_deref() == Ok("1")
        || std::env::var("WAYLAND_SANDBOX_LIVE_MACOS").as_deref() == Ok("1");
    if !declared {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "declared intent to run this suite's 1 #[ignore]d case, but neither \
         --ignored nor --include-ignored was passed, so zero of them can execute. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-sandbox --test live_integrity_macos -- --ignored --test-threads=1"
    );
}
