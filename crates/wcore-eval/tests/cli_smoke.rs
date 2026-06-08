//! Smoke: spawn the binary; assert `gate` exits 0 on the bundled corpus.

use std::process::Command;

fn bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_wcore-eval"))
}

#[test]
fn cli_gate_exits_zero_on_corpus() {
    let out = Command::new(bin()).arg("gate").output().expect("spawn");
    assert!(
        out.status.success(),
        "wcore-eval gate exited {:?}: stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cli_score_emits_json_per_case() {
    let out = Command::new(bin()).arg("score").output().expect("spawn");
    assert!(out.status.success(), "score subcommand exited non-zero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = stdout.lines().next().expect("at least one line");
    let _: serde_json::Value = serde_json::from_str(first).expect("first line is JSON");
}

#[test]
fn cli_gate_emits_json_summary_with_flag() {
    let out = Command::new(bin())
        .arg("gate")
        .arg("--json")
        .output()
        .expect("spawn");
    assert!(out.status.success(), "gate --json exited non-zero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    assert!(v.get("precision").is_some(), "JSON missing precision");
    assert!(v.get("recall").is_some(), "JSON missing recall");
}
