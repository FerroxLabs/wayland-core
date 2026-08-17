//! SEC-10 / L6 (Linux) — the Bash timeout the model is told about must be the
//! one that is enforced.
//!
//! `BashTool` advertises "default 120000, max 600000" milliseconds in its own
//! tool description, and that is the only number the model ever sees. On Linux
//! the bubblewrap backend used to impose a wall-clock cap of its own —
//! `manifest.timeout.unwrap_or(Duration::from_secs(30))` — which fired first
//! and discarded ALL output. A 45 s build was killed at 30 s, on Linux only,
//! and nothing in the result said why.
//!
//! This test drives the REAL host backend through the REAL `BashTool` surface
//! with a command that runs longer than the old cap and far shorter than the
//! advertised default. It skips when the host is not running bubblewrap,
//! because on any other backend the cap never existed.

#![cfg(target_os = "linux")]

use std::time::Instant;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;

/// Only meaningful when the process would actually select bubblewrap:
/// `WAYLAND_SANDBOX` unset (or `bwrap`) and a real `bwrap` on the host.
fn bwrap_is_the_active_backend() -> bool {
    match std::env::var("WAYLAND_SANDBOX") {
        Ok(v) if !v.is_empty() && v != "bwrap" => return false,
        _ => {}
    }
    wcore_sandbox::backends::SandboxBackend::is_available(
        &wcore_sandbox::backends::bwrap::BubblewrapBackend::new(),
    )
}

#[tokio::test]
async fn a_thirty_three_second_command_survives_the_advertised_default_timeout() {
    if !bwrap_is_the_active_backend() {
        eprintln!("skip: bubblewrap is not the active backend on this host");
        return;
    }

    let started = Instant::now();
    // No `timeout` key: this is the advertised 120 s default.
    let result = BashTool
        .execute(json!({"command": "sleep 33; echo SURVIVED_THE_CAP"}))
        .await;
    let elapsed = started.elapsed();

    assert!(
        !result.is_error,
        "a 33 s command is far inside the advertised 120 s default and must \
         not be killed (returned after {elapsed:?}): {}",
        result.content
    );
    assert!(
        result.content.contains("SURVIVED_THE_CAP"),
        "the command's output must survive (returned after {elapsed:?}): {}",
        result.content
    );
    assert!(
        elapsed.as_secs() >= 33,
        "sanity: the child really did sleep, elapsed={elapsed:?}"
    );
}

/// The other direction — an EXPLICIT short `timeout` is still honoured, so the
/// test above cannot be satisfied by removing timeouts altogether.
#[tokio::test]
async fn an_explicit_short_timeout_is_still_enforced() {
    if !bwrap_is_the_active_backend() {
        eprintln!("skip: bubblewrap is not the active backend on this host");
        return;
    }

    let started = Instant::now();
    let result = BashTool
        .execute(json!({"command": "sleep 40", "timeout": 2000}))
        .await;
    let elapsed = started.elapsed();

    assert!(
        result.is_error,
        "an explicit 2 s timeout must kill a 40 s command: {}",
        result.content
    );
    assert!(
        elapsed.as_secs() < 30,
        "the kill must happen at the requested deadline, elapsed={elapsed:?}"
    );
}
