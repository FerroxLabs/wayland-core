//! Live filesystem-ACL verification for R61 (Windows AppContainer DACL grants).
//!
//! Proves, on real Windows hardware (gated behind `WAYLAND_SANDBOX_LIVE_WINDOWS`,
//! which the CI Windows runner sets), that `fs_read_allow`/`fs_write_allow` are
//! actually wired to AppContainer DACLs:
//!   1. WITHOUT a grant, a sandboxed `cmd /c type <file>` is DENIED.
//!   2. WITH `fs_read_allow`, the same command SUCCEEDS and reads the content.
//!   3. AFTER the run, the grant is REVOKED — the AppContainer SID
//!      (`S-1-15-2-…`) / profile name is gone from the file's DACL, so the fix
//!      leaves no permanent grant on the host filesystem.
//!
//! Test files live under `%PUBLIC%` (shallow, AppContainer-traversable ancestor
//! chain; writable without elevation) so the positive read exercises the grant
//! and not an unrelated ancestor-traversal denial.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::Duration;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

const MARKER: &str = "HEADROOM_R61_GRANT_OK";

fn live() -> bool {
    std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_ok()
}

/// Seed a unique test dir under `%PUBLIC%` holding a file containing [`MARKER`].
/// `tag` keeps concurrent tests from colliding even under a shared-process runner.
fn seed_file(tag: &str) -> (PathBuf, PathBuf) {
    let public = std::env::var("PUBLIC").unwrap_or_else(|_| r"C:\Users\Public".into());
    let dir = PathBuf::from(public).join(format!("wcore-r61-{}-{}", std::process::id(), tag));
    std::fs::create_dir_all(&dir).expect("create test dir");
    let file = dir.join("granted.txt");
    std::fs::write(&file, MARKER).expect("write test file");
    (dir, file)
}

/// `icacls <path>` output (unsandboxed), for asserting on the path's DACL.
fn icacls(path: &Path) -> String {
    let out = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .expect("run icacls");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn type_file(file: &Path) -> SandboxCommand {
    SandboxCommand {
        argv: vec![
            "cmd.exe".into(),
            "/c".into(),
            "type".into(),
            file.display().to_string(),
        ],
        cwd: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ungranted_path_is_denied_in_sandbox() {
    if !live() {
        return;
    }
    let (dir, file) = seed_file("denied");
    let backend = AppContainerBackend::new();
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    let out = backend
        .execute(&manifest, type_file(&file))
        .await
        .expect("execute");
    let _ = std::fs::remove_dir_all(&dir);

    assert_ne!(
        out.exit_code,
        0,
        "an ungranted file must be denied inside the sandbox; got exit {} stdout={:?}",
        out.exit_code,
        String::from_utf8_lossy(&out.stdout)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn granted_path_is_readable_then_revoked() {
    if !live() {
        return;
    }
    let (dir, file) = seed_file("granted");
    let backend = AppContainerBackend::new();
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        ..Default::default()
    };
    let out = backend
        .execute(&manifest, type_file(&file))
        .await
        .expect("execute");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // Capture the DACL while the file still exists, before cleanup.
    let acl_after = icacls(&file);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        out.exit_code,
        0,
        "a granted file must be readable inside the sandbox; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(MARKER),
        "sandbox must read the granted file's content; got stdout={stdout:?}"
    );
    // The grant must be revoked once the spawn finished — no permanent
    // AppContainer ACE left on the host. icacls renders the package SID either
    // as the raw `S-1-15-2-…` or a resolved name containing the profile moniker.
    let acl_lower = acl_after.to_lowercase();
    assert!(
        !acl_after.contains("S-1-15-2-") && !acl_lower.contains("wcoresandbox"),
        "AppContainer grant must be revoked after the run (no host ACL leak); icacls:\n{acl_after}"
    );
}

/// Task 4: A secret file under a granted parent directory is unreadable when
/// its path is in `fs_read_deny` (DENY ACE overrides the parent ALLOW grant),
/// and the DENY ACE is revoked after the spawn completes.
#[tokio::test(flavor = "current_thread")]
async fn denied_secret_under_granted_parent_is_unreadable_and_revoked() {
    if !live() {
        return;
    }
    let (dir, _) = seed_file("deny-parent");
    // Place the secret inside the granted parent.
    let secret_file = dir.join("secret.env");
    std::fs::write(&secret_file, "SECRET_TOKEN=supersecret").expect("write secret");

    let backend = AppContainerBackend::new();
    if !backend.is_available() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    // Grant the PARENT directory (so the AppContainer can traverse it) but
    // deny the specific secret file. The DENY ACE overrides the ALLOW.
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        fs_read_deny: vec![secret_file.clone()],
        ..Default::default()
    };
    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "type".into(),
                    secret_file.display().to_string(),
                ],
                cwd: None,
            },
        )
        .await
        .expect("execute");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // Capture DACL while file still exists, before cleanup.
    let acl_after = icacls(&secret_file);
    let _ = std::fs::remove_dir_all(&dir);

    // The secret bytes must not appear in stdout (access denied → empty / error).
    assert!(
        !stdout.contains("SECRET_TOKEN"),
        "secret bytes must not be readable when path is in fs_read_deny; stdout={stdout:?}"
    );
    // The DENY ACE must be revoked after the run — no permanent host ACL leak.
    let acl_lower = acl_after.to_lowercase();
    assert!(
        !acl_after.contains("S-1-15-2-") && !acl_lower.contains("wcoresandbox"),
        "AppContainer DENY ACE must be revoked after the run; icacls:\n{acl_after}"
    );
}
