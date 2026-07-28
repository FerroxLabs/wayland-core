//! Phase 24 Success Criterion 5 — the setup-to-recovery journey receipt.
//!
//! A journey receipt is the machine-checkable record of ONE ordered
//! setup-to-recovery sequence driven against the real shipped binary on ONE
//! platform. It exists because a criterion that reads "setup-to-recovery
//! journeys pass on macOS, Linux, and Windows" cannot be satisfied by a test
//! matrix, and because a prose claim that a journey ran is exactly the shape
//! this program has repeatedly found to be unfalsifiable.
//!
//! The receipt is deliberately NOT routed through
//! [`crate::receipt_policy::verify_authoritative_receipt`]: that verifier means
//! "authoritative signed release evidence" and demands a provider, a model, a
//! fixture digest and a set of required evaluation cells. A journey has none of
//! those and this phase holds no signing key, so widening that surface would
//! blur an authority boundary that exists on purpose. It reuses the DISCIPLINE
//! — bind the run to an exact source identity and to the exact binary bytes —
//! and nothing else.
//!
//! # What the verifier refuses, and why each refusal exists
//!
//! * **A step list that is not exactly the canonical ordered list.** A journey
//!   that drops the hard kill is not a shorter journey, it is a different one.
//! * **A step with an empty command or empty captured output.** An assertion
//!   against empty output is the self-passing shape this repository has caught
//!   more than once; a step that captured nothing did not run.
//! * **An `arrival_source` other than [`ARRIVAL_SOURCE_INDEPENDENT_SINK`].** A
//!   runtime counting deliveries out of its own ledger is grading its own
//!   homework. The count has to come from a process the runtime does not
//!   control and cannot restart.
//! * **A recorded binary digest that differs from the digest the verifier
//!   computes itself.** The verifier hashes the file, so the check does not
//!   depend on the driver that wrote the receipt. This also closes the
//!   "artifact newer than its source" trap: a stale binary has a stale digest.
//! * **Counts that do not reconcile.** `duplicates` and `losses` are DERIVED
//!   from submitted/arrived/unique, so a receipt cannot assert zero of either
//!   while carrying numbers that say otherwise.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The one ordered step list. Every platform runs exactly these, in exactly
/// this order; a platform difference appears in HOW a step is invoked, never in
/// WHICH steps exist.
pub const CANONICAL_STEPS: [&str; 17] = [
    "preflight-clean",
    "binary-identity",
    "profile-setup",
    "sink-start",
    "gateway-install",
    "gateway-start",
    "status-running",
    "automation-add",
    "deliveries-submit",
    "arrival-before-kill",
    "hard-kill",
    "platform-recover",
    "delivery-reconcile",
    "upgrade-in-place",
    "rollback",
    "redaction-canary",
    "drain-uninstall-clean",
];

/// The only arrival source a receipt may claim. See the module docs.
pub const ARRIVAL_SOURCE_INDEPENDENT_SINK: &str = "independent-sink";

/// The schema tag a receipt must carry, so a future shape change is a loud
/// parse refusal rather than a silently different meaning.
pub const RECEIPT_SCHEMA: &str = "wayland.journey.receipt/1";

/// The shortest secret the redactor will accept. The existing
/// [`crate::redaction::SecretRedactor::from_secret`] SILENTLY DROPS anything
/// shorter, which is a fail-open: the caller believes it redacted and it did
/// not. The journey entry point refuses instead.
pub const MIN_SECRET_BYTES: usize = 8;

/// The shortest canary the scan will accept. A short needle makes "absent from
/// the document" meaningless — a four-byte sentinel is absent from most
/// documents by luck.
pub const MIN_CANARY_BYTES: usize = 16;

/// One journey step as the driver captured it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JourneyStep {
    /// Must equal the corresponding entry of [`CANONICAL_STEPS`].
    pub name: String,
    /// The exact command the driver ran. Never synthesised after the fact.
    pub command: String,
    /// The verbatim captured output, pre-publication redaction.
    pub output: String,
    /// The driver's own verdict for the step.
    pub ok: bool,
}

/// The five numbers every recovery claim ends in.
///
/// `arrived` and `unique` are counted at the independent sink. `duplicates` and
/// `losses` are DERIVED here rather than trusted from the receipt, which is why
/// a receipt cannot claim `duplicates: 0` while carrying `arrived > unique`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DeliveryCounts {
    /// Deliveries the journey submitted through the shipped binary.
    pub submitted: u64,
    /// Total arrival records at the independent sink.
    pub arrived: u64,
    /// Distinct arrival bodies at the independent sink.
    pub unique: u64,
    /// Claimed duplicates. Must equal `arrived - unique`.
    pub duplicates: u64,
    /// Claimed losses. Must equal `submitted - unique`.
    pub losses: u64,
}

/// The journey receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JourneyReceipt {
    /// [`RECEIPT_SCHEMA`].
    pub schema: String,
    /// `linux` | `macos` | `windows`.
    pub platform: String,
    /// The platform's own service mechanism that performed the recovery.
    pub service_family: String,
    /// The source commit the DRIVEN BINARY was built from, read out of the
    /// binary itself (`wayland-core --build-info`) rather than from whatever
    /// the host's checkout happens to be sitting at. Those two diverge exactly
    /// when a stale binary is being driven, which is the case worth catching.
    pub candidate_commit: String,
    /// The driven binary's own reported version string.
    pub binary_version: String,
    /// sha256 of the driven binary, as the driver saw it.
    pub binary_sha256: String,
    /// The commit of the instrumentation (this driver + this tool). Recorded
    /// separately because the instrumentation and the product candidate are
    /// different things and conflating them is how a journey ends up proving
    /// the harness.
    pub driver_commit: String,
    /// RFC3339 UTC.
    pub started_at: String,
    /// RFC3339 UTC.
    pub finished_at: String,
    /// Must be [`ARRIVAL_SOURCE_INDEPENDENT_SINK`].
    pub arrival_source: String,
    /// The five numbers.
    pub counts: DeliveryCounts,
    /// The ordered steps.
    pub steps: Vec<JourneyStep>,
}

/// Every way a receipt can be refused. One variant per refusal so a test can
/// name the one it expects rather than matching on a string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JourneyError {
    #[error("receipt is empty")]
    EmptyReceipt,
    #[error("receipt is not parsable: {0}")]
    Unparsable(String),
    #[error("receipt schema is {found:?}, expected {expected:?}")]
    WrongSchema { found: String, expected: String },
    #[error("receipt records platform {found:?}, expected {expected:?}")]
    WrongPlatform { found: String, expected: String },
    #[error("receipt records candidate commit {found:?}, expected {expected:?}")]
    WrongCommit { found: String, expected: String },
    #[error("step list mismatch at position {index}: found {found:?}, expected {expected:?}")]
    StepMismatch {
        index: usize,
        found: String,
        expected: String,
    },
    #[error("step list has {found} steps, expected {expected}")]
    StepCount { found: usize, expected: usize },
    #[error("step {name:?} carries an empty command")]
    EmptyCommand { name: String },
    #[error("step {name:?} carries empty captured output")]
    EmptyOutput { name: String },
    #[error("step {name:?} is recorded as failed")]
    StepFailed { name: String },
    #[error("arrival_source is {found:?}; the only accepted value is {expected:?}")]
    WrongArrivalSource { found: String, expected: String },
    #[error(
        "counts do not reconcile: submitted={submitted} arrived={arrived} unique={unique} \
         duplicates={duplicates} (derived {derived_duplicates}) losses={losses} \
         (derived {derived_losses})"
    )]
    CountsUnreconciled {
        submitted: u64,
        arrived: u64,
        unique: u64,
        duplicates: u64,
        derived_duplicates: u64,
        losses: u64,
        derived_losses: u64,
    },
    #[error("journey submitted zero deliveries; a reconciliation over nothing proves nothing")]
    NoDeliveries,
    #[error("delivery reconciliation is not clean: duplicates={duplicates} losses={losses}")]
    DirtyReconciliation { duplicates: u64, losses: u64 },
    #[error("binary digest mismatch: receipt records {recorded}, verifier computed {computed}")]
    DigestMismatch { recorded: String, computed: String },
    // NOT named `source`: thiserror treats a field of that name as the error
    // source and demands it implement `std::error::Error`, which `String` does
    // not. The io error is carried as text so this enum can stay `PartialEq`
    // and a test can name the variant it expects.
    #[error("binary is missing or unreadable at {path}: {reason}")]
    BinaryUnreadable { path: String, reason: String },
}

/// sha256 a file and return the lowercase hex digest.
pub fn sha256_file(path: &Path) -> Result<String, JourneyError> {
    let bytes = std::fs::read(path).map_err(|error| JourneyError::BinaryUnreadable {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, byte| {
        let _ = write!(acc, "{byte:02x}");
        acc
    })
}

/// Parse a receipt, refusing an empty or unparsable one by name.
pub fn parse_receipt(raw: &str) -> Result<JourneyReceipt, JourneyError> {
    if raw.trim().is_empty() {
        return Err(JourneyError::EmptyReceipt);
    }
    let receipt: JourneyReceipt =
        serde_json::from_str(raw).map_err(|error| JourneyError::Unparsable(error.to_string()))?;
    if receipt.schema != RECEIPT_SCHEMA {
        return Err(JourneyError::WrongSchema {
            found: receipt.schema,
            expected: RECEIPT_SCHEMA.to_string(),
        });
    }
    Ok(receipt)
}

/// The whole receipt check, minus the binary digest (which needs the file).
///
/// Split out so the crate tests can exercise every structural refusal without
/// staging a binary on disk for each one.
pub fn verify_structure(
    receipt: &JourneyReceipt,
    expect_platform: &str,
    expect_commit: &str,
) -> Result<(), JourneyError> {
    if receipt.platform != expect_platform {
        return Err(JourneyError::WrongPlatform {
            found: receipt.platform.clone(),
            expected: expect_platform.to_string(),
        });
    }
    if !receipt.candidate_commit.eq_ignore_ascii_case(expect_commit) {
        return Err(JourneyError::WrongCommit {
            found: receipt.candidate_commit.clone(),
            expected: expect_commit.to_string(),
        });
    }
    if receipt.steps.len() != CANONICAL_STEPS.len() {
        return Err(JourneyError::StepCount {
            found: receipt.steps.len(),
            expected: CANONICAL_STEPS.len(),
        });
    }
    for (index, (step, expected)) in receipt.steps.iter().zip(CANONICAL_STEPS).enumerate() {
        if step.name != expected {
            return Err(JourneyError::StepMismatch {
                index,
                found: step.name.clone(),
                expected: expected.to_string(),
            });
        }
        if step.command.trim().is_empty() {
            return Err(JourneyError::EmptyCommand {
                name: step.name.clone(),
            });
        }
        if step.output.trim().is_empty() {
            return Err(JourneyError::EmptyOutput {
                name: step.name.clone(),
            });
        }
        if !step.ok {
            return Err(JourneyError::StepFailed {
                name: step.name.clone(),
            });
        }
    }
    if receipt.arrival_source != ARRIVAL_SOURCE_INDEPENDENT_SINK {
        return Err(JourneyError::WrongArrivalSource {
            found: receipt.arrival_source.clone(),
            expected: ARRIVAL_SOURCE_INDEPENDENT_SINK.to_string(),
        });
    }
    verify_counts(&receipt.counts)
}

/// The count reconciliation, on its own so its refusals are directly testable.
pub fn verify_counts(counts: &DeliveryCounts) -> Result<(), JourneyError> {
    if counts.submitted == 0 {
        return Err(JourneyError::NoDeliveries);
    }
    let derived_duplicates = counts.arrived.saturating_sub(counts.unique);
    let derived_losses = counts.submitted.saturating_sub(counts.unique);
    // `unique` can never exceed either bound; treating that as "reconciled"
    // would let a receipt claim more distinct arrivals than it ever submitted.
    let coherent = counts.unique <= counts.submitted
        && counts.unique <= counts.arrived
        && counts.duplicates == derived_duplicates
        && counts.losses == derived_losses;
    if !coherent {
        return Err(JourneyError::CountsUnreconciled {
            submitted: counts.submitted,
            arrived: counts.arrived,
            unique: counts.unique,
            duplicates: counts.duplicates,
            derived_duplicates,
            losses: counts.losses,
            derived_losses,
        });
    }
    if counts.duplicates != 0 || counts.losses != 0 {
        return Err(JourneyError::DirtyReconciliation {
            duplicates: counts.duplicates,
            losses: counts.losses,
        });
    }
    Ok(())
}

/// The full verification, including hashing the binary the verifier was given.
///
/// The verifier hashes the file ITSELF. Trusting a digest handed to it by the
/// same driver that wrote the receipt would make the check a restatement.
pub fn verify_receipt(
    raw: &str,
    binary: &Path,
    expect_platform: &str,
    expect_commit: &str,
) -> Result<String, JourneyError> {
    let receipt = parse_receipt(raw)?;
    verify_structure(&receipt, expect_platform, expect_commit)?;
    let computed = sha256_file(binary)?;
    if !computed.eq_ignore_ascii_case(&receipt.binary_sha256) {
        return Err(JourneyError::DigestMismatch {
            recorded: receipt.binary_sha256.clone(),
            computed,
        });
    }
    Ok(format!(
        "JOURNEY VERIFIED platform={} commit={} steps={} submitted={} arrived={} unique={} \
         duplicates={} losses={}",
        receipt.platform,
        receipt.candidate_commit,
        receipt.steps.len(),
        receipt.counts.submitted,
        receipt.counts.arrived,
        receipt.counts.unique,
        receipt.counts.duplicates,
        receipt.counts.losses,
    ))
}

/// Every way a set of receipts fails to describe ONE candidate.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BindError {
    #[error("bind needs at least two receipts, got {0}")]
    TooFew(usize),
    #[error("receipts disagree on the candidate commit: {0:?}")]
    CommitDisagreement(Vec<String>),
    #[error("receipts disagree on the driver commit: {0:?}")]
    DriverDisagreement(Vec<String>),
    #[error("two receipts claim the same platform {0:?}")]
    DuplicatePlatform(String),
}

/// The executable form of the three-platform binding discipline: the platforms
/// must be proving ONE candidate, not three different builds that each happened
/// to pass.
pub fn bind_receipts(receipts: &[JourneyReceipt]) -> Result<String, BindError> {
    if receipts.len() < 2 {
        return Err(BindError::TooFew(receipts.len()));
    }
    let commits: BTreeSet<String> = receipts
        .iter()
        .map(|r| r.candidate_commit.to_ascii_lowercase())
        .collect();
    if commits.len() != 1 {
        return Err(BindError::CommitDisagreement(
            commits.into_iter().collect::<Vec<_>>(),
        ));
    }
    let drivers: BTreeSet<String> = receipts
        .iter()
        .map(|r| r.driver_commit.to_ascii_lowercase())
        .collect();
    if drivers.len() != 1 {
        return Err(BindError::DriverDisagreement(
            drivers.into_iter().collect::<Vec<_>>(),
        ));
    }
    // Three receipts that all say `linux` are one platform measured three
    // times, and would satisfy a naive same-commit check.
    let mut seen = BTreeSet::new();
    for receipt in receipts {
        if !seen.insert(receipt.platform.clone()) {
            return Err(BindError::DuplicatePlatform(receipt.platform.clone()));
        }
    }
    let commit = commits.into_iter().next().unwrap_or_default();
    let driver = drivers.into_iter().next().unwrap_or_default();
    Ok(format!(
        "BOUND commit={} driver={} receipts={} platforms={}",
        commit,
        driver,
        receipts.len(),
        seen.into_iter().collect::<Vec<_>>().join(","),
    ))
}

/// Every way the canary scan is refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScanError {
    #[error("canary file is empty")]
    EmptyCanaryFile,
    #[error(
        "canary {0:?} is shorter than {MIN_CANARY_BYTES} bytes; a short needle makes absence meaningless"
    )]
    CanaryTooShort(String),
    #[error("no raw captures were supplied")]
    NoRawCaptures,
    #[error("raw capture {0} is empty")]
    EmptyRawCapture(String),
    #[error("document is empty")]
    EmptyDocument,
    #[error(
        "canary {0:?} is absent from EVERY raw capture — it never travelled a capture path, so its absence from the document proves nothing"
    )]
    ControlAbsent(String),
    #[error("canary {0:?} is PRESENT in the published document")]
    LeakedToDocument(String),
}

/// One canary's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanaryVerdict {
    pub canary: String,
    pub control_present: bool,
    pub published_absent: bool,
}

/// The positive-controlled redaction scan.
///
/// A canary that was never planted is trivially absent, so the absence check
/// alone certifies nothing. Both halves must hold: PRESENT in a real
/// pre-redaction capture, ABSENT from what will be committed.
pub fn scan_canaries(
    document: &str,
    canaries: &[String],
    raw_captures: &[(String, String)],
) -> Result<Vec<CanaryVerdict>, ScanError> {
    if canaries.is_empty() {
        return Err(ScanError::EmptyCanaryFile);
    }
    for canary in canaries {
        if canary.len() < MIN_CANARY_BYTES {
            return Err(ScanError::CanaryTooShort(canary.clone()));
        }
    }
    if raw_captures.is_empty() {
        return Err(ScanError::NoRawCaptures);
    }
    for (name, contents) in raw_captures {
        if contents.trim().is_empty() {
            return Err(ScanError::EmptyRawCapture(name.clone()));
        }
    }
    if document.trim().is_empty() {
        return Err(ScanError::EmptyDocument);
    }
    let mut verdicts = Vec::new();
    for canary in canaries {
        let control_present = raw_captures
            .iter()
            .any(|(_, contents)| contents.contains(canary.as_str()));
        if !control_present {
            return Err(ScanError::ControlAbsent(canary.clone()));
        }
        if document.contains(canary.as_str()) {
            return Err(ScanError::LeakedToDocument(canary.clone()));
        }
        verdicts.push(CanaryVerdict {
            canary: canary.clone(),
            control_present,
            published_absent: true,
        });
    }
    Ok(verdicts)
}

/// Parse a newline-delimited canary file, dropping blank lines only.
pub fn parse_canary_file(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|line| line.trim_end_matches(['\r', '\n']).trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps() -> Vec<JourneyStep> {
        CANONICAL_STEPS
            .iter()
            .map(|name| JourneyStep {
                name: (*name).to_string(),
                command: format!("run {name}"),
                output: format!("output of {name}"),
                ok: true,
            })
            .collect()
    }

    fn receipt() -> JourneyReceipt {
        JourneyReceipt {
            schema: RECEIPT_SCHEMA.to_string(),
            platform: "linux".into(),
            service_family: "systemd".into(),
            candidate_commit: "a".repeat(40),
            binary_version: "wayland-core 0.12.25".into(),
            binary_sha256: "b".repeat(64),
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

    #[test]
    fn a_well_formed_receipt_passes_the_structural_check() {
        // The positive control. Without it every refusal test below could be
        // passing because the fixture is broken rather than because the
        // refusal fires.
        assert_eq!(
            verify_structure(&receipt(), "linux", &"a".repeat(40)),
            Ok(())
        );
    }

    #[test]
    fn a_missing_step_is_refused() {
        let mut r = receipt();
        r.steps.remove(10);
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::StepCount { .. })
        ));
    }

    #[test]
    fn a_reordered_step_list_is_refused() {
        let mut r = receipt();
        r.steps.swap(10, 11);
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::StepMismatch { index: 10, .. })
        ));
    }

    #[test]
    fn an_empty_captured_output_is_refused() {
        let mut r = receipt();
        r.steps[12].output = "   \n".into();
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::EmptyOutput { .. })
        ));
    }

    #[test]
    fn a_runtime_sourced_arrival_count_is_refused() {
        let mut r = receipt();
        r.arrival_source = "gateway-ledger".into();
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::WrongArrivalSource { .. })
        ));
    }

    #[test]
    fn unreconciled_counts_are_refused_even_when_the_receipt_claims_zero() {
        let mut r = receipt();
        r.counts.arrived = 14; // two duplicates, still claiming zero
        assert!(matches!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::CountsUnreconciled { .. })
        ));
    }

    #[test]
    fn a_journey_that_submitted_nothing_is_refused() {
        let mut r = receipt();
        r.counts = DeliveryCounts {
            submitted: 0,
            arrived: 0,
            unique: 0,
            duplicates: 0,
            losses: 0,
        };
        assert_eq!(
            verify_structure(&r, "linux", &"a".repeat(40)),
            Err(JourneyError::NoDeliveries)
        );
    }

    #[test]
    fn a_wrong_platform_is_refused() {
        assert!(matches!(
            verify_structure(&receipt(), "macos", &"a".repeat(40)),
            Err(JourneyError::WrongPlatform { .. })
        ));
    }

    #[test]
    fn a_wrong_commit_is_refused() {
        assert!(matches!(
            verify_structure(&receipt(), "linux", &"d".repeat(40)),
            Err(JourneyError::WrongCommit { .. })
        ));
    }

    #[test]
    fn bind_refuses_receipts_from_different_candidates() {
        let mut second = receipt();
        second.platform = "macos".into();
        second.candidate_commit = "e".repeat(40);
        assert!(matches!(
            bind_receipts(&[receipt(), second]),
            Err(BindError::CommitDisagreement(_))
        ));
    }

    #[test]
    fn bind_refuses_the_same_platform_twice() {
        assert!(matches!(
            bind_receipts(&[receipt(), receipt()]),
            Err(BindError::DuplicatePlatform(_))
        ));
    }

    #[test]
    fn bind_accepts_three_platforms_at_one_candidate() {
        let mut mac = receipt();
        mac.platform = "macos".into();
        let mut win = receipt();
        win.platform = "windows".into();
        let bound = bind_receipts(&[receipt(), mac, win]).expect("one candidate");
        assert!(bound.starts_with("BOUND commit="), "{bound}");
        assert!(bound.contains("receipts=3"), "{bound}");
    }

    #[test]
    fn a_canary_absent_from_every_capture_fails_the_positive_control() {
        let canary = "WLJ-CANARY-0123456789abcdef".to_string();
        let err = scan_canaries(
            "a published document",
            &[canary.clone()],
            &[("raw".into(), "nothing relevant here".into())],
        )
        .unwrap_err();
        assert_eq!(err, ScanError::ControlAbsent(canary));
    }

    #[test]
    fn a_canary_present_in_the_document_fails() {
        let canary = "WLJ-CANARY-0123456789abcdef".to_string();
        let err = scan_canaries(
            &format!("leaked {canary} here"),
            &[canary.clone()],
            &[("raw".into(), format!("planted {canary}"))],
        )
        .unwrap_err();
        assert_eq!(err, ScanError::LeakedToDocument(canary));
    }

    #[test]
    fn a_short_canary_is_refused() {
        let canary = "fifteen-bytes!!".to_string();
        assert_eq!(canary.len(), 15);
        assert_eq!(
            scan_canaries("doc", &[canary.clone()], &[("raw".into(), canary.clone())]),
            Err(ScanError::CanaryTooShort(canary))
        );
    }

    #[test]
    fn a_clean_scan_reports_both_halves() {
        let canary = "WLJ-CANARY-0123456789abcdef".to_string();
        let verdicts = scan_canaries(
            "a published document with nothing sensitive",
            &[canary.clone()],
            &[("raw".into(), format!("planted {canary}"))],
        )
        .expect("clean");
        assert_eq!(
            verdicts,
            vec![CanaryVerdict {
                canary,
                control_present: true,
                published_absent: true,
            }]
        );
    }

    #[test]
    fn an_empty_receipt_is_refused_by_name() {
        assert_eq!(parse_receipt("   "), Err(JourneyError::EmptyReceipt));
    }

    #[test]
    fn a_receipt_with_the_wrong_schema_tag_is_refused() {
        let mut value = serde_json::to_value(receipt()).expect("serialise");
        value["schema"] = serde_json::Value::String("something.else/9".into());
        let raw = serde_json::to_string(&value).expect("serialise");
        assert!(matches!(
            parse_receipt(&raw),
            Err(JourneyError::WrongSchema { .. })
        ));
    }
}
