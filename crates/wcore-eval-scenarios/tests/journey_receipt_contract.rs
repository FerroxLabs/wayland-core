//! Phase 24 F24-05 — the journey receipt tool's refusals, driven through the
//! COMPILED BINARY rather than through the library.
//!
//! The library unit tests in `src/journey.rs` cover the decision functions. These
//! cover the thing a gate actually invokes: the process, its argument parser and
//! its exit status. A verifier whose library says "refuse" but whose binary exits
//! zero is worse than no verifier, because every gate downstream reads the status.
//!
//! Each test is written so that REMOVING the refusal it names turns it red.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use wcore_eval_scenarios::journey::{
    ARRIVAL_SOURCE_INDEPENDENT_SINK, CANONICAL_STEPS, DeliveryCounts, JourneyReceipt, JourneyStep,
    RECEIPT_SCHEMA,
};

const TOOL: &str = env!("CARGO_BIN_EXE_wayland-journey");

fn steps() -> Vec<JourneyStep> {
    CANONICAL_STEPS
        .iter()
        .map(|name| JourneyStep {
            name: (*name).to_string(),
            command: format!("wayland-core {name}"),
            output: format!("captured output of {name}"),
            ok: true,
        })
        .collect()
}

fn receipt(platform: &str, commit: &str, digest: &str) -> JourneyReceipt {
    JourneyReceipt {
        schema: RECEIPT_SCHEMA.to_string(),
        platform: platform.to_string(),
        service_family: "systemd".into(),
        candidate_commit: commit.to_string(),
        binary_version: "wayland-core 0.12.25".into(),
        binary_sha256: digest.to_string(),
        driver_commit: "c".repeat(40),
        started_at: "2026-07-28T00:00:00Z".into(),
        finished_at: "2026-07-28T00:05:00Z".into(),
        arrival_source: ARRIVAL_SOURCE_INDEPENDENT_SINK.into(),
        counts: DeliveryCounts {
            submitted: 12,
            arrived: 12,
            unique: 12,
            duplicates: 0,
            losses: 0,
        },
        steps: steps(),
    }
}

fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write fixture");
    path
}

fn write_receipt(dir: &Path, name: &str, receipt: &JourneyReceipt) -> PathBuf {
    write(
        dir,
        name,
        &serde_json::to_string_pretty(receipt).expect("serialise"),
    )
}

/// Stage a file whose sha256 the caller then reads back, so the "digest
/// matches" positive control is never a hand-copied constant.
fn stage_binary(dir: &Path, bytes: &[u8]) -> (PathBuf, String) {
    let path = write_bytes(dir, "fake-wayland-core", bytes);
    let digest = wcore_eval_scenarios::journey::sha256_file(&path).expect("hash");
    (path, digest)
}

fn write_bytes(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write bytes");
    path
}

fn run(args: &[&str]) -> Output {
    Command::new(TOOL).args(args).output().expect("spawn tool")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

// ── verify ──────────────────────────────────────────────────────────────────

#[test]
fn verify_accepts_a_well_formed_receipt() {
    // POSITIVE CONTROL, and it is not optional. Every refusal test below would
    // also pass against a tool that refused everything; only this one
    // distinguishes a verifier from a rejector.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let path = write_receipt(dir.path(), "r.json", &receipt("linux", &commit, &digest));

    let output = run(&[
        "verify",
        "--receipt",
        &path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &commit,
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("JOURNEY VERIFIED platform=linux"),
        "{}",
        stdout(&output)
    );
    assert!(
        stdout(&output).contains("duplicates=0 losses=0"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn verify_refuses_a_receipt_with_a_missing_step() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"binary bytes");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    r.steps.retain(|s| s.name != "hard-kill");
    let path = write_receipt(dir.path(), "r.json", &r);

    let output = run(&[
        "verify",
        "--receipt",
        &path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &commit,
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("step"), "{}", stderr(&output));
}

#[test]
fn verify_refuses_a_receipt_whose_digest_does_not_match_the_binary_it_hashes() {
    // The whole point of having the VERIFIER hash the file: this receipt is
    // internally consistent and would pass any check that trusted its own
    // recorded digest.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, _) = stage_binary(dir.path(), b"the binary actually on disk");
    let other = write_bytes(dir.path(), "other", b"a DIFFERENT build entirely");
    let other_digest = wcore_eval_scenarios::journey::sha256_file(&other).expect("hash");
    let commit = "a".repeat(40);
    let path = write_receipt(
        dir.path(),
        "r.json",
        &receipt("linux", &commit, &other_digest),
    );

    let output = run(&[
        "verify",
        "--receipt",
        &path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &commit,
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("digest mismatch"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_a_receipt_claiming_the_wrong_platform() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"binary bytes");
    let commit = "a".repeat(40);
    let path = write_receipt(dir.path(), "r.json", &receipt("linux", &commit, &digest));

    let output = run(&[
        "verify",
        "--receipt",
        &path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "macos",
        "--expect-commit",
        &commit,
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("platform"), "{}", stderr(&output));
}

#[test]
fn verify_refuses_counts_that_do_not_reconcile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"binary bytes");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    // Two extra arrivals; the receipt still asserts a clean run.
    r.counts.arrived = 14;
    let path = write_receipt(dir.path(), "r.json", &r);

    let output = run(&[
        "verify",
        "--receipt",
        &path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &commit,
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("do not reconcile"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_a_receipt_whose_arrivals_came_from_the_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"binary bytes");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    r.arrival_source = "gateway-ledger".into();
    let path = write_receipt(dir.path(), "r.json", &r);

    let output = run(&[
        "verify",
        "--receipt",
        &path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &commit,
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("arrival_source"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_an_empty_receipt_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, _) = stage_binary(dir.path(), b"binary bytes");
    let path = write(dir.path(), "empty.json", "   \n");
    let output = run(&[
        "verify",
        "--receipt",
        &path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &"a".repeat(40),
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("empty"), "{}", stderr(&output));
}

// ── scan ────────────────────────────────────────────────────────────────────

const CANARY: &str = "WLJ-CANARY-c0ffee0123456789abcdef";

#[test]
fn scan_passes_when_the_canary_travelled_a_capture_and_is_absent_from_the_document() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = write(dir.path(), "doc.md", "# receipts\nnothing sensitive here\n");
    let canaries = write(dir.path(), "canaries.txt", &format!("{CANARY}\n"));
    let raw = write(
        dir.path(),
        "raw.txt",
        &format!("seeded {CANARY} into home\n"),
    );

    let output = run(&[
        "scan",
        "--document",
        &doc.display().to_string(),
        "--canary-file",
        &canaries.display().to_string(),
        "--raw-capture",
        &raw.display().to_string(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("SCAN PASS canaries=1"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn scan_refuses_a_canary_absent_from_every_raw_capture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = write(dir.path(), "doc.md", "nothing sensitive here\n");
    let canaries = write(dir.path(), "canaries.txt", &format!("{CANARY}\n"));
    let raw = write(dir.path(), "raw.txt", "a capture that never saw it\n");

    let output = run(&[
        "scan",
        "--document",
        &doc.display().to_string(),
        "--canary-file",
        &canaries.display().to_string(),
        "--raw-capture",
        &raw.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("never travelled a capture path"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn scan_refuses_a_canary_present_in_the_document() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = write(dir.path(), "doc.md", &format!("oops {CANARY}\n"));
    let canaries = write(dir.path(), "canaries.txt", &format!("{CANARY}\n"));
    let raw = write(dir.path(), "raw.txt", &format!("seeded {CANARY}\n"));

    let output = run(&[
        "scan",
        "--document",
        &doc.display().to_string(),
        "--canary-file",
        &canaries.display().to_string(),
        "--raw-capture",
        &raw.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("PRESENT in the published document"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn scan_refuses_a_fifteen_byte_canary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let short = "fifteen-bytes!!";
    assert_eq!(short.len(), 15);
    let doc = write(dir.path(), "doc.md", "clean\n");
    let canaries = write(dir.path(), "canaries.txt", &format!("{short}\n"));
    let raw = write(dir.path(), "raw.txt", &format!("planted {short}\n"));

    let output = run(&[
        "scan",
        "--document",
        &doc.display().to_string(),
        "--canary-file",
        &canaries.display().to_string(),
        "--raw-capture",
        &raw.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("shorter than"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn scan_refuses_an_empty_canary_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = write(dir.path(), "doc.md", "clean\n");
    let canaries = write(dir.path(), "canaries.txt", "\n\n");
    let raw = write(dir.path(), "raw.txt", "something\n");

    let output = run(&[
        "scan",
        "--document",
        &doc.display().to_string(),
        "--canary-file",
        &canaries.display().to_string(),
        "--raw-capture",
        &raw.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("empty"), "{}", stderr(&output));
}

#[test]
fn scan_refuses_an_empty_raw_capture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = write(dir.path(), "doc.md", "clean\n");
    let canaries = write(dir.path(), "canaries.txt", &format!("{CANARY}\n"));
    let raw = write(dir.path(), "raw.txt", "   \n");

    let output = run(&[
        "scan",
        "--document",
        &doc.display().to_string(),
        "--canary-file",
        &canaries.display().to_string(),
        "--raw-capture",
        &raw.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("empty"), "{}", stderr(&output));
}

#[test]
fn scan_never_echoes_the_canary_it_was_given() {
    // A tool that proves a secret was redacted by printing the secret is its
    // own leak, and its output routinely ends up in a summary document.
    let dir = tempfile::tempdir().expect("tempdir");
    let doc = write(dir.path(), "doc.md", "clean\n");
    let canaries = write(dir.path(), "canaries.txt", &format!("{CANARY}\n"));
    let raw = write(dir.path(), "raw.txt", &format!("seeded {CANARY}\n"));

    let output = run(&[
        "scan",
        "--document",
        &doc.display().to_string(),
        "--canary-file",
        &canaries.display().to_string(),
        "--raw-capture",
        &raw.display().to_string(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        !combined.contains(CANARY),
        "scan echoed the canary: {combined}"
    );
}

// ── redact ──────────────────────────────────────────────────────────────────

#[test]
fn redact_removes_the_secret_and_reports_the_hit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let secret = "xoxb-f24-fixture-not-a-real-credential";
    let input = write(dir.path(), "raw.txt", &format!("token={secret}\nrest\n"));
    let secrets = write(dir.path(), "secrets.txt", &format!("{secret}\n"));
    let out = dir.path().join("redacted.txt");

    let output = run(&[
        "redact",
        "--input",
        &input.display().to_string(),
        "--output",
        &out.display().to_string(),
        "--secrets-file",
        &secrets.display().to_string(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let written = std::fs::read_to_string(&out).expect("read output");
    assert!(!written.contains(secret), "{written}");
    assert!(written.contains("[REDACTED]"), "{written}");
    assert!(
        stdout(&output).contains("REDACTED input="),
        "{}",
        stdout(&output)
    );
}

#[test]
fn redact_refuses_a_seven_byte_secret_rather_than_dropping_it() {
    // The pre-existing `from_secret` filters a short secret away and reports
    // success, so the caller believes it redacted. That is the fail-open this
    // entry point must NOT inherit.
    let dir = tempfile::tempdir().expect("tempdir");
    let short = "abc1234";
    assert_eq!(short.len(), 7);
    let input = write(dir.path(), "raw.txt", &format!("token={short}\n"));
    let secrets = write(dir.path(), "secrets.txt", &format!("{short}\n"));
    let out = dir.path().join("redacted.txt");

    let output = run(&[
        "redact",
        "--input",
        &input.display().to_string(),
        "--output",
        &out.display().to_string(),
        "--secrets-file",
        &secrets.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("fail-open"), "{}", stderr(&output));
    assert!(
        !out.exists(),
        "a refused redaction must not leave an output file behind"
    );
}

#[test]
fn redact_refuses_an_empty_secrets_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = write(dir.path(), "raw.txt", "anything\n");
    let secrets = write(dir.path(), "secrets.txt", "\n \n");
    let out = dir.path().join("redacted.txt");

    let output = run(&[
        "redact",
        "--input",
        &input.display().to_string(),
        "--output",
        &out.display().to_string(),
        "--secrets-file",
        &secrets.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("empty"), "{}", stderr(&output));
}

// ── bind ────────────────────────────────────────────────────────────────────

#[test]
fn bind_accepts_three_platforms_at_one_candidate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let commit = "a".repeat(40);
    let digest = "b".repeat(64);
    let l = write_receipt(dir.path(), "l.json", &receipt("linux", &commit, &digest));
    let m = write_receipt(dir.path(), "m.json", &receipt("macos", &commit, &digest));
    let w = write_receipt(dir.path(), "w.json", &receipt("windows", &commit, &digest));

    let output = run(&[
        "bind",
        "--receipt",
        &l.display().to_string(),
        "--receipt",
        &m.display().to_string(),
        "--receipt",
        &w.display().to_string(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("BOUND commit=") && stdout(&output).contains("receipts=3"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn bind_refuses_two_receipts_naming_different_commits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let digest = "b".repeat(64);
    let l = write_receipt(
        dir.path(),
        "l.json",
        &receipt("linux", &"a".repeat(40), &digest),
    );
    let m = write_receipt(
        dir.path(),
        "m.json",
        &receipt("macos", &"e".repeat(40), &digest),
    );

    let output = run(&[
        "bind",
        "--receipt",
        &l.display().to_string(),
        "--receipt",
        &m.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("disagree on the candidate commit"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn bind_refuses_one_platform_measured_twice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let commit = "a".repeat(40);
    let digest = "b".repeat(64);
    let l = write_receipt(dir.path(), "l.json", &receipt("linux", &commit, &digest));
    let l2 = write_receipt(dir.path(), "l2.json", &receipt("linux", &commit, &digest));

    let output = run(&[
        "bind",
        "--receipt",
        &l.display().to_string(),
        "--receipt",
        &l2.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("same platform"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn bind_refuses_a_single_receipt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let l = write_receipt(
        dir.path(),
        "l.json",
        &receipt("linux", &"a".repeat(40), &"b".repeat(64)),
    );
    let output = run(&["bind", "--receipt", &l.display().to_string()]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("at least two receipts"),
        "{}",
        stderr(&output)
    );
}
