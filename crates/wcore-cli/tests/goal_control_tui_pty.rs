//! F22-C1 REACHABILITY — drive `/goal` on a real TUI attached to a real PTY.
//!
//! ## What this file is for
//!
//! `issue_goal_control` and the five `ProtocolCommand::Goal*` variants shipped
//! with ZERO call sites: the control surface existed and no user could reach
//! it. A unit test that calls the handler directly would restate that defect
//! rather than close it — it proves the handler works, not that anything can
//! reach the handler.
//!
//! So this drives the SHIPPED BINARY: spawn `wayland-core` on a pseudo-terminal,
//! type `/goal …` the way a user types it, and read the answer off the rendered
//! vt100 grid. The chain under test is the whole one —
//! keystrokes → palette/composer → `SurfaceAction::Command`
//! → `CommandRegistry::dispatch` → the `/goal` router arm
//! → `TuiEngine::request_goal_control` → `GoalControlBridge::issue_goal_control`
//! → `wcore_agent::goal::handle_goal_control` → `ProtocolEvent` → `apply_event`
//! → the rendered frame.
//!
//! ## Three hosts in one binary
//!
//! Whether Core is ever asked depends on whether this host holds a durable
//! journal to name a Goal in. Every disposition that reaches `/goal` is driven
//! here, because inferring one from the other is the habit that left this row
//! open:
//!
//! * [`goal_open_is_accepted_by_core_on_a_durable_host`] supplies vault unlock
//!   material, so the session is fully durable and Core answers with a
//!   `goal_snapshot` the status line renders. **This is the leg that proves
//!   `issue_goal_control` actually runs.**
//! * [`goal_open_is_accepted_on_a_keyless_host`] supplies none — the stock
//!   headless-Linux posture. It used to be the leg that proved the refusal:
//!   `Config::resolve` degraded `[session] enabled` to false on a box with no
//!   keyring and no unlocked vault, so there was no journal and no Goal. ADR
//!   0003's third decision ended that — a keyless host now journals without
//!   crash replay (`SessionPersistence::JournaledWithoutReplay`), so it holds
//!   Goals like any other. The leg stays, driving the same host, asserting the
//!   outcome that replaced the refusal; without it nothing here would notice a
//!   regression that took the keyless journal away again.
//! * [`goal_open_names_the_cause_when_the_session_has_no_journal`] is where the
//!   refusal moved. `[session] enabled = false` is the operator's own opt-out
//!   and the only remaining way to reach `/goal` with no session id, and it
//!   proves the surface names the real cause instead of reporting a missing
//!   Goal or going silent.
//!
//! ## Why `#![cfg(unix)]`
//!
//! Identical to every other PTY smoke here: `portable_pty`'s ConPTY backend on
//! a headless Windows runner does not surface the child's stdout to the master
//! end, so the vt100 grid stays empty and every wait times out. The `/goal`
//! parse half is covered cross-platform by the unit tests in
//! `tui::commands::goal`. **The Windows and macOS terminal legs of this
//! criterion are NOT measured by this file.**

#![cfg(unix)]

use std::time::Duration;

use tempfile::TempDir;

mod support;
use support::pty::{Pty, write_config};

/// A hermetic TUI on a host that cannot SEAL a durable session — no keyring on
/// a headless box and no vault unlock material. It still journals: the host
/// degrade costs `sealed_prepared_request` and nothing else.
fn boot_keyless() -> (TempDir, Pty) {
    boot_with(&[], false)
}

/// A hermetic TUI on a host that can seal. `WAYLAND_VAULT_PASSPHRASE` is the
/// unlock material `vault_unlock_material_present` reads, so the encrypted-file
/// vault is available, the degrade does not fire, and `init_session` opens a
/// durable journal with crash replay.
fn boot_durable() -> (TempDir, Pty) {
    boot_with(
        &[("WAYLAND_VAULT_PASSPHRASE", "goal-control-pty-passphrase")],
        false,
    )
}

/// A hermetic TUI with NO journal at all, which since ADR 0003's third decision
/// only the operator can ask for. `[session] enabled = false` is written into
/// the config rather than forced by the environment for exactly that reason:
/// starving the host of credentials no longer produces this state, and a test
/// that constructed it that way would be asserting a posture the product left.
fn boot_without_a_journal() -> (TempDir, Pty) {
    boot_with(&[], true)
}

fn boot_with(extra_env: &[(&str, &str)], sessions_disabled: bool) -> (TempDir, Pty) {
    let home = TempDir::new().expect("tempdir");
    // No network is ever reached: `/goal` never sends an agent turn, and the
    // key below is not a credential.
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        None,
    );
    if sessions_disabled {
        let path = home.path().join("config.toml");
        let mut toml = std::fs::read_to_string(&path).expect("read seeded config.toml");
        toml.push_str("\n[session]\nenabled = false\n");
        std::fs::write(&path, toml).expect("disable durable sessions");
    }
    // 200 columns, not the harness default 120: the Goal segment is the LAST
    // thing on the status line, and at 120 the first run of this file rendered
    // `goals 1 live` with the `/ 1` clipped off the right edge. A width that
    // truncates the evidence turns a real acceptance into a timeout.
    let pty = Pty::spawn_with_env(home.path(), 40, 200, extra_env);
    pty.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );
    (home, pty)
}

/// Type a slash line and submit it, the way a user does.
///
/// `/` on an empty composer opens the command palette (`workspace.rs`). The
/// palette accumulates `[a-z0-9-_]` into its query and, on the first character
/// that cannot appear in a command name — a SPACE — hands `/<query> ` back to
/// the composer (`CloseOverlayAndPasteToActive`). So a bare `/goal` is run from
/// the palette row, and `/goal <args>` is completed in the composer. Both are
/// the real user path, which is why `/goal` had to be registered in
/// `CommandRegistry::with_builtins` to be reachable at all.
fn run_slash(pty: &mut Pty, line: &str) {
    pty.send(line.as_bytes());
    std::thread::sleep(Duration::from_millis(500));
    pty.send(b"\r");
}

/// POSITIVE CONTROL, the one this criterion turns on: a real user at a real
/// terminal opens a durable Goal, and Core's answer comes back through the
/// event stream and is rendered.
///
/// `goals 1 live / 1` is written by `goal_status_summary` from `App.goals`,
/// which is written ONLY by the `GoalSnapshot` arm of `apply_event`. The only
/// producer of that event on this path is `handle_goal_control` ACCEPTING the
/// command — so this string cannot appear unless `issue_goal_control` ran and
/// Core accepted.
#[test]
fn goal_open_is_accepted_by_core_on_a_durable_host() {
    let (_home, mut pty) = boot_durable();
    run_slash(&mut pty, "/goal open tui-probe direct 2 prove the surface");
    pty.wait_for(
        |s| s.contains("goals 1 live / 1"),
        Duration::from_secs(30),
        "the status line to render a live Goal fed from goal_snapshot",
    );
    println!(
        "--- accepted screen ---\n{}\n--- end ---",
        pty.screen_text()
    );
    pty.quit();
}

/// NEGATIVE CONTROL for the leg above. A `/goal advance` for a Goal this
/// terminal holds no projection of must NOT produce a snapshot: the cursor that
/// binds the decision to inspected state does not exist, so the surface refuses
/// locally and no Goal reaches the status line.
///
/// Without it, `goals 1 live / 1` above would be evidence that *something*
/// renders, not that the command was accepted on its merits.
#[test]
fn advance_without_a_projection_produces_no_goal_on_the_status_line() {
    let (_home, mut pty) = boot_durable();
    run_slash(&mut pty, "/goal advance never-opened");
    pty.wait_for(
        |s| s.contains("/goal resync never-opened"),
        Duration::from_secs(30),
        "the local refusal naming the recovery step",
    );
    let screen = pty.screen_text();
    assert!(
        !screen.contains("live / "),
        "a refused advance must not put a Goal on the status line.\n--- screen ---\n{screen}"
    );
    pty.quit();
}

/// The stock headless-Linux posture: no keyring, no vault material. This leg
/// asserted the refusal until ADR 0003's third decision, which is the whole
/// reason it still drives this host — a keyless box journals now, so the Goal
/// is ACCEPTED here exactly as it is on a sealed host, and the only way to see
/// a regression that takes the keyless journal away again is to keep asking.
///
/// The negative half is not decoration. `no durable journal` is the answer this
/// host used to give; if it ever comes back, the run is silently back to
/// refusing Goals on every headless server, and `goals 1 live / 1` alone would
/// not say which of the two answers arrived first.
#[test]
fn goal_open_is_accepted_on_a_keyless_host() {
    let (_home, mut pty) = boot_keyless();
    run_slash(&mut pty, "/goal open tui-probe direct 2 prove the surface");
    pty.wait_for(
        |s| s.contains("goals 1 live / 1"),
        Duration::from_secs(30),
        "a keyless host to hold the Goal its journal can record",
    );
    let screen = pty.screen_text();
    assert!(
        !screen.contains("no durable journal"),
        "a keyless host journals; it must not be told it has no journal.\n--- screen ---\n{screen}"
    );
    pty.quit();
}

/// The operator's own `[session] enabled = false`, the one remaining way to
/// reach `/goal` with no session id. Core is never asked, and the surface names
/// the missing journal as the cause rather than reporting a missing Goal or
/// saying nothing at all.
#[test]
fn goal_open_names_the_cause_when_the_session_has_no_journal() {
    let (_home, mut pty) = boot_without_a_journal();
    run_slash(&mut pty, "/goal open tui-probe direct 2 prove the surface");
    pty.wait_for(
        |s| s.contains("no durable journal"),
        Duration::from_secs(30),
        "the journal-less session's cause to be named at the terminal",
    );
    let screen = pty.screen_text();
    assert!(
        !screen.contains("live / "),
        "a session with no journal must not put a Goal on the status line.\n--- screen ---\n{screen}"
    );
    pty.quit();
}

/// A bare `/goal` is reachable from the palette row and renders the Goal
/// listing — the discovery step a user takes before controlling anything. It
/// needs no durable session because it reads only what the event stream
/// already delivered.
#[test]
fn a_bare_goal_is_reachable_from_the_palette() {
    let (_home, mut pty) = boot_keyless();
    run_slash(&mut pty, "/goal");
    pty.wait_for(
        |s| s.contains("No durable Goal has been reported"),
        Duration::from_secs(30),
        "the /goal listing to render",
    );
    pty.quit();
}

/// NEGATIVE CONTROL for the instrument itself. A near-miss command must not
/// produce the Goal surface's answer. If this ever passes while showing goal
/// text, every assertion above is measuring the screen and not the wiring.
#[test]
fn a_near_miss_command_does_not_reach_the_goal_surface() {
    let (_home, mut pty) = boot_keyless();
    run_slash(&mut pty, "/goalzzzzzz");
    std::thread::sleep(Duration::from_secs(3));
    let screen = pty.screen_text();
    assert!(
        !screen.contains("no durable journal")
            && !screen.contains("Durable Goals in this session")
            && !screen.contains("No durable Goal has been reported"),
        "`/goalzzzzzz` must not reach the Goal surface.\n--- screen ---\n{screen}"
    );
    pty.quit();
}
