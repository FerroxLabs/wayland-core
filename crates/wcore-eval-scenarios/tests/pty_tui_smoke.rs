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
