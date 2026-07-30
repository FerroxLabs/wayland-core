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
    ARRIVAL_SOURCE_INDEPENDENT_SINK, AdapterCoverage, AdapterDelivery, CANONICAL_STEPS,
    DeliveryCounts, DeliveryIdentity, JourneyReceipt, JourneyStep, RECEIPT_SCHEMA,
    REGISTERED_ADAPTER_TOTAL, VERDICT_TOKENS,
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

/// The three-adapter coverage the repaired journey drives: 12 deliveries split
/// 4/4/4 across three adapters that land on three DISTINCT sink endpoints.
fn coverage_three() -> AdapterCoverage {
    AdapterCoverage {
        registered_total: REGISTERED_ADAPTER_TOTAL,
        exercised: vec![
            AdapterDelivery {
                adapter: "slack".into(),
                endpoint: "chat.postMessage".into(),
                submitted: 4,
                arrived: 4,
                unique: 4,
            },
            AdapterDelivery {
                adapter: "whatsapp".into(),
                endpoint: "whatsapp.messages".into(),
                submitted: 4,
                arrived: 4,
                unique: 4,
            },
            AdapterDelivery {
                adapter: "sms".into(),
                endpoint: "twilio.messages".into(),
                submitted: 4,
                arrived: 4,
                unique: 4,
            },
        ],
    }
}

/// The shape every published Phase 24 receipt actually had: all twelve
/// deliveries on Slack alone. Legitimate, and it must verify — but it must
/// verify as `adapters=1/10`.
fn coverage_slack_only() -> AdapterCoverage {
    AdapterCoverage {
        registered_total: REGISTERED_ADAPTER_TOTAL,
        exercised: vec![AdapterDelivery {
            adapter: "slack".into(),
            endpoint: "chat.postMessage".into(),
            submitted: 12,
            arrived: 12,
            unique: 12,
        }],
    }
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
        delivery_identity: None,
        adapter_coverage: coverage_three(),
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

// ── adapter coverage ────────────────────────────────────────────────────────
//
// Phase 24 published three platform receipts reading `submitted=12 arrived=12
// unique=12 duplicates=0 losses=0`. All three were carried by Slack alone —
// one adapter of ten — and the receipt had no field that could say so, so the
// three-platform matrix read as a delivery matrix it never was.
//
// These tests are the structural repair in the other direction: a single-adapter
// run can no longer be read as a matrix, because the receipt must name its
// adapters and the verifier prints the fraction.

#[test]
fn verify_prints_the_adapter_fraction_so_a_reader_cannot_infer_a_matrix() {
    // POSITIVE CONTROL. A three-adapter journey verifies AND says 3/10.
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
    let line = stdout(&output);
    assert!(line.contains("adapters=3/10"), "{line}");
    assert!(line.contains("exercised=slack,sms,whatsapp"), "{line}");
}

#[test]
fn verify_accepts_a_one_adapter_journey_but_reports_it_as_one_of_ten() {
    // The published Phase 24 shape. It is a legitimate journey and refusing it
    // would invent a stricter criterion than the one written down — but the
    // success line now makes the narrowness unmissable, which is the entire
    // repair. Before this field existed, THIS receipt and the 3/10 receipt
    // above produced byte-identical success lines.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    r.adapter_coverage = coverage_slack_only();
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
    assert!(output.status.success(), "{}", stderr(&output));
    let line = stdout(&output);
    assert!(line.contains("adapters=1/10"), "{line}");
    assert!(line.contains("exercised=slack"), "{line}");
}

#[test]
fn verify_refuses_a_receipt_with_no_adapter_coverage_field_at_all() {
    // THE REGRESSION GUARD for every receipt Phase 24 actually published. A
    // receipt that omits the field is not neutral about coverage — with nothing
    // to read it against it reads as the whole population — so it is refused at
    // parse rather than accepted and silently under-reported.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let mut value =
        serde_json::to_value(receipt("linux", &commit, &digest)).expect("receipt to value");
    value
        .as_object_mut()
        .expect("object")
        .remove("adapter_coverage")
        .expect("field was present before removal");
    let path = write(
        dir.path(),
        "r.json",
        &serde_json::to_string_pretty(&value).expect("serialise"),
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
        stderr(&output).contains("adapter_coverage"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_a_receipt_naming_zero_exercised_adapters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    r.adapter_coverage.exercised.clear();
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
        stderr(&output).contains("zero exercised adapters"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_a_breakdown_that_does_not_sum_to_the_headline_counts() {
    // The subtlest forgery this field permits: an honest-looking three-adapter
    // list beside a twelve-delivery headline that only one of them produced.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    r.adapter_coverage.exercised[1].submitted = 1; // 4+1+4 = 9, headline says 12
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
        stderr(&output).contains("per-adapter submitted sums to 9"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_three_adapters_that_all_landed_on_one_endpoint() {
    // Configuring three adapters and exercising one is exactly the defect one
    // level up. The endpoint is OBSERVED at the sink, so it is the field that
    // can tell a real three-adapter run from three names on one code path.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    for entry in &mut r.adapter_coverage.exercised {
        entry.endpoint = "chat.postMessage".into();
    }
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
        stderr(&output).contains("share the observed endpoint"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_refuses_an_idle_adapter_listed_to_inflate_the_fraction() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    r.adapter_coverage.exercised.push(AdapterDelivery {
        adapter: "telegram".into(),
        endpoint: "sendMessage".into(),
        submitted: 0,
        arrived: 0,
        unique: 0,
    });
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
        stderr(&output).contains("is not coverage"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn verify_min_adapters_refuses_a_one_adapter_run_and_admits_a_three_adapter_one() {
    // §3b-iii: the control in BOTH directions, in one test. A gate that cannot
    // pass proves as little as one that cannot fail, and `--min-adapters` is
    // exactly the shape that tempts a permanently-red threshold.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);

    let mut narrow = receipt("linux", &commit, &digest);
    narrow.adapter_coverage = coverage_slack_only();
    let narrow_path = write_receipt(dir.path(), "narrow.json", &narrow);
    let wide_path = write_receipt(dir.path(), "wide.json", &receipt("linux", &commit, &digest));

    let refused = run(&[
        "verify",
        "--receipt",
        &narrow_path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &commit,
        "--min-adapters",
        "3",
    ]);
    assert!(!refused.status.success(), "CAN IT FAIL? it must");
    assert!(
        stderr(&refused).contains("exercises 1 adapter(s) of 10"),
        "{}",
        stderr(&refused)
    );

    let admitted = run(&[
        "verify",
        "--receipt",
        &wide_path.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "linux",
        "--expect-commit",
        &commit,
        "--min-adapters",
        "3",
    ]);
    assert!(
        admitted.status.success(),
        "CAN IT PASS? it must — {}",
        stderr(&admitted)
    );
    assert!(
        stdout(&admitted).contains("adapters=3/10"),
        "{}",
        stdout(&admitted)
    );
}

#[test]
fn bind_refuses_platforms_that_each_exercised_a_different_adapter() {
    // Three platforms each driving a different single adapter would otherwise
    // bind, and the union would read as a three-adapter matrix that no single
    // platform ever ran.
    let dir = tempfile::tempdir().expect("tempdir");
    let commit = "a".repeat(40);
    let digest = "b".repeat(64);

    let l = write_receipt(dir.path(), "l.json", &receipt("linux", &commit, &digest));
    let mut m = receipt("macos", &commit, &digest);
    m.adapter_coverage = coverage_slack_only();
    let m = write_receipt(dir.path(), "m.json", &m);

    let output = run(&[
        "bind",
        "--receipt",
        &l.display().to_string(),
        "--receipt",
        &m.display().to_string(),
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("disagree on which adapters were exercised"),
        "{}",
        stderr(&output)
    );
}

// ── the delivery-leg verdict, and cross-gate agreement ──────────────────────
//
// Until this lane, BOTH gates refused any `duplicates != 0`: `verify_counts`
// here and `assertFinalReconciliation` in `scripts/f24-journey.mjs`. Exactly-once
// is scoped to a DELIVERY IDENTITY, `cron:{job}:{scheduled_millis}`
// (`wcore-cron/src/runner.rs:327`), not to a message body, and every job the
// journey submits carries `--trigger every:15` which `trigger.rs:238`
// rate-floors to sixty seconds. Windows crosses that period on every
// kill-and-recover — Task Scheduler's minimum repetition is `PT1M` — while
// launchd and systemd restart inside it. So the Windows journey had **no
// reachable pass state**, and a gate that cannot pass proves as little as one
// that cannot fail while additionally hiding real progress.
//
// The four fixtures below are REAL DRIVER OUTPUT, produced by
// `scripts/f24-journey-quadrants.mjs` through the driver's own `receipt()` path
// from synthetic arrival journals, and committed. Both gates grade the same
// bytes:
//
//   q1 recurrence    -> PASS   (the state that was unreachable)
//   q2 replay        -> FAIL   (a real exactly-once violation)
//   q3 indeterminate -> FAIL   (unprovable is not clean)
//   q4 clean         -> PASS   (the gate must still grade quiet runs)
//
// q1, q2 and q3 carry a BYTE-IDENTICAL headline — `submitted=12 arrived=24
// unique=12 duplicates=12 losses=0`. Nothing but the identity block separates a
// correct slow-platform run from a duplicate-delivering one, which is exactly
// why the field had to become part of the verified receipt rather than a
// decoration `serde` discarded.

fn quadrant_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("journey-quadrants")
        .join(format!("{name}.json"))
}

/// Load a committed quadrant receipt and re-point its digest at a binary staged
/// in `dir`, so the digest check passes and the test grades the DELIVERY LEG
/// rather than re-testing the hasher.
fn staged_quadrant(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    let path = quadrant_fixture(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("quadrant fixture {} must exist: {error}", path.display()));
    let (binary, digest) = stage_binary(dir, name.as_bytes());
    let mut value: serde_json::Value = serde_json::from_str(&raw).expect("fixture parses");
    value["binary_sha256"] = serde_json::Value::String(digest);
    let staged = write(
        dir,
        &format!("{name}.json"),
        &serde_json::to_string_pretty(&value).expect("serialise"),
    );
    (staged, binary)
}

fn verify_quadrant(name: &str) -> (bool, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (receipt, binary) = staged_quadrant(dir.path(), name);
    let output = run(&[
        "verify",
        "--receipt",
        &receipt.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "windows",
        "--expect-commit",
        &"a".repeat(40),
    ]);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    (output.status.success(), text)
}

/// Pull the one `verdict=<TOKEN>` the tool emitted. Refuses zero and refuses two
/// — an extractor that silently accepts either is the self-passing shape in a
/// regex's clothing, and this token is the whole basis of the agreement claim.
fn verdict_token(text: &str) -> &'static str {
    let found: Vec<&'static str> = VERDICT_TOKENS
        .iter()
        .copied()
        .filter(|token| text.contains(&format!("verdict={token}")))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one verdict token, found {found:?} in: {text}"
    );
    found[0]
}

#[test]
fn quadrant_1_a_windows_run_of_proven_recurrences_verifies() {
    // THE POINT OF THE CHANGE, at the process boundary. Twelve repeated bodies,
    // every one under a distinct delivery identity. Before this lane the tool
    // exited non-zero on this receipt and there was no receipt it would have
    // accepted from a Windows kill-and-recover run.
    let (ok, text) = verify_quadrant("q1-recurrence-passes");
    assert!(ok, "CAN IT PASS? it must — {text}");
    assert_eq!(verdict_token(&text), "RECURRENCE", "{text}");
    // And it says so, loudly, so nobody reads a passing `duplicates=12` as
    // twelve tolerated duplicates.
    assert!(text.contains("duplicates=12"), "{text}");
    assert!(
        text.contains("repeats=12 (replays=0 recurrences=12 indeterminate=0 unidentified=0)"),
        "{text}"
    );
}

#[test]
fn quadrant_2_a_planted_replay_is_refused() {
    // Identical headline to q1. Only the identity block differs, so this test
    // cannot pass for any reason other than the replay being detected.
    let (ok, text) = verify_quadrant("q2-replay-fails");
    assert!(!ok, "CAN IT FAIL? it must — {text}");
    assert_eq!(verdict_token(&text), "EXACTLY-ONCE-VIOLATED", "{text}");
    assert!(text.contains("already been delivered"), "{text}");
}

#[test]
fn quadrant_3_indeterminate_repeats_are_refused() {
    // The real Windows adapter mix. `twilio.messages` and `whatsapp.messages`
    // emit no key, so eight repeats cannot be judged — and a gate that quietly
    // passed them would publish an unmeasurable property as a measured clean
    // one, which is the F24-GWP-M1 defect one level down.
    let (ok, text) = verify_quadrant("q3-indeterminate-fails");
    assert!(!ok, "CAN IT FAIL? it must — {text}");
    assert_eq!(verdict_token(&text), "NOT-PROVEN", "{text}");
    assert!(
        text.contains("twilio.messages,whatsapp.messages"),
        "the refusal must NAME the adapters that cannot be graded: {text}"
    );
}

#[test]
fn quadrant_4_a_clean_fast_platform_run_still_verifies() {
    // The regression direction. Admitting recurrences must not have turned this
    // into a gate that only knows how to grade runs WITH repeats.
    let (ok, text) = verify_quadrant("q4-clean-passes");
    assert!(ok, "CAN IT PASS? it must — {text}");
    assert_eq!(verdict_token(&text), "NO-REPEATS", "{text}");
    assert!(text.contains("duplicates=0 losses=0"), "{text}");
}

#[test]
fn the_verifier_and_the_driver_return_the_same_verdict_on_the_same_receipt() {
    // The claim this lane exists to make good on, as an executable comparison
    // rather than an assertion in a summary.
    //
    // The driver's verdict is NOT recomputed here. It is read from the sidecar
    // `.driver-verdict.txt` that `scripts/f24-journey-quadrants.mjs` produced by
    // invoking the JavaScript gate itself over these exact bytes. So the two
    // sides compared are a RECORD the JavaScript gate made and a JUDGEMENT the
    // Rust gate makes now. Recomputing the driver's half in Rust would compare
    // the Rust implementation with itself, which is the tautology this test is
    // supposed to be the opposite of.
    //
    // The sidecar is deliberately not a receipt field: the verifier must reach
    // its conclusion from the counts alone, never from the driver's answer.
    let mut compared = 0;
    for (name, expected, driver_should_pass) in [
        ("q1-recurrence-passes", "RECURRENCE", true),
        ("q2-replay-fails", "EXACTLY-ONCE-VIOLATED", false),
        ("q3-indeterminate-fails", "NOT-PROVEN", false),
        ("q4-clean-passes", "NO-REPEATS", true),
    ] {
        let sidecar = quadrant_fixture(name).with_file_name(format!("{name}.driver-verdict.txt"));
        let recorded = std::fs::read_to_string(&sidecar).unwrap_or_else(|error| {
            panic!(
                "the driver's recorded verdict must exist at {} — regenerate with \
                 `node scripts/f24-journey-quadrants.mjs --write`: {error}",
                sidecar.display()
            )
        });
        let driver = verdict_token(&recorded);

        // The verifier's verdict, from the compiled tool, right now.
        let (verifier_passed, text) = verify_quadrant(name);
        let verifier = verdict_token(&text);

        assert_eq!(
            driver, verifier,
            "{name}: the driver recorded verdict={driver} and the verifier returned \
             verdict={verifier}. Two gates that disagree on one receipt make every grade \
             downstream unreadable, and whichever gate a reader happens to run decides the answer"
        );
        // Agreeing on the TOKEN is not enough — they must also agree on the
        // pass/fail it implies, or the tokens are decoration over two different
        // decisions. The driver's is recoverable from its sidecar: it throws on
        // a refusal, and only a refusal carries the closing sentence.
        let driver_passed = !recorded.contains("The receipt was written and records the true");
        assert_eq!(
            driver_passed, verifier_passed,
            "{name}: driver_passed={driver_passed} verifier_passed={verifier_passed}"
        );
        assert_eq!(driver_passed, driver_should_pass, "{name}: wrong quadrant");
        assert_eq!(driver, expected, "{name}: neither gate matched the quadrant");
        compared += 1;
    }
    // §"a skip is not a pass": count the pairs that actually ran. A loop over a
    // list that silently shortened would otherwise report four quadrants proved
    // while proving none.
    assert_eq!(compared, 4, "all four quadrants must have been compared");
}

#[test]
fn a_receipt_with_repeats_and_no_identity_block_is_refused() {
    // The hole the new pass state would otherwise open: strip the classification
    // from the passing q1 receipt and it must stop passing. Without this, any
    // driver could reach green by simply not saying what its repeats were.
    let dir = tempfile::tempdir().expect("tempdir");
    let (staged, binary) = staged_quadrant(dir.path(), "q1-recurrence-passes");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&staged).expect("read")).expect("parse");
    value
        .as_object_mut()
        .expect("object")
        .remove("delivery_identity")
        .expect("q1 carries the block before removal");
    let stripped = write(
        dir.path(),
        "stripped.json",
        &serde_json::to_string_pretty(&value).expect("serialise"),
    );

    let output = run(&[
        "verify",
        "--receipt",
        &stripped.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "windows",
        "--expect-commit",
        &"a".repeat(40),
    ]);
    assert!(!output.status.success(), "{}", stdout(&output));
    let text = stderr(&output);
    assert_eq!(verdict_token(&text), "UNCLASSIFIED-REPEATS", "{text}");
}

#[test]
fn a_forged_classification_that_does_not_partition_the_repeats_is_refused() {
    // The other way to forge a pass: keep the block but under-report. The buckets
    // must sum to `duplicates`, so "call them all recurrences" is the only lie
    // that survives arithmetic — and that lie is exactly what q2 and q3 catch.
    let dir = tempfile::tempdir().expect("tempdir");
    let (staged, binary) = staged_quadrant(dir.path(), "q1-recurrence-passes");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&staged).expect("read")).expect("parse");
    value["delivery_identity"]["recurrences"] = serde_json::json!(11);
    let forged = write(
        dir.path(),
        "forged.json",
        &serde_json::to_string_pretty(&value).expect("serialise"),
    );

    let output = run(&[
        "verify",
        "--receipt",
        &forged.display().to_string(),
        "--binary",
        &binary.display().to_string(),
        "--expect-platform",
        "windows",
        "--expect-commit",
        &"a".repeat(40),
    ]);
    assert!(!output.status.success(), "{}", stdout(&output));
    let text = stderr(&output);
    assert_eq!(verdict_token(&text), "CLASSIFICATION-UNRECONCILED", "{text}");
}

#[test]
fn a_loss_is_still_refused_now_that_repeats_can_pass() {
    // Losses and repeats were folded into one blanket refusal. Splitting the
    // repeat half out must not have taken the loss half with it — a delivery
    // that never arrived is not made acceptable by anything about identity.
    let dir = tempfile::tempdir().expect("tempdir");
    let (binary, digest) = stage_binary(dir.path(), b"pretend this is wayland-core");
    let commit = "a".repeat(40);
    let mut r = receipt("linux", &commit, &digest);
    r.counts = DeliveryCounts {
        submitted: 12,
        arrived: 11,
        unique: 11,
        duplicates: 0,
        losses: 1,
    };
    r.delivery_identity = Some(DeliveryIdentity::default());
    for entry in &mut r.adapter_coverage.exercised {
        entry.arrived = 4;
        entry.unique = 4;
    }
    r.adapter_coverage.exercised[2].arrived = 3;
    r.adapter_coverage.exercised[2].unique = 3;
    let path = write_receipt(dir.path(), "loss.json", &r);

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
    assert!(!output.status.success(), "{}", stdout(&output));
    let text = stderr(&output);
    assert_eq!(verdict_token(&text), "DELIVERY-LOSS", "{text}");
}

#[test]
fn bind_reports_the_adapter_fraction_it_bound() {
    // CAN IT PASS? The companion to the refusal above. `bind` printing
    // `platforms=3` while silent on adapters is what let a three-platform,
    // one-adapter set read as complete.
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
    let line = stdout(&output);
    assert!(line.contains("receipts=3"), "{line}");
    assert!(line.contains("platforms=linux,macos,windows"), "{line}");
    assert!(line.contains("adapters=3/10"), "{line}");
    assert!(line.contains("exercised=slack,sms,whatsapp"), "{line}");
}
