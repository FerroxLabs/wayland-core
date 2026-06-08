//! W12 (debt B.4-tokens): smoke test for the `tool_token_bench` binary.
//!
//! Runs the bench in `--scripted` mode and asserts:
//!   1. The bench writes a non-empty markdown file at the expected path.
//!   2. The markdown contains a row for every built-in tool the harness
//!      claims to measure (matches the registry registration order in
//!      `build_registry` inside the bin).
//!   3. The harness exits 0.
//!
//! Live-API mode is NOT exercised here — that needs real provider
//! credentials (see `docs/tool-token-empirical-<date>.md` §2).

#![cfg(feature = "test-utils")]

use std::path::PathBuf;
use std::process::Command;

/// Tools that the bench is expected to measure. Must stay in sync with
/// `build_registry` + `run_scripted` in
/// `src/bin/tool_token_bench.rs`. The Git tool is registered but the
/// scripted run doesn't issue a Git call (would touch the host repo);
/// keep it out of the assert set.
const EXPECTED_TOOLS: &[&str] = &["Read", "Bash", "Grep", "Glob", "Write", "Edit"];

fn engine_root() -> PathBuf {
    // crates/wcore-agent/tests/<this file> → up two = engine repo root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or(manifest)
}

fn today_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

// The bench launches a NESTED `cargo run -p wcore-agent --bin tool_token_bench`
// from inside a test inside `cargo nextest run`. In CI that nested cargo
// invocation hits a fresh target dir, recompiles ~half the workspace, and
// runs for >30s — long enough that nextest sends SIGTERM and the run fails
// not because the bench is wrong but because cargo-in-cargo doesn't fit a
// regular CI test slot. It also writes to `docs/tool-token-empirical-{today}.md`
// which is gitignored (per banked rule). Run locally or in the nightly
// soak workflow where bench-class timing is the point, not a regression.
#[test]
#[ignore = "bench-class: nested cargo run, runs >30s in CI; exercise via nightly soak or `cargo test -- --ignored` locally"]
fn scripted_run_writes_expected_markdown() {
    let root = engine_root();
    let expected_doc = root
        .join("docs")
        .join(format!("tool-token-empirical-{}.md", today_iso()));

    // Best-effort: remove a stale copy so we can prove the bench wrote
    // a fresh one. If the file doesn't exist, ignore the error.
    let _ = std::fs::remove_file(&expected_doc);

    let status = Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "-p",
            "wcore-agent",
            "--bin",
            "tool_token_bench",
            "--features",
            "test-utils",
        ])
        .current_dir(&root)
        .status()
        .expect("failed to spawn cargo run for tool_token_bench");
    assert!(status.success(), "tool_token_bench exited with {status:?}");

    assert!(
        expected_doc.exists(),
        "expected markdown at {} was not written",
        expected_doc.display()
    );
    let body = std::fs::read_to_string(&expected_doc).expect("read markdown");
    assert!(!body.is_empty(), "markdown is empty");
    assert!(
        body.contains("# Tool-token empirical baseline"),
        "header missing"
    );
    assert!(
        body.contains("## Runbook for live-API verification"),
        "runbook section missing"
    );

    let measured: usize = EXPECTED_TOOLS
        .iter()
        .filter(|name| body.contains(&format!("| {} |", name)))
        .count();
    assert_eq!(
        measured,
        EXPECTED_TOOLS.len(),
        "expected {} tools in markdown, found {} — body:\n{body}",
        EXPECTED_TOOLS.len(),
        measured
    );
}
