//! Proving Ground integration tests — deterministic cell runner.
//!
//! Each test is a `Cell` that declares its config state, terminal shape,
//! and a script closure that drives the PTY. `run_cell` materializes the
//! config, launches the binary, runs the script, captures a `RunRecord`,
//! and cleans up the tempdir.
//!
//! The whole harness is PTY-driven and therefore Unix-only — gate the entire
//! test crate so it compiles to an empty binary on Windows (the
//! `support::proving_ground` module is itself `#[cfg(unix)]`, so a file-scope
//! `use` of it would otherwise fail to resolve on Windows).
#![cfg(unix)]

#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use support::proving_ground::invariants;
use support::proving_ground::record::{self, RunRecord};
use support::proving_ground::{
    CANONICAL_REVEAL_KEYS, Cell, ConfigState, Session, TermShape, run_cell,
};

const SECS_10: Duration = Duration::from_secs(10);

#[cfg(unix)]
#[test]
fn run_cell_captures_a_runrecord_for_a_clean_boot() {
    let cell = Cell {
        name: "clean-boot",
        config: ConfigState::ConfiguredOpenAi, // writes a minimal config so it boots to Workspace
        term: TermShape::default(),
        script: |pty, _s| {
            pty.wait_for(
                |t| t.contains("Workspace"),
                std::time::Duration::from_secs(10),
                "workspace",
            );
        },
    };
    let rec = run_cell(&cell);
    assert!(
        !rec.dirty_death,
        "clean boot must not leave a dirty-death sentinel"
    );
    assert!(rec.final_screen.contains("Workspace"));
}

#[cfg(unix)]
#[test]
fn onboarding_persists_across_relaunch() {
    let session = Session::new();
    ConfigState::EnvKeysOnly.materialize(session.home()); // OPENAI_API_KEY in child env, no config.toml

    // First launch: connect the detected env key (press '1'), complete the flow.
    let mut p1 = session.launch();
    p1.wait_for(
        |t| t.contains("Detected in your environment"),
        SECS_10,
        "onboarding",
    );
    p1.send(b"1"); // connect OpenAI (offline) -> AddMore ("Keys saved")
    // Env keys connect without a network round-trip and land on AddMore; the
    // single detected key means the cursor defaults to "Continue".
    p1.wait_for(|t| t.contains("Keys saved"), SECS_10, "keys-saved");
    p1.send(b"\r"); // AddMore: Continue -> Name ("Almost done")
    p1.wait_for(|t| t.contains("Almost done"), SECS_10, "name-step");
    p1.send(b"\r"); // Name: finish -> Ready
    p1.wait_for(|t| t.contains("Ready"), SECS_10, "ready");
    p1.send(b"\r"); // finish -> Workspace

    // Item 4: snapshot screen BEFORE quit so final_screen reflects the UI state.
    let final_screen_p1 = record::redact(&p1.screen_text());
    p1.quit();
    let rec1 = RunRecord::capture_post_quit(session.home(), &mut p1, final_screen_p1);

    // config.toml MUST now exist with the provider.
    let cfg = std::fs::read_to_string(session.home().join("config.toml")).unwrap_or_default();
    assert!(
        cfg.contains("openai"),
        "connect must persist a provider to config.toml"
    );

    // Second launch (same home): MUST land on Workspace, not Onboarding.
    let mut p2 = session.launch();
    p2.wait_for(
        |t| t.contains("Workspace") && !t.contains("connect a provider to begin"),
        SECS_10,
        "workspace-not-onboarding",
    );
    let final_screen_p2 = record::redact(&p2.screen_text());
    p2.quit();
    let rec2 = RunRecord::capture_post_quit(session.home(), &mut p2, final_screen_p2);

    // Item 4: wire the config_persists invariant.
    invariants::config_persists(&[rec1, rec2]).unwrap();
}

#[cfg(unix)]
#[test]
fn connect_all_env_keys_persists_across_relaunch() {
    let session = Session::new();
    ConfigState::MultiEnvKeys.materialize(session.home()); // OPENAI + ANTHROPIC keys in env, no config.toml

    // First launch: connect all detected env keys at once (press 'a').
    let mut p1 = session.launch();
    p1.wait_for(
        |t| t.contains("Detected in your environment"),
        SECS_10,
        "onboarding",
    );
    p1.send(b"a"); // connect all env keys
    // 'a' routes to Step::Name which renders "What should I call you?"
    p1.wait_for(
        |t| t.contains("What should I call you?"),
        SECS_10,
        "name-step",
    );
    p1.send(b"\r"); // accept default name → finish_with_config → Ready step
    // Wait for Ready, then press ⏎ to enter the Workspace so quit() can
    // send the palette /exit command cleanly (it requires the Workspace surface).
    p1.wait_for(
        |t| t.contains("Press") && t.contains("workspace"),
        SECS_10,
        "ready-step",
    );
    p1.send(b"\r"); // advance to Workspace
    p1.wait_for(
        |t| t.contains("Workspace") && !t.contains("connect a provider to begin"),
        SECS_10,
        "workspace-after-onboarding",
    );

    let final_screen_p1 = record::redact(&p1.screen_text());
    p1.quit();
    let rec1 = RunRecord::capture_post_quit(session.home(), &mut p1, final_screen_p1);

    assert!(
        !rec1.dirty_death,
        "first launch must exit cleanly (no force-kill sentinel)"
    );

    // config.toml MUST now exist with BOTH provider slugs — proves the full
    // multi-provider config was written, not just the single-provider stub
    // that connect_all_env_keys persists as an early mid-flow checkpoint.
    let cfg = std::fs::read_to_string(session.home().join("config.toml")).unwrap_or_default();
    assert!(
        cfg.contains("openai"),
        "connect-all must write openai to config.toml; got: {cfg}"
    );
    assert!(
        cfg.contains("anthropic"),
        "connect-all must write anthropic to config.toml; got: {cfg}"
    );

    // Second launch (same home): MUST land on Workspace, not Onboarding.
    let mut p2 = session.launch();
    p2.wait_for(
        |t| t.contains("Workspace") && !t.contains("connect a provider to begin"),
        SECS_10,
        "workspace-not-onboarding",
    );
    let final_screen_p2 = record::redact(&p2.screen_text());
    p2.quit();
    let rec2 = RunRecord::capture_post_quit(session.home(), &mut p2, final_screen_p2);

    // Wire the config_persists invariant.
    invariants::config_persists(&[rec1, rec2]).unwrap();
}

#[cfg(unix)]
#[test]
fn same_cell_yields_same_verdicts_twice() {
    let cell = Cell {
        name: "replay-determinism",
        config: ConfigState::ConfiguredOpenAi,
        term: TermShape::default(),
        script: |pty, _s| {
            pty.wait_for(
                |t| t.contains("Workspace"),
                std::time::Duration::from_secs(10),
                "workspace",
            );
        },
    };
    let a = run_cell(&cell);
    let b = run_cell(&cell);
    assert_eq!(
        a.dirty_death, b.dirty_death,
        "dirty_death verdict must be stable across runs"
    );
    assert_eq!(
        a.final_screen.contains("Workspace"),
        b.final_screen.contains("Workspace"),
        "Workspace-reached verdict must be stable across runs"
    );
    assert_eq!(
        a.config_toml.is_some(),
        b.config_toml.is_some(),
        "config-present verdict must be stable across runs"
    );
}

#[cfg(unix)]
#[test]
fn doctor_content_reachable_at_short_height() {
    let session = Session::new();
    ConfigState::ConfiguredOpenAi.materialize(session.home());
    // 24-row height forces overflow: the /doctor report spans SYSTEM ·
    // PROVIDERS · CONFIG · TOOLS · MCP SERVERS · CHANNELS · DISCOVERED ·
    // RECENT ERRORS · TOKEN BUDGET — more than 24 lines — so TOKENS is
    // below the fold without scrolling.
    let mut p = session.launch_sized(TermShape {
        rows: 24,
        cols: 100,
    });
    p.wait_for(|t| t.contains("Workspace"), SECS_10, "workspace");
    p.send(b"/doctor\r");
    p.wait_for(|t| t.contains("SYSTEM"), SECS_10, "doctor");
    // The TOKEN BUDGET section must be reachable via the canonical reveal keys.
    let reached = support::proving_ground::reach_text(
        &mut p,
        "TOKEN BUDGET",
        CANONICAL_REVEAL_KEYS,
        Duration::from_millis(300),
    );
    assert!(
        reached,
        "/doctor must scroll to its TOKEN BUDGET section at 24-row height"
    );
    // Wire the content_reachable invariant.
    invariants::content_reachable("TOKEN BUDGET", reached);
    p.quit();
}
