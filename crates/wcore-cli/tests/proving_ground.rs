//! Proving Ground integration tests — deterministic cell runner.
//!
//! Each test is a `Cell` that declares its config state, terminal shape,
//! and a script closure that drives the PTY. `run_cell` materializes the
//! config, launches the binary, runs the script, captures a `RunRecord`,
//! and cleans up the tempdir.

#[path = "support/mod.rs"]
mod support;

use std::time::Duration;

use support::proving_ground::{Cell, ConfigState, Session, TermShape, run_cell};

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
    p1.wait_for(|t| t.contains("Detected in your environment"), SECS_10, "onboarding");
    p1.send(b"1"); // connect OpenAI
    p1.wait_for(
        |t| t.contains("Ready") || t.contains("Workspace"),
        SECS_10,
        "connected",
    );
    p1.send(b"\r"); // finish
    p1.quit();
    // config.toml MUST now exist with the provider.
    let cfg = std::fs::read_to_string(session.home().join("config.toml")).unwrap_or_default();
    assert!(cfg.contains("openai"), "connect must persist a provider to config.toml");
    // Second launch (same home): MUST land on Workspace, not Onboarding.
    let mut p2 = session.launch();
    p2.wait_for(
        |t| t.contains("Workspace") && !t.contains("connect a provider to begin"),
        SECS_10,
        "workspace-not-onboarding",
    );
    p2.quit();
}
