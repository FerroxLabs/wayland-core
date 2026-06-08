//! T2 smoke tests — exercise process plumbing without any API calls.
//!
//! These tests are deliberately narrow:
//!
//! 1. `spawns_and_captures_help` — the runner's binary-discovery +
//!    spawn helpers can launch `wayland-core --help`, see exit 0, and
//!    collect stdout. Proves we found the right artifact and pipe
//!    plumbing works.
//! 2. `hung_scenario_does_not_leak_pid` — `spawn_for_run` produces a
//!    `kill_on_drop(true)` child; dropping it (or explicitly killing
//!    it) reaps the process before the test returns. Verified by
//!    checking that the captured PID is gone after drop.
//!
//! Neither test makes a network call. The hung-scenario test passes
//! an empty API key so even if the engine tried to reach a provider
//! it would fail fast — but the test never gets that far because
//! the wall-time is set to 1 second and we kill on Elapsed.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{discover_binary, spawn_for_run, spawn_with_args};
use wcore_eval_scenarios::tempenv;

/// Locate the binary or skip the test with a clear message. The
/// runner's discovery requires either `WCORE_EVAL_BIN` or a binary
/// at `target/{debug,release}/wayland-core` — neither of which is
/// guaranteed in a cold checkout. The test gates run after the
/// orchestrator pre-builds via `cargo build -p wcore-cli` (per the
/// task brief gates), so on a normal run this returns `Some(path)`.
fn maybe_binary() -> Option<std::path::PathBuf> {
    match discover_binary() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!(
                "[smoke] skipping: could not locate wayland-core binary: {e}. \
                 Run `cargo build -p wcore-cli` first or set WCORE_EVAL_BIN."
            );
            None
        }
    }
}

#[tokio::test]
async fn spawns_and_captures_help() {
    let Some(bin) = maybe_binary() else {
        return;
    };

    let mut child = spawn_with_args(&bin, &["--help"]).expect("spawn --help");

    // Capture stdout BEFORE wait so the pipe never fills (--help is
    // small enough that this would be safe either way, but the
    // discipline mirrors run() where the child can produce a lot
    // before exiting).
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut buf = Vec::new();
    let read_fut = stdout.read_to_end(&mut buf);
    let read_result = tokio::time::timeout(Duration::from_secs(15), read_fut).await;
    let _ = read_result.expect("stdout read timed out — --help should be fast");

    let status = tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("child --help did not exit within 15s")
        .expect("child wait failed");

    assert!(
        status.success(),
        "wayland-core --help should exit 0, got {status}"
    );
    let s = String::from_utf8_lossy(&buf);
    assert!(!s.trim().is_empty(), "wayland-core --help stdout was empty");
    // Don't pin the exact text — clap's output evolves. Just check
    // for a single anchor that is overwhelmingly likely to remain:
    // the binary's own name appears in the usage line.
    assert!(
        s.contains("wayland-core") || s.to_lowercase().contains("usage"),
        "help stdout missing both 'wayland-core' and 'usage' anchors:\n{s}"
    );
}

/// Verify `kill_on_drop(true)` semantics + explicit `start_kill` on
/// timeout cleanly reaps the child. Without this discipline a hung
/// scenario would leak PIDs across the test suite, per cross-audit
/// M-1.
#[tokio::test]
async fn hung_scenario_does_not_leak_pid() {
    let Some(bin) = maybe_binary() else {
        return;
    };

    // Build a hermetic env. No API key — that's fine for the plumbing
    // test; the engine will fail to reach the provider but we kill it
    // long before it gets that far.
    let provider = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-chat")
        .with_api_key("test-key-never-used");
    let env = tempenv::build(&provider).expect("seed config");

    let mut child = spawn_for_run(&bin, env.path(), &provider, true, None).expect("spawn for run");

    let pid = child.id().expect("child should have a PID before kill");

    // Hold the child briefly then kill explicitly — mirrors what the
    // runner does on `tokio::time::timeout` Elapsed. We don't run a
    // full scenario; we just want to confirm the start_kill + wait
    // dance reaps cleanly.
    tokio::time::sleep(Duration::from_millis(200)).await;
    child.start_kill().expect("start_kill should succeed");
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("wait after start_kill timed out — child leaked");

    // Give the OS a moment to actually reap.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Post-check via libc::kill(pid, 0) — the canonical "does this
    // process exist?" probe. Returns 0 if alive, -1 with ESRCH if gone.
    // Originally shelled out to `/bin/kill -0 <pid>` but the ci-linux
    // slim Debian docker image (`rust:1.95-slim-bookworm`) doesn't
    // ship the kill binary, causing the test to panic with ENOENT and
    // taking nextest down with it (CI run 26396718138, job 77699695683).
    // The libc path has the same semantic check + no binary dep.
    #[cfg(unix)]
    {
        // SAFETY: kill(pid, 0) tests existence + permission only, no
        // signal sent. Return 0 = alive; -1 with ESRCH = gone (success).
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        let alive = if rc == 0 {
            true
        } else {
            !matches!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            )
        };
        assert!(
            !alive,
            "PID {pid} should be gone after wait(); libc::kill(0) reports alive — likely leak"
        );
    }

    // Drop env at end so the tempdir cleans up after the child is
    // confirmed dead (avoids EBUSY on macOS).
    let _ = env;
    let _ = pid;
}

/// Sanity: `discover_binary` rejects a missing `WCORE_EVAL_BIN`
/// override even when the auto-discovery would otherwise succeed.
/// Guards a future regression where a typo'd env var path is silently
/// ignored and the test falls back to the default — that would make
/// `WCORE_EVAL_BIN` a footgun.
#[test]
fn binary_discovery_rejects_missing_override() {
    let guard = EnvGuard::set("WCORE_EVAL_BIN", "/nonexistent/path/to/wayland-core");
    let r = discover_binary();
    drop(guard);
    assert!(
        r.is_err(),
        "discover_binary should reject a nonexistent WCORE_EVAL_BIN, got {r:?}"
    );
}

/// Minimal RAII env-var guard for the discovery test. `tokio::test`
/// processes share env state with sibling tests; nextest's
/// `[profile.eval] test-threads = 1` makes this safe in the eval
/// profile, but we restore on drop regardless.
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}
impl EnvGuard {
    fn set(key: &'static str, value: impl AsRef<Path>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: tests in this crate run with test-threads=1 (eval
        // profile) and no sibling thread reads this env. set_var is
        // marked unsafe in newer std editions because of the FFI race
        // on libc envp, which we explicitly avoid here.
        unsafe { std::env::set_var(key, value.as_ref()) };
        Self { key, prev }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
