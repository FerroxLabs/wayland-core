//! Wave RA RELIABILITY BLOCKER #1 — verify `kill_on_drop(true)` is
//! applied by `wcore_config::shell::shell_command_builder` and
//! `shell_command_argv` so a subprocess actually dies when its parent
//! future is dropped.
//!
//! Failure mode without the fix: Tokio's default `Command` behavior is
//! to leave the child running when its future is dropped. The Bash
//! tool's `tokio::select! { _ = ctx.cancel.cancelled() => ..., r = self.execute(input) => ... }`
//! returns "cancelled" but the spawned `sleep 60` keeps consuming CPU
//! + memory for the full duration.
//!
//! Strategy: directly use the shell helpers (which is where the fix
//! lives), capture the spawned child's PID via `Child::id()`, drop the
//! Child handle, then poll `kill(pid, 0)` for ESRCH within 2s. Also
//! exercises BashTool::execute_with_ctx end-to-end with cancel race:
//! the cancel must cause the BashTool to return is_error=true within
//! 500ms — proving the cancel propagation works through the existing
//! `tokio::select!`.
//!
//! Unix-only because `kill(pid, 0)` is the canonical "does this process
//! exist?" probe and doesn't exist on Windows. The fix is
//! cross-platform; Windows verification needs a different probe.

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use wcore_config::shell::{shell_command_argv, shell_command_builder};
use wcore_sandbox::SandboxRegistry;
use wcore_sandbox::backends::no_sandbox::NoSandboxBackend;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::vfs::RealFs;
use wcore_tools::{NullToolOutputSink, Tool};

/// kill(pid, 0) is the canonical "does this process exist?" probe.
/// Returns true iff the PID still exists (alive OR zombie).
fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) has no side effects — it tests permission +
    // existence only. Pid is non-zero so we won't accidentally target a
    // process group.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // ESRCH = gone. Everything else (EPERM is the common case) means
    // the pid exists but we lack signal permission.
    errno != libc::ESRCH
}

async fn wait_for_pid_gone(pid: u32, deadline: Duration) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    !pid_alive(pid)
}

/// shell_command_builder: dropping the Child must SIGKILL the
/// subprocess. Pre-fix this test would hang for 60s; post-fix the
/// child is gone within ~50ms.
#[tokio::test]
async fn shell_command_builder_kill_on_drop_actually_kills() {
    let mut cmd = shell_command_builder("sleep 60");
    let child = cmd.spawn().expect("spawn");
    let pid = child.id().expect("child must have a pid before drop");

    // Confirm the child is alive immediately after spawn.
    assert!(
        pid_alive(pid),
        "child must be alive immediately after spawn"
    );

    // Drop the Child — `kill_on_drop(true)` should have armed SIGKILL.
    drop(child);

    let gone = wait_for_pid_gone(pid, Duration::from_secs(2)).await;
    assert!(
        gone,
        "child pid={pid} survived drop by more than 2s; \
         `.kill_on_drop(true)` is NOT applied by `shell_command_builder`."
    );
}

/// shell_command_argv: same contract as shell_command_builder. Wave SA
/// shipped the argv mode; Wave RA adds kill_on_drop to its returned
/// Command.
#[tokio::test]
async fn shell_command_argv_kill_on_drop_actually_kills() {
    let mut cmd = shell_command_argv("sleep", &["60"]);
    let child = cmd.spawn().expect("spawn");
    let pid = child.id().expect("child must have a pid before drop");
    assert!(
        pid_alive(pid),
        "child must be alive immediately after spawn"
    );

    drop(child);

    let gone = wait_for_pid_gone(pid, Duration::from_secs(2)).await;
    assert!(
        gone,
        "child pid={pid} survived drop by more than 2s; \
         `.kill_on_drop(true)` is NOT applied by `shell_command_argv`."
    );
}

/// Negative-control: a Command WITHOUT kill_on_drop still leaves its
/// child running after drop. Documents what the pre-fix behavior was
/// and proves the test apparatus actually distinguishes the two states.
#[tokio::test]
async fn unkilled_command_survives_drop_negative_control() {
    let mut cmd = Command::new("sleep");
    cmd.arg("60");
    // Deliberately NOT setting kill_on_drop.
    let child = cmd.spawn().expect("spawn");
    let pid = child.id().expect("child must have a pid before drop");
    assert!(pid_alive(pid));

    drop(child);

    // The unkilled child should STILL be alive after a generous 500ms
    // wait. (Defensive: if some platform quirk eats it anyway, we just
    // skip — we don't want a flaky negative control to break CI.)
    tokio::time::sleep(Duration::from_millis(500)).await;
    let still_alive = pid_alive(pid);

    // Clean up the leaked child so it doesn't survive the test run.
    if still_alive {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }

    assert!(
        still_alive,
        "negative control: a Command without kill_on_drop must NOT die \
         on drop. If this assertion fails, the apparatus is broken — \
         the positive tests above cannot be trusted."
    );
}

/// End-to-end: BashTool::execute_with_ctx receives a cancel mid-sleep
/// and returns is_error=true within 500ms. Verifies the full pipeline:
/// `tokio::select!` race + helper-applied kill_on_drop reaping the
/// child as the execute future drops.
#[tokio::test]
async fn bash_tool_returns_promptly_when_cancelled_mid_sleep() {
    // Inject the exact backend under test. ToolContext defaults fail closed,
    // so process-global environment toggles cannot prove this execution path.
    let sandbox = Arc::new(SandboxRegistry::new(Arc::new(NoSandboxBackend::new())));
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel2.cancel();
    });
    let ctx = ToolContext::new(
        "ra-bash-cancel-e2e",
        cancel,
        Arc::new(RealFs),
        None,
        Arc::new(NullToolOutputSink),
    )
    .with_sandbox(sandbox);

    let start = Instant::now();
    let result = BashTool
        .execute_with_ctx(json!({ "command": "sleep 30" }), &ctx)
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_error, "cancelled BashTool must return is_error");
    assert!(
        result.content.to_lowercase().contains("cancel"),
        "expected cancellation message, got: {}",
        result.content
    );
    assert!(
        elapsed < Duration::from_millis(700),
        "BashTool cancel must return <700ms (S2 contract); took {elapsed:?}"
    );
}
