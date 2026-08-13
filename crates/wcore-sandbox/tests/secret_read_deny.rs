//! Task 9 — adversarial matrix for secret-read-deny enforcement.
//!
//! Each case exercises the host sandbox backend (macOS sandbox-exec, Linux
//! bwrap) through the real `SandboxBackend::execute` path with a crafted
//! `SandboxManifest` that includes both `fs_read_allow` and `fs_read_deny`
//! entries. All cases skip gracefully when the backend is unavailable.
//!
//! **Cases:**
//! (a) Pre-existing project secret under allowed root → secret bytes absent.
//! (b) Symlink `link -> .env`; deny the resolved target `.env` → reading
//!     through `link` yields no secret bytes (production denies the canonical
//!     target, not the raw symlink path).
//! (c) Symlink `ext -> <external non-secret>` (external, NOT a secret) → readable.
//!     Proves the sandbox does not over-deny files reached via symlinks that cross
//!     the primary allowed root boundary.  Documents the symlink-to-external-SECRET
//!     residual (backstopped by network-Deny).
//! (d) Credential-dir style deny (synthesized `creds/` under root) → bytes absent.
//! (e) Ordinary `src/main.rs` under the root → readable (no over-deny).
//!
//! Only compiled and run on macOS and Linux where a real sandbox backend exists.

#![cfg(any(target_os = "macos", target_os = "linux"))]

use wcore_sandbox::SandboxManifest;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::test_support::run_contained_probe;

/// Resolve a real `sh`. Backends scrub `PATH`, so we need an absolute path.
///
/// This replaced a bare `cat_path()`: every case now runs its probe through a
/// shell so `run_contained_probe` can wrap it in the liveness markers that
/// prove the child executed INSIDE the sandbox. Without that proof an absent
/// secret is indistinguishable from a sandbox that never spawned anything.
fn sh_path() -> Option<&'static str> {
    ["/bin/sh", "/usr/bin/sh"]
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
    let Some(sh) = sh_path() else {
        eprintln!("skip: no sh binary found");
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

    // The old non-vacuity guard here was `!stderr.contains("bwrap: ")`, and its
    // own comment conceded "the marker never appears on macOS sandbox-exec" —
    // i.e. on macOS there was NO control at all, and a sandbox-exec profile that
    // failed to load would have produced empty stdout and a green. The liveness
    // markers replace it with a control that works on every backend.
    let probe = run_contained_probe(
        backend.as_ref(),
        &manifest,
        sh,
        &format!("cat '{}'", secret.display()),
    )
    .await;
    probe.assert_absent(
        "SECRET_TOKEN",
        "(a) secret bytes must not be readable via direct path",
    );
}

// ===========================================================================
// (b) Symlink `link -> .env`; deny the resolved target `.env` → reading
//     through `link` yields no secret bytes.
// ===========================================================================

#[tokio::test]
async fn secret_read_deny_case_b_symlink_to_env() {
    let Some(backend) = host_backend() else {
        eprintln!("skip: host sandbox backend not available");
        return;
    };
    let Some(sh) = sh_path() else {
        eprintln!("skip: no sh binary found");
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let secret = root.join(".env");
    let link = root.join("link");

    std::fs::write(&secret, b"SECRET_TOKEN=hunter2").expect("write secret");
    std::os::unix::fs::symlink(&secret, &link).expect("create symlink");

    // compute_secret_deny canonicalizes (symlink-resolves) every candidate, so
    // it denies the RESOLVED target (.env) — never the raw symlink path. Feed
    // bwrap the same production-faithful deny list (just the target) and read
    // THROUGH the symlink: the read resolves to the masked .env and yields no
    // secret bytes. (Overlaying /dev/null directly onto a symlink path is not
    // something production ever emits, and bwrap cannot bind onto it.)
    let manifest = SandboxManifest {
        fs_read_allow: vec![root.clone()],
        fs_read_deny: vec![secret.clone()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    let probe = run_contained_probe(
        backend.as_ref(),
        &manifest,
        sh,
        &format!("cat '{}'", link.display()),
    )
    .await;
    probe.assert_absent(
        "SECRET_TOKEN",
        "(b) symlink to secret must not expose secret bytes",
    );
}

// ===========================================================================
// (c) Symlink `ext -> <external_non_secret>` (external, NOT a secret) →
//     readable. Proves that denying the workspace `.env` does NOT over-deny
//     a file reached via a symlink that crosses the primary allowed root
//     into a separately-allowed external location.
//
//     The external target lives in a second tempdir (`ext_root`) that is
//     added to `fs_read_allow` independently of the primary workspace root.
//     On macOS sandbox-exec, SBPL resolves the symlink to the real target
//     and checks the target path; on Linux bwrap, the `--ro-bind` for
//     `ext_root` makes the target accessible in the namespace.  Both code
//     paths exercise symlink-resolution behavior at the sandbox boundary —
//     which is the behaviour that differs from a plain neighbour file.
//
//     RESIDUAL (documented, not tested here): a symlink whose resolved
//     target is an external SECRET that is NOT itself in `fs_read_deny` is a
//     known limitation — the allowlist + network-Deny contain the blast
//     radius.  A predictable external secret path is environment-specific,
//     so the no-over-deny half (symlink to a non-secret) is what we prove.
// ===========================================================================

#[tokio::test]
async fn secret_read_deny_case_c_symlink_to_external_non_secret_is_readable() {
    let Some(backend) = host_backend() else {
        eprintln!("skip: host sandbox backend not available");
        return;
    };
    let Some(sh) = sh_path() else {
        eprintln!("skip: no sh binary found");
        return;
    };

    // Primary workspace root: contains the secret.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");
    let secret = root.join(".env");
    std::fs::write(&secret, b"SECRET_TOKEN=hunter2").expect("write secret");

    // External (second) root: holds a non-secret file.  This root is
    // outside the workspace but explicitly added to fs_read_allow, so the
    // sandbox can reach it when the symlink is followed.
    let ext_tmp = tempfile::tempdir().expect("ext tempdir");
    let ext_root = std::fs::canonicalize(ext_tmp.path()).expect("canonicalize ext_root");
    let ext_file = ext_root.join("non_secret.txt");
    std::fs::write(&ext_file, b"external non-secret data").expect("write ext file");

    // Symlink inside the workspace root that points to the external file.
    // Reading through this symlink crosses the primary allowed root boundary.
    let link = root.join("ext");
    std::os::unix::fs::symlink(&ext_file, &link).expect("create symlink ext -> ext_file");

    let manifest = SandboxManifest {
        // Both roots in the allow list; only the workspace secret is denied.
        fs_read_allow: vec![root.clone(), ext_root.clone()],
        fs_read_deny: vec![secret.clone()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };

    // Read through the symlink, not the target directly.
    let probe = run_contained_probe(
        backend.as_ref(),
        &manifest,
        sh,
        &format!("cat '{}'", link.display()),
    )
    .await;
    // Behavioural proof: the symlink to a non-secret external file must be
    // readable — the probe body exits 0 and the expected bytes are present —
    // even though .env is denied in the same workspace root.
    probe.assert_probe_succeeded(
        "(c) symlink to external non-secret must be readable (no over-deny)",
    );
    probe.assert_present(
        "external non-secret data",
        "(c) symlink content must be present (no over-deny)",
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
    let Some(sh) = sh_path() else {
        eprintln!("skip: no sh binary found");
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

    let probe = run_contained_probe(
        backend.as_ref(),
        &manifest,
        sh,
        &format!("cat '{}'", token_file.display()),
    )
    .await;
    probe.assert_absent(
        "CRED_TOKEN",
        "(d) credential dir deny must prevent reading token file",
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
    let Some(sh) = sh_path() else {
        eprintln!("skip: no sh binary found");
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

    let probe = run_contained_probe(
        backend.as_ref(),
        &manifest,
        sh,
        &format!("cat '{}'", main_rs.display()),
    )
    .await;
    probe.assert_probe_succeeded("(e) ordinary src/main.rs must be readable");
    probe.assert_present("fn main", "(e) ordinary file content must be readable");
}
