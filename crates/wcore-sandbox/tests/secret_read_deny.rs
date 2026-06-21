//! Task 9 — adversarial matrix for secret-read-deny enforcement.
//!
//! Each case exercises the host sandbox backend (macOS sandbox-exec, Linux
//! bwrap) through the real `SandboxBackend::execute` path with a crafted
//! `SandboxManifest` that includes both `fs_read_allow` and `fs_read_deny`
//! entries. All cases skip gracefully when the backend is unavailable.
//!
//! **Cases:**
//! (a) Pre-existing project secret under allowed root → secret bytes absent.
//! (b) Symlink `link -> .env` (both enumerated) → reading `link` yields no secret bytes.
//! (c) Non-secret file under allowed root, adjacent to a denied `.env` → readable (no over-deny).
//!     Documents the symlink-to-external-SECRET residual (backstopped by network-Deny).
//! (d) Credential-dir style deny (synthesized `creds/` under root) → bytes absent.
//! (e) Ordinary `src/main.rs` under the root → readable (no over-deny).
//!
//! Only compiled and run on macOS and Linux where a real sandbox backend exists.

#![cfg(any(target_os = "macos", target_os = "linux"))]

use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// Resolve a real `cat` binary. Backends scrub `PATH`, so we need an
/// absolute path.
fn cat_path() -> Option<&'static str> {
    ["/bin/cat", "/usr/bin/cat"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
}

/// Obtain the platform-appropriate backend. Returns None if unavailable
/// (sandbox-exec not installed, bwrap not installed, etc.) so tests can
/// skip gracefully.
fn host_backend() -> Option<Box<dyn SandboxBackend>> {
    #[cfg(target_os = "macos")]
    {
        use wcore_sandbox::backends::sandbox_exec::SandboxExecBackend;
        let b = SandboxExecBackend::new();
        if b.is_available() {
            return Some(Box::new(b));
        }
        None
    }
    #[cfg(target_os = "linux")]
    {
        use wcore_sandbox::backends::bwrap::BubblewrapBackend;
        let b = BubblewrapBackend::new();
        if b.is_available() {
            return Some(Box::new(b));
        }
        None
    }
}

// ===========================================================================
// (a) Pre-existing project secret under allowed root → bytes absent.
// ===========================================================================

#[tokio::test]
async fn secret_read_deny_case_a_project_env_under_allowed_root() {
    let Some(backend) = host_backend() else {
        eprintln!("skip: host sandbox backend not available");
        return;
    };
    let Some(cat) = cat_path() else {
        eprintln!("skip: no cat binary found");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let secret = root.join(".env");
    std::fs::write(&secret, b"SECRET_TOKEN=hunter2").expect("write secret");

    let manifest = SandboxManifest {
        fs_read_allow: vec![root.clone()],
        fs_read_deny: vec![secret.clone()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![cat.into(), secret.to_string_lossy().into_owned()],
                cwd: None,
            },
        )
        .await
        .expect("execute must not error");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("SECRET_TOKEN"),
        "(a) secret bytes must not be readable via direct path; exit={} stdout={:?}",
        out.exit_code,
        stdout,
    );
}

// ===========================================================================
// (b) Symlink `link -> .env` (both in deny list) → reading `link` yields no
//     secret bytes.
// ===========================================================================

#[tokio::test]
async fn secret_read_deny_case_b_symlink_to_env() {
    let Some(backend) = host_backend() else {
        eprintln!("skip: host sandbox backend not available");
        return;
    };
    let Some(cat) = cat_path() else {
        eprintln!("skip: no cat binary found");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let secret = root.join(".env");
    let link = root.join("link");

    std::fs::write(&secret, b"SECRET_TOKEN=hunter2").expect("write secret");
    std::os::unix::fs::symlink(&secret, &link).expect("create symlink");

    // Deny both the secret itself and the symlink (simulating what
    // compute_secret_deny does when it detects a symlink whose resolved
    // target is a secret).
    let manifest = SandboxManifest {
        fs_read_allow: vec![root.clone()],
        fs_read_deny: vec![secret.clone(), link.clone()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![cat.into(), link.to_string_lossy().into_owned()],
                cwd: None,
            },
        )
        .await
        .expect("execute must not error");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("SECRET_TOKEN"),
        "(b) symlink to secret must not expose secret bytes; exit={} stdout={:?}",
        out.exit_code,
        stdout,
    );
}

// ===========================================================================
// (c) Non-secret file under allowed root, adjacent to a denied `.env` →
//     readable. Proves that denying `.env` does NOT over-deny a neighbour
//     file at the same tree level.
//
//     NOTE (residual): a symlink whose resolved target is an external SECRET
//     that is NOT itself in `fs_read_deny` is a known limitation — the
//     allowlist + network-Deny contain the blast radius. That case is NOT
//     what this test covers; testing it here would require a predictable
//     external secret path, which is environment-specific.
// ===========================================================================

#[tokio::test]
async fn secret_read_deny_case_c_non_secret_neighbour_is_readable() {
    let Some(backend) = host_backend() else {
        eprintln!("skip: host sandbox backend not available");
        return;
    };
    let Some(cat) = cat_path() else {
        eprintln!("skip: no cat binary found");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");

    // The secret — in the deny list.
    let secret = root.join(".env");
    std::fs::write(&secret, b"SECRET_TOKEN=hunter2").expect("write secret");

    // An ordinary neighbour file — NOT in the deny list.
    let neighbour = root.join("README.md");
    std::fs::write(&neighbour, b"hello from README").expect("write README");

    let manifest = SandboxManifest {
        fs_read_allow: vec![root.clone()],
        fs_read_deny: vec![secret.clone()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![cat.into(), neighbour.to_string_lossy().into_owned()],
                cwd: None,
            },
        )
        .await
        .expect("execute must not error");

    // Behavioural proof: the neighbour must be readable — exit 0 and
    // content present — even though .env is denied at the same root level.
    assert_eq!(
        out.exit_code,
        0,
        "(c) non-secret neighbour must be readable (no over-deny); exit={} stderr={:?}",
        out.exit_code,
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("hello from README"),
        "(c) non-secret neighbour content must be present (no over-deny); stdout={:?}",
        stdout,
    );
}

// ===========================================================================
// (d) Credential-dir style deny: `creds/` directory under root → bytes absent.
// ===========================================================================

#[tokio::test]
async fn secret_read_deny_case_d_credential_dir_deny() {
    let Some(backend) = host_backend() else {
        eprintln!("skip: host sandbox backend not available");
        return;
    };
    let Some(cat) = cat_path() else {
        eprintln!("skip: no cat binary found");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let creds_dir = root.join("creds");
    std::fs::create_dir(&creds_dir).expect("create creds dir");
    let token_file = creds_dir.join("token");
    std::fs::write(&token_file, b"CRED_TOKEN=s3cr3t").expect("write credential file");

    // Deny the entire creds/ directory.
    let manifest = SandboxManifest {
        fs_read_allow: vec![root.clone()],
        fs_read_deny: vec![creds_dir.clone()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![cat.into(), token_file.to_string_lossy().into_owned()],
                cwd: None,
            },
        )
        .await
        .expect("execute must not error");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("CRED_TOKEN"),
        "(d) credential dir deny must prevent reading token file; exit={} stdout={:?}",
        out.exit_code,
        stdout,
    );
}

// ===========================================================================
// (e) Ordinary `src/main.rs` under root → readable (no over-deny).
// ===========================================================================

#[tokio::test]
async fn secret_read_deny_case_e_ordinary_file_remains_readable() {
    let Some(backend) = host_backend() else {
        eprintln!("skip: host sandbox backend not available");
        return;
    };
    let Some(cat) = cat_path() else {
        eprintln!("skip: no cat binary found");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let src_dir = root.join("src");
    std::fs::create_dir(&src_dir).expect("create src dir");
    let main_rs = src_dir.join("main.rs");
    std::fs::write(&main_rs, b"fn main() {}").expect("write main.rs");

    // Deny only the .env; do NOT deny src/main.rs.
    let secret = root.join(".env");
    std::fs::write(&secret, b"SECRET=hunter2").expect("write secret");

    let manifest = SandboxManifest {
        fs_read_allow: vec![root.clone()],
        fs_read_deny: vec![secret.clone()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![cat.into(), main_rs.to_string_lossy().into_owned()],
                cwd: None,
            },
        )
        .await
        .expect("execute must not error");

    assert_eq!(
        out.exit_code,
        0,
        "(e) ordinary src/main.rs must be readable; exit={} stderr={:?}",
        out.exit_code,
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("fn main"),
        "(e) ordinary file content must be readable; stdout={:?}",
        stdout,
    );
}
