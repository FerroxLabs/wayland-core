//! Proving Ground integration tests — deterministic cell runner.
//!
//! Each test is a `Cell` that declares its config state, terminal shape,
//! and a script closure that drives the PTY. `run_cell` materializes the
//! config, launches the binary, runs the script, captures a `RunRecord`,
//! and cleans up the tempdir.

#[path = "support/mod.rs"]
mod support;

use support::proving_ground::{Cell, ConfigState, TermShape, run_cell};

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
