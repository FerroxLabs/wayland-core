//! D8 — live PTY smoke test: boot the real `wayland-core` TUI under a
//! pseudo-terminal and assert the workspace chrome renders.
//!
//! This is the live counterpart to the unit tests in `pty_capture.rs` (which
//! only exercise `strip_ansi` / geometry without spawning the binary). It
//! proves the end-to-end D8 path: spawn → PTY → vt100 parse → rendered-screen
//! assertion. `#[ignore]`'d like the other live tests so the cheap CI floor
//! never boots a TUI; run it explicitly:
//!
//! ```text
//! WCORE_EVAL_BIN=$PWD/target/release/wayland-core \
//!   cargo test -p wcore-eval-scenarios --test pty_tui_smoke -- --ignored --nocapture
//! ```
//!
//! No API key or network is needed: the workspace chrome renders during boot,
//! before any turn, so the seeded config carries an empty key and the test
//! never makes an LLM call. `PtyCapture` points `WAYLAND_HOME`/`HOME` at a
//! throwaway tempdir, so the boot is hermetic (no real user MCP/config).

#![cfg(unix)]

use std::time::Duration;

use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::pty_capture::PtyCapture;
use wcore_eval_scenarios::runner::discover_binary;

#[test]
#[ignore = "live: boots the real wayland-core TUI under a PTY (needs a pre-built binary)"]
fn tui_boots_and_renders_workspace() {
    // Require the binary — a live smoke with no binary is operator error, not a
    // pass. Skip cleanly (not fail) so the suite is a no-op where it's absent.
    if discover_binary().is_err() {
        eprintln!(
            "SKIP tui_boots_and_renders_workspace: no wayland-core binary. \
             Pre-build it (`cargo build -p wcore-cli`) or set WCORE_EVAL_BIN."
        );
        return;
    }

    // Boot paints chrome before any turn, so no real key is needed.
    let provider = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-v4-pro");
    let mut cap = PtyCapture::spawn(&provider).expect("spawn wayland-core TUI under a PTY");

    // The core D8 assertion: the workspace chrome (the WAYLAND wordmark AND the
    // Workspace tab) renders within the boot budget. `wait_for_workspace` dumps
    // the last rendered screen on timeout, so a regression that breaks boot or
    // reintroduces unbounded waiting is debuggable from the failure alone.
    cap.wait_for_workspace()
        .expect("TUI should render the workspace chrome (WAYLAND wordmark + Workspace tab)");

    // Belt-and-suspenders: confirm the rendered grid is real chrome, not a
    // blank/partial paint. (Intent-documenting; `wait_for_workspace` already
    // gates on these anchors.)
    let screen = cap.screen_text();
    assert!(
        screen.contains("WAYLAND") && screen.contains("Workspace"),
        "expected workspace chrome in the rendered screen, got:\n{screen}"
    );

    // Clean shutdown via the command-palette `/exit` path; best-effort (the
    // Drop guard kills the child regardless).
    let _ = cap.quit_via_palette(Duration::from_secs(8));
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test pty_tui_smoke`
/// executes 0 of 1 and still exits 0 printing `test result: ok`. This guard is
/// deliberately NOT `#[ignore]`d: three suites in this repo carried a guard that
/// was itself ignored, which made each inert against precisely the scenario it
/// existed for — it could only fire under `--ignored`, by which point the real
/// case were running anyway.
///
/// It always runs, so this binary can never report success on zero executed
/// tests, and it FAILS when a caller sets `WAYLAND_REQUIRE_IGNORED=1` to declare a run of the
/// ignored case while passing an invocation that cannot execute any of them.
/// Skipped under nextest, whose `no-tests = "fail"` policy covers the same
/// ground at the invocation site.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    if std::env::var("WAYLAND_REQUIRE_IGNORED").as_deref() != Ok("1") {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "declared intent to run this suite's 1 #[ignore]d case, but neither \
         --ignored nor --include-ignored was passed, so zero of them can execute. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-eval-scenarios --test pty_tui_smoke -- --ignored --test-threads=1"
    );
}
