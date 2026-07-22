//! Live descendant process-tree containment (macOS sandbox-exec backend).
//!
//! This is the macOS counterpart to the Linux descendant-reaping proof in
//! `hard_process_containment.rs` (`contained_detached_child_exit`), adapted to
//! the sandbox-exec backend. Linux reaps the tree through the bubblewrap PID
//! namespace (`--die-with-parent`); macOS has no PID namespace, so the
//! sandbox-exec backend instead attaches the sandboxed process group to a
//! `ProcessTreeGuard` and SIGKILLs the whole group the instant the direct child
//! exits (see `backends/process_tree.rs` — the macOS `disarm`/`Drop` path).
//! Either way, a detached descendant is contained and reaped with its parent.
//!
//! Whole-file `#![cfg(target_os = "macos")]` gating mirrors `live_integrity.rs`
//! (`#![cfg(windows)]`): on other platforms the file compiles to zero tests.
//! The `WAYLAND_SANDBOX_LIVE_MACOS` env opt-in mirrors the
//! `WAYLAND_SANDBOX_LIVE_WINDOWS` gate in `live_integrity.rs`.

#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::sandbox_exec::SandboxExecBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// Run `/bin/sh -c '/bin/sleep 45 & exit <code>'` under the macOS sandbox and
/// return `(exit_code, wall_clock)`.
///
/// The detached `sleep` inherits the sandboxed child's stdout pipe. If the
/// backend did NOT own and reap the descendant process tree, the direct child
/// exiting would leave the grandchild `sleep` alive holding that pipe, so
/// `execute` would block draining stdout until the 45s sleep exits or the 30s
/// manifest timeout fires (returning `Err(Timeout)`). Because the backend
/// SIGKILLs the sandboxed process group when the direct child exits, `execute`
/// returns promptly with the declared exit code. The wall-clock bound is
/// therefore a falsifiable descendant-reaping assertion.
async fn contained_detached_child_exit(
    backend: &SandboxExecBackend,
    exit_code: u8,
) -> (i32, Duration) {
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(30)),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        ..Default::default()
    };
    let cmd = SandboxCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("/bin/sleep 45 & exit {exit_code}"),
        ],
        cwd: None,
    };
    let started = Instant::now();
    let out = backend
        .execute(&manifest, cmd)
        .await
        .expect("contained execution must return an exit status, not block or error");
    (out.exit_code, started.elapsed())
}

/// Descendant containment on every terminal path. A generous 20s bound sits
/// well below both the 45s detached sleep and the 30s manifest timeout, so a
/// non-reaping backend (grandchild holding the pipe) cannot pass.
#[tokio::test]
#[ignore = "live macOS process-tree acceptance; run via `--run-ignored all` with WAYLAND_SANDBOX_LIVE_MACOS=1"]
async fn required_live_macos_process_tree_contains_descendants() {
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

    let bound = Duration::from_secs(20);

    let (zero_code, zero_elapsed) = contained_detached_child_exit(&backend, 0).await;
    assert_eq!(zero_code, 0, "zero-exit terminal path must report exit 0");
    assert!(
        zero_elapsed < bound,
        "zero-exit path leaked a descendant: execute took {zero_elapsed:?} (>= {bound:?})"
    );

    let (nonzero_code, nonzero_elapsed) = contained_detached_child_exit(&backend, 7).await;
    assert_eq!(
        nonzero_code, 7,
        "non-zero-exit terminal path must report the exact exit code"
    );
    assert!(
        nonzero_elapsed < bound,
        "non-zero-exit path leaked a descendant: execute took {nonzero_elapsed:?} (>= {bound:?})"
    );
}
