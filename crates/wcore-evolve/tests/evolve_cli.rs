//! `wcore-evolve --seed-file <skill> --generations 2 --fan-out 2 --graveyard-root <tmp>`
//! against a fixture skill completes and exits 0; output contains termination + score lines.
//!
//! Shells through `Command::new(env!("CARGO_BIN_EXE_wcore-evolve"))` per cargo's
//! standard integration-test pattern — NOT through `Command::new("sh")` (per
//! AGENTS.md cross-platform rule).

use std::process::Command;

use tempfile::tempdir;

#[test]
fn evolve_smoke_against_fixture() {
    let bin = env!("CARGO_BIN_EXE_wcore-evolve");
    let seed_file =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parent_skill.md");
    let graveyard = tempdir().expect("tempdir");
    let out = Command::new(bin)
        .args([
            "--seed-file",
            seed_file.to_str().expect("seed file utf8"),
            "--seed-name",
            "refactor-imports",
            "--generations",
            "2",
            "--fan-out",
            "2",
            "--plateau-window",
            "3",
            "--child-timeout-secs",
            "5",
            "--graveyard-root",
            graveyard.path().to_str().expect("graveyard utf8"),
            "--run-id",
            "smoke-run",
        ])
        .output()
        .expect("spawn wcore-evolve");
    assert!(
        out.status.success(),
        "wcore-evolve exit {:?}\nstderr:\n{}\nstdout:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("generations_run="),
        "missing generations_run= in:\n{stdout}"
    );
    assert!(
        stdout.contains("termination="),
        "missing termination= in:\n{stdout}"
    );
    assert!(
        stdout.contains("parent_score="),
        "missing parent_score= in:\n{stdout}"
    );
    assert!(
        stdout.contains("graveyard_root="),
        "missing graveyard_root= in:\n{stdout}"
    );
    assert!(
        stdout.contains("curator_decision="),
        "missing curator_decision= in:\n{stdout}"
    );
}
