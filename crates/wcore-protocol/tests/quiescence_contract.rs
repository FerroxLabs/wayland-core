//! Desktop-facing contract tests for the quiesced snapshot lease (wayland#896).
//!
//! The corpus ships serialized happy AND adversarial frames so the real Desktop
//! consumer can replay them. These tests are what keeps that corpus honest:
//! every adversarial fixture is asserted to produce a SPECIFIC verdict, and the
//! valid frame runs in the same test as its known-positive control — a refusal
//! that is never shown to be absent under acceptable input proves nothing about
//! the guard.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::contract::{PRODUCER_COMMAND_TYPES, PRODUCER_EVENT_TYPES, check_contract};
use wcore_protocol::quiescence::{
    QUIESCENCE_PROTOCOL_VERSION, QuiesceCoverage, QuiesceRefusalReason, QuiesceReleaseVerdict,
    QuiesceScope, validate_acquire, validate_release,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/desktop/v1")
}

fn read_line(relative: &str) -> String {
    let bytes = fs::read(root().join(relative))
        .unwrap_or_else(|error| panic!("missing corpus fixture {relative}: {error}"));
    String::from_utf8(bytes)
        .unwrap_or_else(|_| panic!("{relative} must be UTF-8"))
        .lines()
        .next()
        .unwrap_or_else(|| panic!("{relative} must carry one frame"))
        .to_string()
}

fn read_value(relative: &str) -> Value {
    serde_json::from_str(&read_line(relative))
        .unwrap_or_else(|error| panic!("{relative} must be JSON: {error}"))
}

/// Re-run the frame's own boundary validator over a corpus fixture, exactly as
/// the dispatcher does.
fn verdict(relative: &str) -> Option<QuiesceRefusalReason> {
    let value = read_value(relative);
    let version = u16::try_from(value["quiescence_version"].as_u64().unwrap_or(u64::MAX))
        .unwrap_or(u16::MAX);
    let request_id = value["request_id"].as_str().unwrap_or_default();
    let lease_id = value["lease_id"].as_str().unwrap_or_default();
    let session_id = value["session_id"].as_str().unwrap_or_default();
    match value["type"].as_str() {
        Some("quiesce_acquire") => {
            let scope: QuiesceScope = serde_json::from_value(value["scope"].clone())
                .unwrap_or_else(|error| panic!("{relative} scope must decode: {error}"));
            validate_acquire(
                version,
                request_id,
                lease_id,
                session_id,
                &scope,
                value["ttl_ms"].as_u64().unwrap_or(u64::MAX),
            )
        }
        Some("quiesce_release") => validate_release(
            version,
            request_id,
            lease_id,
            session_id,
            value["epoch"].as_str().unwrap_or_default(),
        ),
        other => panic!("{relative} carries an unexpected frame type {other:?}"),
    }
}

#[test]
fn the_published_corpus_is_current() {
    check_contract().expect("regenerate with `wcore-contract generate`");
}

#[test]
fn the_quiescence_surface_is_published_in_both_producer_inventories() {
    for wire in ["quiesce_acquire", "quiesce_release", "quiesce_status"] {
        assert!(
            PRODUCER_COMMAND_TYPES.contains(&wire),
            "{wire} must be a declared producer command"
        );
    }
    for wire in [
        "quiesce_lease_granted",
        "quiesce_lease_released",
        "quiesce_lease_expired",
        "quiesce_status_report",
        "quiesce_refused",
    ] {
        assert!(
            PRODUCER_EVENT_TYPES.contains(&wire),
            "{wire} must be a declared producer event"
        );
    }
}

#[test]
fn the_happy_command_fixtures_decode_through_the_real_command_type() {
    for relative in [
        "commands/quiesce_acquire.json",
        "commands/quiesce_release.json",
        "commands/quiesce_status.json",
    ] {
        let bytes = fs::read(root().join(relative)).expect("fixture");
        serde_json::from_slice::<ProtocolCommand>(&bytes)
            .unwrap_or_else(|error| panic!("{relative} must decode as a command: {error}"));
    }
}

/// A one-root grant would let a host implement "all named profile state" as
/// "the default home, probably". The fixture the consumer replays carries both.
#[test]
fn the_granted_fixture_covers_the_default_home_and_a_named_profile() {
    let value = read_value("events/quiesce_lease_granted.json");
    let coverage: QuiesceCoverage = serde_json::from_value(value["coverage"].clone())
        .expect("coverage must decode");
    assert!(coverage.complete, "a grant always reports complete coverage");
    assert_eq!(coverage.roots.len(), 2);
    let kinds: Vec<&str> = coverage
        .roots
        .iter()
        .map(|root| match &root.identity {
            wcore_protocol::quiescence::QuiesceProfileIdentity::Default => "default",
            wcore_protocol::quiescence::QuiesceProfileIdentity::Named { .. } => "named",
        })
        .collect();
    assert_eq!(kinds, vec!["default", "named"]);
}

/// Every adversarial acquire/release fixture, each with the reason it must
/// produce — and the valid frame in the same run as the control.
#[test]
fn every_adversarial_quiescence_fixture_produces_its_own_refusal() {
    let cases: [(&str, Option<QuiesceRefusalReason>); 8] = [
        // Known-positive control: a well-formed frame must NOT be refused, so a
        // refusal below is the guard firing rather than the validator refusing
        // everything it is handed.
        ("adversarial/quiescence/valid-acquire.jsonl", None),
        (
            "adversarial/quiescence/acquire-unsupported-version.jsonl",
            Some(QuiesceRefusalReason::UnsupportedVersion),
        ),
        (
            "adversarial/quiescence/acquire-empty-profile-selection.jsonl",
            Some(QuiesceRefusalReason::InvalidRequest),
        ),
        (
            "adversarial/quiescence/acquire-traversal-profile-name.jsonl",
            Some(QuiesceRefusalReason::InvalidRequest),
        ),
        (
            "adversarial/quiescence/acquire-unbounded-ttl.jsonl",
            Some(QuiesceRefusalReason::InvalidRequest),
        ),
        (
            "adversarial/quiescence/acquire-zero-ttl.jsonl",
            Some(QuiesceRefusalReason::InvalidRequest),
        ),
        (
            "adversarial/quiescence/release-missing-epoch.jsonl",
            Some(QuiesceRefusalReason::InvalidRequest),
        ),
        (
            "adversarial/quiescence/release-unsupported-version.jsonl",
            Some(QuiesceRefusalReason::UnsupportedVersion),
        ),
    ];
    for (relative, expected) in cases {
        assert_eq!(verdict(relative), expected, "fixture {relative}");
    }
}

#[test]
fn an_unknown_scope_field_fails_the_whole_frame() {
    let line = read_line("adversarial/quiescence/acquire-unknown-scope-field.jsonl");
    let parsed = serde_json::from_str::<ProtocolCommand>(&line);
    assert!(
        parsed.is_err(),
        "an unrecognised scope field must fail the frame, not be silently ignored"
    );
    // Control: the same frame without the extra field decodes.
    let control = read_line("adversarial/quiescence/valid-acquire.jsonl");
    serde_json::from_str::<ProtocolCommand>(&control).expect("the control frame must decode");
}

/// A receipt that decodes cleanly and must still not be acted on.
#[test]
fn a_grant_claiming_incomplete_coverage_is_visible_as_such() {
    let value = read_value("adversarial/quiescence/granted-incomplete-coverage.jsonl");
    let coverage: QuiesceCoverage =
        serde_json::from_value(value["coverage"].clone()).expect("coverage must decode");
    assert!(
        !coverage.complete,
        "the adversarial grant must be recognisable by its own complete flag"
    );
    // Control: the happy grant on the same field reads true, so the assertion
    // above is reading a real signal rather than a default.
    let happy = read_value("events/quiesce_lease_granted.json");
    let happy_coverage: QuiesceCoverage =
        serde_json::from_value(happy["coverage"].clone()).expect("coverage must decode");
    assert!(happy_coverage.complete);
}

#[test]
fn a_mutated_release_receipt_is_distinguishable_from_a_clean_one() {
    let mutated = read_value("adversarial/quiescence/released-mutated.jsonl");
    let verdict: QuiesceReleaseVerdict =
        serde_json::from_value(mutated["verdict"].clone()).expect("verdict must decode");
    assert_eq!(verdict, QuiesceReleaseVerdict::Mutated);
    assert_ne!(mutated["epoch_at_acquire"], mutated["epoch_at_release"]);

    let clean = read_value("events/quiesce_lease_released.json");
    let clean_verdict: QuiesceReleaseVerdict =
        serde_json::from_value(clean["verdict"].clone()).expect("verdict must decode");
    assert_eq!(clean_verdict, QuiesceReleaseVerdict::Clean);
    assert_eq!(clean["epoch_at_acquire"], clean["epoch_at_release"]);
}

#[test]
fn the_published_version_matches_the_code() {
    for relative in [
        "commands/quiesce_acquire.json",
        "commands/quiesce_release.json",
        "commands/quiesce_status.json",
        "events/quiesce_lease_granted.json",
        "events/quiesce_lease_released.json",
        "events/quiesce_lease_expired.json",
        "events/quiesce_status_report.json",
        "events/quiesce_refused.json",
    ] {
        let bytes = fs::read(root().join(relative)).expect("fixture");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(
            value["quiescence_version"].as_u64(),
            Some(u64::from(QUIESCENCE_PROTOCOL_VERSION)),
            "{relative} must publish the version the code speaks"
        );
    }
}
