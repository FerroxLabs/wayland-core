//! SEC-06 / SEC-10 (Linux, bubblewrap) — two HONESTY properties of the
//! bwrap backend, every one of them graded from OUTSIDE the namespace.
//!
//! 1. **A write that does not land must not report success.** The bwrap root
//!    is a fresh tmpfs and `/tmp` is a fresh tmpfs, so before the fix a
//!    `printf > /tmp/<name>` under the ACTIVE sandbox returned `Exit code: 0`,
//!    read back successfully inside the namespace, and then vanished at
//!    teardown — the host path never existed. macOS/sandbox-exec returns
//!    EPERM for the same command, i.e. on macOS the agent is told the truth.
//!    Data loss presented as success is the worst kind of tool result: an
//!    agent that believes it wrote a file builds on that belief.
//!
//! 2. **The backend must not invent a wall-clock cap the caller never asked
//!    for.** `manifest.timeout == None` used to mean "bwrap kills the child at
//!    30 s", while `BashTool` advertises 120 s default / 600 s max to the model
//!    and `SandboxExecBackend` (macOS) imposes no cap of its own. A 45 s build
//!    was killed at 30 s on Linux and nowhere else.
//!
//! Both cases skip gracefully when bwrap is not installed.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::bwrap::BubblewrapBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// Backends scrub `PATH`, so the child argv must be an absolute path.
fn sh_path() -> Option<&'static str> {
    ["/bin/sh", "/usr/bin/sh"]
        .into_iter()
        .find(|p| Path::new(p).exists())
}

fn backend() -> Option<BubblewrapBackend> {
    let b = BubblewrapBackend::new();
    b.is_available().then_some(b)
}

/// A host path that does not exist and is not inside any granted root.
fn unique_host_path(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_nanos();
    PathBuf::from(format!("/tmp/wlc-{tag}-{}-{nanos}.txt", std::process::id()))
}

async fn run(
    backend: &BubblewrapBackend,
    manifest: &SandboxManifest,
    script: String,
) -> wcore_sandbox::SandboxOutput {
    let sh = sh_path().expect("checked by the caller");
    backend
        .execute(
            manifest,
            SandboxCommand {
                argv: vec![sh.into(), "-c".into(), script],
                cwd: None,
            },
        )
        .await
        .expect("bwrap execute must not error out")
}

/// The workspace grant the agent actually has: one writable root.
fn workspace_manifest(root: &Path) -> SandboxManifest {
    SandboxManifest {
        fs_write_allow: vec![root.to_path_buf()],
        fs_read_allow: vec![root.to_path_buf()],
        ..Default::default()
    }
}

// ===========================================================================
// 1. Silent write loss
// ===========================================================================

/// A write to a path OUTSIDE every granted root must fail visibly. Graded on
/// (a) what the tool reports and (b) the host filesystem, from outside.
#[tokio::test]
async fn out_of_workspace_write_reports_failure_and_leaves_no_file() {
    let Some(backend) = backend() else {
        eprintln!("skip: bwrap not available on this host");
        return;
    };
    if sh_path().is_none() {
        eprintln!("skip: no /bin/sh");
        return;
    }
    let workspace = tempfile::tempdir().expect("tempdir");
    let phantom = unique_host_path("phantom");
    assert!(
        !phantom.exists(),
        "precondition: {} must not exist before the run",
        phantom.display()
    );

    let out = run(
        &backend,
        &workspace_manifest(workspace.path()),
        format!("printf wrote > {}", phantom.display()),
    )
    .await;

    assert_ne!(
        out.exit_code,
        0,
        "a write to {} landed nowhere, so it MUST NOT report success; \
         stdout={:?} stderr={:?}",
        phantom.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !phantom.exists(),
        "graded from outside the sandbox: {} must not exist on the host",
        phantom.display()
    );
}

/// The same property for the namespace ROOT, which is where an unbound
/// absolute path lands once the child creates its parent directories.
#[tokio::test]
async fn write_to_namespace_root_reports_failure() {
    let Some(backend) = backend() else {
        eprintln!("skip: bwrap not available on this host");
        return;
    };
    if sh_path().is_none() {
        eprintln!("skip: no /bin/sh");
        return;
    }
    let workspace = tempfile::tempdir().expect("tempdir");

    let out = run(
        &backend,
        &workspace_manifest(workspace.path()),
        "mkdir -p /wlc-root-probe && printf wrote > /wlc-root-probe/f".to_string(),
    )
    .await;

    assert_ne!(
        out.exit_code,
        0,
        "creating a directory in the ephemeral namespace root and writing into \
         it MUST NOT report success; stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// POSITIVE CONTROL for both cases above. Without this a backend that simply
/// refused every write would pass them. The workspace tempdir lives UNDER
/// `/tmp`, so this also proves the granted bind survives the read-only `/tmp`.
#[tokio::test]
async fn in_workspace_write_succeeds_and_lands_on_the_host() {
    let Some(backend) = backend() else {
        eprintln!("skip: bwrap not available on this host");
        return;
    };
    if sh_path().is_none() {
        eprintln!("skip: no /bin/sh");
        return;
    }
    let workspace = tempfile::tempdir().expect("tempdir");
    let target = workspace.path().join("granted.txt");

    let out = run(
        &backend,
        &workspace_manifest(workspace.path()),
        format!("printf granted > {}", target.display()),
    )
    .await;

    assert_eq!(
        out.exit_code,
        0,
        "a write inside the granted root must succeed; stderr={:?}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("granted file must exist on the host"),
        "granted",
        "graded from outside the sandbox: the bytes must be on the host"
    );
}

// ===========================================================================
// 2. The invented 30 s cap
// ===========================================================================

/// With no `manifest.timeout`, the backend must not impose one of its own.
/// The child sleeps 33 s — past the old Linux-only 30 s cap, well inside the
/// 120 s BashTool advertises — and must be allowed to finish.
#[tokio::test]
async fn no_manifest_timeout_means_no_backend_imposed_cap() {
    let Some(backend) = backend() else {
        eprintln!("skip: bwrap not available on this host");
        return;
    };
    if sh_path().is_none() {
        eprintln!("skip: no /bin/sh");
        return;
    }
    let workspace = tempfile::tempdir().expect("tempdir");
    let mut manifest = workspace_manifest(workspace.path());
    manifest.timeout = None;

    let sh = sh_path().expect("checked above");
    let started = Instant::now();
    let result = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![sh.into(), "-c".into(), "sleep 33; printf SURVIVED".into()],
                cwd: None,
            },
        )
        .await;
    let elapsed = started.elapsed();

    let out = result.unwrap_or_else(|e| {
        panic!(
            "a 33 s command must not be killed when the manifest asked for no \
             timeout (killed after {elapsed:?}): {e}"
        )
    });
    assert_eq!(out.exit_code, 0, "the sleep must exit cleanly");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "SURVIVED",
        "the command's output must survive"
    );
    assert!(
        elapsed >= Duration::from_secs(33),
        "sanity: the child really did sleep, elapsed={elapsed:?}"
    );
}

/// The other direction: an EXPLICIT `manifest.timeout` is still enforced.
/// Without this, "no cap" could be satisfied by never timing out at all.
#[tokio::test]
async fn explicit_manifest_timeout_is_still_enforced() {
    let Some(backend) = backend() else {
        eprintln!("skip: bwrap not available on this host");
        return;
    };
    if sh_path().is_none() {
        eprintln!("skip: no /bin/sh");
        return;
    }
    let workspace = tempfile::tempdir().expect("tempdir");
    let mut manifest = workspace_manifest(workspace.path());
    manifest.timeout = Some(Duration::from_secs(1));

    let sh = sh_path().expect("checked above");
    let started = Instant::now();
    let result = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![sh.into(), "-c".into(), "sleep 30".into()],
                cwd: None,
            },
        )
        .await;

    assert!(
        matches!(result, Err(wcore_sandbox::SandboxError::Timeout)),
        "a 1 s manifest timeout must still kill a 30 s child, got {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the kill must happen at the requested deadline, not the child's exit"
    );
}
