//! M4.3 — `wcore-evolve-bench` smoke test.
//!
//! Mirrors `evolve_cli.rs` for the bench-driven entrypoint: confirms
//! the binary spawns, loads the 30-case mini-bench, scores the seed
//! skill through `BenchScorer`, and exits 0 with the documented
//! stdout shape.
//!
//! Two cases:
//!
//! - `bench_smoke_all_pass` — no overrides, no force-fail. The
//!   `CannedBenchRunner` mirrors every strategy's expected output so
//!   the parent skill scores 1.000.
//! - `bench_smoke_force_fail_drops_parent_score` — forces three cases
//!   to fail and asserts `parent_score` falls below 1.0. Proves the
//!   bench scorer's signal is actually wired into the GEPA loop.

use std::path::PathBuf;
use std::process::Command;

use tempfile::tempdir;

fn seed_file() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parent_skill.md")
}

fn parse_score(stdout: &str, key: &str) -> Option<f64> {
    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix(&format!("{key}=")) {
            return v.trim().parse().ok();
        }
    }
    None
}

#[test]
fn bench_smoke_all_pass() {
    let bin = env!("CARGO_BIN_EXE_wcore-evolve-bench");
    let graveyard = tempdir().expect("tempdir");
    let out = Command::new(bin)
        .args([
            "--seed-file",
            seed_file().to_str().expect("seed file utf8"),
            "--seed-name",
            "refactor-imports",
            "--generations",
            "1",
            "--fan-out",
            "2",
            "--plateau-window",
            "3",
            "--child-timeout-secs",
            "5",
            "--graveyard-root",
            graveyard.path().to_str().expect("graveyard utf8"),
            "--run-id",
            "bench-smoke",
        ])
        .output()
        .expect("spawn wcore-evolve-bench");
    assert!(
        out.status.success(),
        "wcore-evolve-bench exit {:?}\nstderr:\n{}\nstdout:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    for key in [
        "scorer=bench",
        "generations_run=",
        "termination=",
        "parent_score=",
        "best_score=",
        "graveyard_root=",
        "curator_decision=",
    ] {
        assert!(stdout.contains(key), "missing {key} in:\n{stdout}");
    }

    let parent = parse_score(&stdout, "parent_score").expect("parent_score line parses");
    // Canned runner mirrors every strategy's pass output, so the
    // bench score is exactly 30/30 = 1.000.
    assert!(
        (parent - 1.0).abs() < 1e-9,
        "expected parent_score=1.000, got {parent} in:\n{stdout}"
    );
}

#[test]
fn bench_smoke_force_fail_drops_parent_score() {
    let bin = env!("CARGO_BIN_EXE_wcore-evolve-bench");
    let graveyard = tempdir().expect("tempdir");
    let out = Command::new(bin)
        .args([
            "--seed-file",
            seed_file().to_str().expect("seed file utf8"),
            "--seed-name",
            "refactor-imports",
            "--generations",
            "1",
            "--fan-out",
            "2",
            "--plateau-window",
            "3",
            "--child-timeout-secs",
            "5",
            "--graveyard-root",
            graveyard.path().to_str().expect("graveyard utf8"),
            "--run-id",
            "bench-smoke-fail",
            // Three forced failures — exact ids that exist in the
            // corpus (verified by the M4.1 invariant check; if any
            // of these are renamed in a future PR this test will
            // break loudly, which is the desired behaviour).
            "--force-fail-case",
            "arith-01-add-small",
            "--force-fail-case",
            "arith-02-multiply",
            "--force-fail-case",
            "arith-03-subtract-negative",
        ])
        .output()
        .expect("spawn wcore-evolve-bench");
    assert!(
        out.status.success(),
        "wcore-evolve-bench exit {:?}\nstderr:\n{}\nstdout:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let parent = parse_score(&stdout, "parent_score").expect("parent_score line parses");
    // 27/30 = 0.900.
    let expected = 27.0_f64 / 30.0_f64;
    assert!(
        (parent - expected).abs() < 1e-9,
        "expected parent_score={expected}, got {parent} in:\n{stdout}"
    );
}
