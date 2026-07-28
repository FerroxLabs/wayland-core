//! Phase 28 certification-receipt contract (F28-03 / F28-04).
//!
//! Deliberately a SEPARATE file from `receipt_contract.rs`. Plan 28-04 does not modify an
//! existing test, and keeping the new rules together makes them readable as a set.
//!
//! Every rule below is proved in BOTH directions: a fixture that TRIPS it and a fixture that
//! does not. A validator only ever shown valid input is untested, and a rule that only ever
//! rejects is equally broken — this repository's own plan-gate linter shipped the disease it
//! hunts four separate times by testing one direction only, and three of its live suites
//! carried a zero-execution guard that was itself `#[ignore]`d.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use wcore_eval_scenarios::receipt::{
    ArtifactBindingV2, CERT_PERMITTED_CLAIMS, CERT_RECEIPT_SCHEMA, CERT_RECEIPT_SCHEMA_VERSION,
    CERT_SKIP_CLASSES, CandidateBinaryV2, CandidateBindingV2, CertAuthority, CertAuthorityClaimV2,
    CertBindingsV2, CertFindingV2, CertificationBodyV2, CertificationReceiptV2,
    CertificationVerifier, CorpusBindingV2, EnvironmentBindingV2, LogBindingV2, PlatformBindingV2,
    PostureBindingV2, SkipPolicyV2, SkippedCellV2,
};

const KEY_ID: &str = "phase-28-certification";
const SCOPE: &str = "phase-scoped: evidence assembly only. NOT a release trust root, NOT a seal.";

fn key() -> SigningKey {
    // Deterministic so a failure is reproducible; this is a test key and signs nothing real.
    SigningKey::from_bytes(&[7u8; 32])
}

fn hex64(seed: u8) -> String {
    format!("{:x}", Sha256::digest([seed]))
}

fn hex40(seed: u8) -> String {
    hex64(seed)[..40].to_string()
}

fn claims(a: bool, b: bool, c: bool) -> BTreeMap<String, bool> {
    let mut m = BTreeMap::new();
    m.insert(CERT_PERMITTED_CLAIMS[0].to_string(), a);
    m.insert(CERT_PERMITTED_CLAIMS[1].to_string(), b);
    m.insert(CERT_PERMITTED_CLAIMS[2].to_string(), c);
    m
}

fn finding(id: &str, sev: &str, criterion: &str, disposition: &str) -> CertFindingV2 {
    let paper = matches!(disposition, "ACCEPTED" | "DEFERRED");
    CertFindingV2 {
        id: id.to_string(),
        origin: "matrix".to_string(),
        subject: "a subject".to_string(),
        inherited_severity: "-".to_string(),
        p28_severity: sev.to_string(),
        contradicted_criterion: criterion.to_string(),
        disposition: disposition.to_string(),
        rationale: "a rationale".to_string(),
        owner: if paper {
            "an owner".into()
        } else {
            String::new()
        },
        backlog_id: if paper { "BL-1".into() } else { String::new() },
        executable_check: if disposition == "FIXED" {
            "a check".into()
        } else {
            String::new()
        },
        counter_evidence: if disposition == "DISPROVED" {
            "counter evidence".into()
        } else {
            String::new()
        },
    }
}

fn bindings() -> CertBindingsV2 {
    CertBindingsV2 {
        candidate: vec![CandidateBindingV2 {
            scope: "matrix".to_string(),
            commit: hex40(1),
            tree: hex40(2),
            ledger_ref: "evidence/28-02/candidate.json".to_string(),
            binaries: vec![CandidateBinaryV2 {
                target: "x86_64-unknown-linux-gnu".to_string(),
                sha256: hex64(3),
                provenance: "CI release artifact".to_string(),
            }],
        }],
        platform: vec![PlatformBindingV2 {
            os_family: "linux".to_string(),
            target: "x86_64-unknown-linux-gnu".to_string(),
            cells_total: 216,
            cells_pass: 216,
            cells_red: 0,
            cells_skipped: 0,
            critical_cells: 49,
            evidence_ref: "evidence/28-02/results.json".to_string(),
        }],
        posture: vec![PostureBindingV2 {
            name: "fail-closed-sandbox".to_string(),
            description: "WAYLAND_SANDBOX=none is an error, not a downgrade".to_string(),
            evidence_ref: "evidence/28-02/results.json".to_string(),
        }],
        fixture_corpus: vec![CorpusBindingV2 {
            name: "e5".to_string(),
            sha256: hex64(4),
            item_count: 9,
            source_ref: "crates/wcore-eval-scenarios/src/e5_cases.rs".to_string(),
        }],
        environment: vec![EnvironmentBindingV2 {
            host: "hetzner-dsm".to_string(),
            os_family: "linux".to_string(),
            os_build: "x86_64".to_string(),
            run_context: "ssh non-interactive".to_string(),
            evidence_ref: "evidence/28-02/results.json".to_string(),
        }],
        artifacts: vec![ArtifactBindingV2 {
            path: "evidence/28-02/results.json".to_string(),
            sha256: hex64(5),
            bytes: 100,
        }],
        logs: vec![LogBindingV2 {
            path: "evidence/28-02/win-matrix.log".to_string(),
            sha256: hex64(6),
            bytes: 100,
            produced_by: "f28-win-matrix.ps1".to_string(),
        }],
        skip_policy: SkipPolicyV2 {
            classes: CERT_SKIP_CLASSES.iter().map(|c| c.to_string()).collect(),
            skipped_cells: vec![],
            skipped_critical_cases: 0,
        },
    }
}

/// The known-good fixture. Every negative case below is this, with exactly one field moved.
fn body() -> CertificationBodyV2 {
    CertificationBodyV2 {
        certification_id: "f28-certification".to_string(),
        phase: "28-native-cross-platform-certification".to_string(),
        bindings: bindings(),
        findings: vec![
            finding("F-A", "HIGH", "1", "FIXED"),
            finding("F-B", "MEDIUM", "-", "DEFERRED"),
        ],
        claims: claims(true, true, true),
    }
}

fn verifier() -> CertificationVerifier {
    let mut v = CertificationVerifier::new();
    v.trust_phase_key(KEY_ID, key().verifying_key());
    v
}

fn code_of(body: CertificationBodyV2) -> String {
    CertificationReceiptV2::unsigned(body)
        .expect_err("expected this fixture to be rejected")
        .code()
}

// ---------------------------------------------------------------------------------------
// The direction that matters first: the KNOWN-GOOD fixture must be ACCEPTED.
// A validator that rejects everything passes every rejection test and is worthless.
// ---------------------------------------------------------------------------------------

#[test]
fn known_good_receipt_is_accepted_and_verifies_under_a_trusted_phase_key() {
    let receipt = CertificationReceiptV2::unsigned(body())
        .expect("the known-good fixture must be accepted")
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);
    assert_eq!(receipt.schema, CERT_RECEIPT_SCHEMA);
    assert_eq!(receipt.schema_version, CERT_RECEIPT_SCHEMA_VERSION);

    let verified = verifier().verify(&receipt).expect("must verify");
    assert_eq!(verified.authority, CertAuthority::PhaseScopedSigned);
    assert!(verified.acceptance_gate_passed);
}

#[test]
fn authority_is_derived_from_an_externally_configured_key_not_from_the_body() {
    // The v1 property this schema must not lose. A receipt is not authoritative because it
    // says so; it is authoritative because a verifier YOU configured trusts the signing key.
    let receipt = CertificationReceiptV2::unsigned(body())
        .unwrap()
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);

    let stranger = CertificationVerifier::new();
    assert_eq!(
        stranger.verify(&receipt).unwrap_err().code(),
        "F28R-KEY",
        "a verifier with no configured key must not derive authority from the receipt"
    );

    let mut wrong = CertificationVerifier::new();
    wrong.trust_phase_key(KEY_ID, SigningKey::from_bytes(&[9u8; 32]).verifying_key());
    assert_eq!(wrong.verify(&receipt).unwrap_err().code(), "F28R-KEY");

    assert_eq!(
        verifier().verify(&receipt).unwrap().authority,
        CertAuthority::PhaseScopedSigned
    );
}

#[test]
fn an_unsigned_receipt_is_structurally_valid_but_carries_no_authority() {
    let receipt = CertificationReceiptV2::unsigned(body()).unwrap();
    assert!(matches!(receipt.authority, CertAuthorityClaimV2::Unsigned));
    let verified = verifier().verify(&receipt).unwrap();
    assert_eq!(verified.authority, CertAuthority::UnverifiedProvenance);
}

// ---------------------------------------------------------------------------------------
// The eight F28-03 bindings, each with its OWN failure code.
// ---------------------------------------------------------------------------------------

/// One binding removed from an otherwise-valid fixture. Named rather than `#[allow]`ed —
/// suppressing a lint to reach a gate is exactly what this phase is not permitted to do.
type BindingMutation = Box<dyn Fn(&mut CertBindingsV2)>;

#[test]
fn each_missing_binding_is_rejected_with_its_own_code() {
    let cases: Vec<(&str, BindingMutation)> = vec![
        (
            "F28R-B01",
            Box::new(|b: &mut CertBindingsV2| b.candidate.clear()),
        ),
        (
            "F28R-B02",
            Box::new(|b: &mut CertBindingsV2| b.platform.clear()),
        ),
        (
            "F28R-B03",
            Box::new(|b: &mut CertBindingsV2| b.posture.clear()),
        ),
        (
            "F28R-B04",
            Box::new(|b: &mut CertBindingsV2| b.fixture_corpus.clear()),
        ),
        (
            "F28R-B05",
            Box::new(|b: &mut CertBindingsV2| b.environment.clear()),
        ),
        (
            "F28R-B06",
            Box::new(|b: &mut CertBindingsV2| b.artifacts.clear()),
        ),
        (
            "F28R-B07",
            Box::new(|b: &mut CertBindingsV2| b.logs.clear()),
        ),
        (
            "F28R-B08",
            Box::new(|b: &mut CertBindingsV2| b.skip_policy.classes.clear()),
        ),
    ];
    assert_eq!(cases.len(), 8, "F28-03 names exactly eight bindings");
    for (expected, mutate) in cases {
        let mut b = body();
        mutate(&mut b.bindings);
        assert_eq!(
            code_of(b),
            expected,
            "removing the binding behind {expected} must be rejected with that exact code"
        );
    }
    // And the negative direction: with all eight present, the same fixture is accepted.
    assert!(CertificationReceiptV2::unsigned(body()).is_ok());
}

// ---------------------------------------------------------------------------------------
// Skip policy — the four classes, and no fifth.
// ---------------------------------------------------------------------------------------

#[test]
fn a_skip_class_outside_the_four_is_rejected_and_each_of_the_four_is_accepted() {
    let mut b = body();
    b.bindings.skip_policy.classes.push("harness-bound".into());
    assert_eq!(code_of(b), "F28R-SKIPCLASS");

    let mut b = body();
    b.bindings.skip_policy.skipped_cells.push(SkippedCellV2 {
        cell_id: "cell-x".into(),
        class: "convenient".into(),
        criticality: "normal".into(),
        required_evidence: "none".into(),
    });
    assert_eq!(code_of(b), "F28R-SKIPCLASS");

    for class in CERT_SKIP_CLASSES {
        let mut b = body();
        b.bindings.skip_policy.skipped_cells.push(SkippedCellV2 {
            cell_id: format!("cell-{class}"),
            class: class.to_string(),
            criticality: "normal".into(),
            required_evidence: "the class's required evidence".into(),
        });
        assert!(
            CertificationReceiptV2::unsigned(b).is_ok(),
            "{class} is one of the four contract classes and must be accepted"
        );
    }
}

#[test]
fn a_skipped_cell_with_no_required_evidence_is_rejected() {
    let mut b = body();
    b.bindings.skip_policy.skipped_cells.push(SkippedCellV2 {
        cell_id: "cell-y".into(),
        class: "platform-inapplicability".into(),
        criticality: "normal".into(),
        required_evidence: "   ".into(),
    });
    assert_eq!(code_of(b), "F28R-SKIPEVID");
}

#[test]
fn a_skipped_critical_case_is_rejected_two_ways_because_criterion_1_forbids_one() {
    let mut b = body();
    b.bindings.skip_policy.skipped_cells.push(SkippedCellV2 {
        cell_id: "sandbox-probes-windows-acp".into(),
        class: "observation-blocked".into(),
        criticality: "critical".into(),
        required_evidence: "a directional control".into(),
    });
    assert_eq!(code_of(b), "F28R-SKIPCRIT");

    // The declared count is checked independently of the cell list, so a receipt cannot
    // declare a nonzero count while listing no cells.
    let mut b = body();
    b.bindings.skip_policy.skipped_critical_cases = 1;
    assert_eq!(code_of(b), "F28R-SKIPCRIT");

    // Negative: a skipped NON-critical case is legal.
    let mut b = body();
    b.bindings.skip_policy.skipped_cells.push(SkippedCellV2 {
        cell_id: "long-paths-linux-unc".into(),
        class: "platform-inapplicability".into(),
        criticality: "normal".into(),
        required_evidence: "UNC paths do not exist on linux".into(),
    });
    assert!(CertificationReceiptV2::unsigned(b).is_ok());
}

// ---------------------------------------------------------------------------------------
// The finding ledger — Criterion 4 as the panel implemented it, plus A1 and A2.
// ---------------------------------------------------------------------------------------

#[test]
fn an_unrecognised_disposition_is_rejected_and_all_five_recognised_ones_are_accepted() {
    for unrecognised in ["", "PENDING", "WONTFIX", "RESOLVED"] {
        let mut b = body();
        b.findings[1].disposition = unrecognised.to_string();
        b.claims = claims(false, true, true);
        assert_eq!(
            code_of(b),
            "F28R-NODISP",
            "{unrecognised:?} is not a recognised disposition"
        );
    }
    for terminal in ["FIXED", "DISPROVED", "ACCEPTED", "DEFERRED"] {
        let mut b = body();
        b.findings[1] = finding("F-B", "MEDIUM", "-", terminal);
        assert!(
            CertificationReceiptV2::unsigned(b).is_ok(),
            "{terminal} is terminal and must be accepted at MEDIUM with no contradiction"
        );
    }
}

#[test]
fn an_open_finding_is_recordable_and_forces_the_receipt_to_report_its_gate_as_not_passed() {
    // The honest outcome must be SAYABLE. An executor holding a HIGH it can neither fix nor
    // disprove must be able to produce a valid receipt that says so, or the schema itself
    // becomes pressure to launder the finding.
    let mut b = body();
    b.findings[1] = finding("F-B", "HIGH", "-", "OPEN");
    b.claims = claims(false, true, false);
    let receipt = CertificationReceiptV2::unsigned(b)
        .expect("a receipt reporting an open HIGH must be valid")
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);
    let verified = verifier().verify(&receipt).unwrap();
    assert_eq!(verified.authority, CertAuthority::PhaseScopedSigned);
    assert!(
        !verified.acceptance_gate_passed,
        "an open HIGH must leave the acceptance gate not passed"
    );

    // And it may NOT claim otherwise: both affected claims are recomputed against the ledger.
    let mut b = body();
    b.findings[1] = finding("F-B", "HIGH", "-", "OPEN");
    b.claims = claims(true, true, false);
    assert_eq!(code_of(b), "F28R-CLAIMFALSE");

    let mut b = body();
    b.findings[1] = finding("F-B", "HIGH", "-", "OPEN");
    b.claims = claims(false, true, true);
    assert_eq!(code_of(b), "F28R-CLAIMFALSE");

    // An open MEDIUM does not touch the CRITICAL/HIGH claim, only the disposition claim.
    let mut b = body();
    b.findings[1] = finding("F-B", "MEDIUM", "-", "OPEN");
    b.claims = claims(false, true, true);
    assert!(CertificationReceiptV2::unsigned(b).is_ok());
}

#[test]
fn accept_and_defer_are_unreachable_at_critical_and_high() {
    for severity in ["CRITICAL", "HIGH"] {
        for disposition in ["ACCEPTED", "DEFERRED"] {
            let mut b = body();
            b.findings[1] = finding("F-B", severity, "-", disposition);
            assert_eq!(
                code_of(b),
                "F28R-PAPERSEV",
                "{disposition} must not be reachable at {severity}"
            );
        }
        // Negative: the two dispositions that ARE available at those severities.
        for disposition in ["FIXED", "DISPROVED"] {
            let mut b = body();
            b.findings[1] = finding("F-B", severity, "-", disposition);
            assert!(CertificationReceiptV2::unsigned(b).is_ok());
        }
    }
}

#[test]
fn a2_closes_the_paper_path_on_a_contradicted_criterion_at_any_recorded_severity() {
    // The load-bearing case: A2 must fire on the CONTRADICTED CRITERION even when the row's
    // severity has been mis-scored downward, so a bad score cannot reopen the accept path.
    for severity in ["MEDIUM", "LOW"] {
        for disposition in ["ACCEPTED", "DEFERRED"] {
            let mut b = body();
            b.findings[1] = finding("F-B", severity, "2", disposition);
            assert_eq!(
                code_of(b),
                "F28R-PAPERA2",
                "A2 must fire on a contradicted criterion at {severity} recorded severity"
            );
        }
    }
    // Negative: the same row with NO contradicted criterion is legal at MEDIUM.
    let mut b = body();
    b.findings[1] = finding("F-B", "MEDIUM", "-", "ACCEPTED");
    assert!(CertificationReceiptV2::unsigned(b).is_ok());
}

#[test]
fn an_empty_contradicted_criterion_is_rejected_so_an_omission_cannot_read_as_a_none() {
    let mut b = body();
    b.findings[1].contradicted_criterion = String::new();
    assert_eq!(code_of(b), "F28R-FIELD");

    let mut b = body();
    b.findings[1].contradicted_criterion = "5".to_string();
    assert_eq!(code_of(b), "F28R-FIELD");

    let mut b = body();
    b.findings[1].contradicted_criterion = "-".to_string();
    assert!(CertificationReceiptV2::unsigned(b).is_ok());
}

#[test]
fn a1_requires_the_inherited_severity_as_provenance_and_the_p28_score_as_the_operative_value() {
    let mut b = body();
    b.findings[1].inherited_severity = String::new();
    assert_eq!(code_of(b), "F28R-PROV");

    let mut b = body();
    b.findings[1].p28_severity = "known-red/non-gating".to_string();
    assert_eq!(
        code_of(b),
        "F28R-SEV",
        "an inherited label is not a Phase 28 severity"
    );

    // Negative: provenance recorded, operative score valid.
    let mut b = body();
    b.findings[1].inherited_severity = "known-red/non-gating".to_string();
    b.findings[1].p28_severity = "MEDIUM".to_string();
    assert!(CertificationReceiptV2::unsigned(b).is_ok());
}

#[test]
fn the_paper_path_requires_an_owner_and_a_backlog_id_and_the_repair_path_requires_evidence() {
    let mut b = body();
    b.findings[1] = finding("F-B", "LOW", "-", "ACCEPTED");
    b.findings[1].owner = String::new();
    assert_eq!(code_of(b), "F28R-PAPEREVID");

    let mut b = body();
    b.findings[1] = finding("F-B", "LOW", "-", "DEFERRED");
    b.findings[1].backlog_id = String::new();
    assert_eq!(code_of(b), "F28R-PAPEREVID");

    let mut b = body();
    b.findings[0].executable_check = String::new();
    assert_eq!(
        code_of(b),
        "F28R-REPAIREVID",
        "a repair is proved by a check, not asserted"
    );

    let mut b = body();
    b.findings[1] = finding("F-B", "HIGH", "-", "DISPROVED");
    b.findings[1].counter_evidence = String::new();
    assert_eq!(code_of(b), "F28R-REPAIREVID");
}

// ---------------------------------------------------------------------------------------
// AMENDMENT A3 — the one place an over-claim would be signed and consumed downstream.
// ---------------------------------------------------------------------------------------

#[test]
fn a_receipt_asserting_zero_known_defects_or_zero_findings_is_rejected_under_a3() {
    for over_claim in [
        "zero_known_defects",
        "zero_findings",
        "no_open_defects",
        "release_approved",
    ] {
        let mut b = body();
        b.claims.insert(over_claim.to_string(), true);
        assert_eq!(
            code_of(b),
            "F28R-OVERCLAIM",
            "{over_claim:?} is outside the three claims A3 permits"
        );
    }
    // Even asserting it FALSE is an over-claim: the vocabulary itself is the problem, because
    // a reader who sees the key infers the certification reasoned about it.
    let mut b = body();
    b.claims.insert("zero_known_defects".to_string(), false);
    assert_eq!(code_of(b), "F28R-OVERCLAIM");
}

#[test]
fn all_three_permitted_claims_must_be_stated_and_exactly_three_exist() {
    assert_eq!(CERT_PERMITTED_CLAIMS.len(), 3);
    for claim in CERT_PERMITTED_CLAIMS {
        let mut b = body();
        b.claims.remove(claim);
        assert_eq!(
            code_of(b),
            "F28R-CLAIMMISS",
            "{claim} must be stated, true or false"
        );
    }
    // Negative: all three stated FALSE is a perfectly legal receipt. A certification that
    // reports its own gate as not passed is the honest outcome, not a rejected one.
    let mut b = body();
    b.claims = claims(false, false, false);
    b.findings[1] = finding("F-B", "HIGH", "2", "FIXED");
    assert!(CertificationReceiptV2::unsigned(b).is_ok());
}

#[test]
fn a_claim_asserted_true_is_recomputed_against_the_ledger_and_rejected_if_it_disagrees() {
    // This is the in-receipt half of "recompute, do not copy". The independent Python
    // verifier recomputes the same three from the RAW evidence, and BOTH must agree for the
    // receipt to stand.

    // zero_undispositioned_findings, contradicted by an OPEN row.
    let mut b = body();
    b.findings[1].disposition = "OPEN".to_string();
    b.findings[1].backlog_id = String::new();
    b.findings[1].owner = String::new();
    b.claims = claims(true, true, true);
    assert_eq!(code_of(b), "F28R-CLAIMFALSE");

    // zero_unresolved_critical_or_high, contradicted by an OPEN HIGH.
    let mut b = body();
    b.findings[1] = finding("F-B", "HIGH", "-", "OPEN");
    b.claims = claims(false, true, true);
    assert_eq!(code_of(b), "F28R-CLAIMFALSE");

    // zero_skipped_critical_cases is enforced even harder: the skip rule rejects the cell
    // outright, so the claim can never be signed over a contradicting ledger by any route.
    let mut b = body();
    b.bindings.skip_policy.skipped_critical_cases = 3;
    assert_eq!(code_of(b), "F28R-SKIPCRIT");

    // Negative direction: a ledger that genuinely supports all three is accepted.
    assert!(CertificationReceiptV2::unsigned(body()).is_ok());
}

// ---------------------------------------------------------------------------------------
// Tamper detection — a verifier nobody has seen say no is a verifier nobody should believe.
// ---------------------------------------------------------------------------------------

#[test]
fn flipping_any_byte_of_the_body_breaks_verification() {
    let receipt = CertificationReceiptV2::unsigned(body())
        .unwrap()
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);
    let json = receipt.to_json().unwrap();
    let v = verifier();
    assert!(v.parse_and_verify(json.as_bytes()).is_ok());

    // 1. Body mutated, digest left stale -> digest mismatch.
    let tampered = json.replace("f28-certification", "f28-certificatioX");
    assert_ne!(tampered, json);
    assert_eq!(
        v.parse_and_verify(tampered.as_bytes()).unwrap_err().code(),
        "F28R-DIGEST"
    );

    // 2. Body AND digest both recomputed by an attacker, but the signature is over the old
    //    digest -> signature failure. This is the case a naive "does the digest match?"
    //    check would wave through.
    let mut forged = receipt.clone();
    forged.body.certification_id = "forged".to_string();
    forged.body_sha256 = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&forged.body).unwrap())
    );
    assert_eq!(v.verify(&forged).unwrap_err().code(), "F28R-SIG");
}

#[test]
fn a_signature_over_a_different_body_does_not_verify() {
    let a = CertificationReceiptV2::unsigned(body())
        .unwrap()
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);
    let mut other = body();
    other.certification_id = "a-different-certification".to_string();
    let mut b = CertificationReceiptV2::unsigned(other).unwrap();
    b.authority = a.authority.clone();
    assert_eq!(verifier().verify(&b).unwrap_err().code(), "F28R-SIG");
}

#[test]
fn a_recorded_public_half_that_disagrees_with_its_fingerprint_is_rejected() {
    let receipt = CertificationReceiptV2::unsigned(body())
        .unwrap()
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);
    let CertAuthorityClaimV2::PhaseScoped {
        key_id,
        public_key_base64,
        signature_base64,
        scope,
        ..
    } = receipt.authority.clone()
    else {
        panic!("expected a phase-scoped claim");
    };
    let mut broken = receipt.clone();
    broken.authority = CertAuthorityClaimV2::PhaseScoped {
        key_id,
        public_key_base64,
        fingerprint_sha256: format!("{:x}", Sha256::digest([0u8])),
        signature_base64,
        scope,
    };
    assert_eq!(
        verifier().verify(&broken).unwrap_err().code(),
        "F28R-FINGERPRINT"
    );
}

#[test]
fn the_recorded_public_half_is_really_the_key_that_is_checked() {
    // Guards against a receipt that records key A, is trusted as key B, and is signed by B —
    // a reader taking the recorded half at face value would be verifying the wrong thing.
    let receipt = CertificationReceiptV2::unsigned(body())
        .unwrap()
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);
    let stranger = SigningKey::from_bytes(&[3u8; 32]).verifying_key();
    let CertAuthorityClaimV2::PhaseScoped {
        key_id,
        signature_base64,
        scope,
        ..
    } = receipt.authority.clone()
    else {
        panic!("expected a phase-scoped claim");
    };
    let mut swapped = receipt.clone();
    swapped.authority = CertAuthorityClaimV2::PhaseScoped {
        key_id,
        public_key_base64: BASE64.encode(stranger.to_bytes()),
        fingerprint_sha256: format!("{:x}", Sha256::digest(stranger.as_bytes())),
        signature_base64,
        scope,
    };
    assert_eq!(verifier().verify(&swapped).unwrap_err().code(), "F28R-KEY");
}

// ---------------------------------------------------------------------------------------
// Fail-closed schema versioning.
// ---------------------------------------------------------------------------------------

#[test]
fn an_unknown_schema_or_version_fails_closed_rather_than_ignoring_the_new_sections() {
    let mut r = CertificationReceiptV2::unsigned(body()).unwrap();
    r.schema_version = CERT_RECEIPT_SCHEMA_VERSION + 1;
    assert_eq!(verifier().verify(&r).unwrap_err().code(), "F28R-SCHEMA");

    let mut r = CertificationReceiptV2::unsigned(body()).unwrap();
    r.schema = "wayland.eval.receipt".to_string();
    assert_eq!(verifier().verify(&r).unwrap_err().code(), "F28R-SCHEMA");
}

#[test]
fn an_unknown_field_anywhere_in_the_body_is_rejected_rather_than_silently_dropped() {
    let receipt = CertificationReceiptV2::unsigned(body())
        .unwrap()
        .sign_phase_scoped(KEY_ID, &key(), SCOPE);
    let json = receipt.to_json().unwrap();
    let with_extra = json.replacen(
        "\"certification_id\":",
        "\"future_section\": {\"x\": 1},\n    \"certification_id\":",
        1,
    );
    assert_ne!(with_extra, json);
    assert_eq!(
        verifier()
            .parse_and_verify(with_extra.as_bytes())
            .unwrap_err()
            .code(),
        "F28R-JSON",
        "a reader must fail closed on a section it does not understand"
    );
}

#[test]
fn platform_cell_counts_must_sum_so_a_red_cannot_be_dropped_from_the_arithmetic() {
    let mut b = body();
    b.bindings.platform[0].cells_red = 24;
    assert_eq!(
        code_of(b),
        "F28R-FIELD",
        "24 reds with the total unchanged must not balance"
    );

    let mut b = body();
    b.bindings.platform[0].cells_pass = 192;
    b.bindings.platform[0].cells_red = 24;
    assert!(CertificationReceiptV2::unsigned(b).is_ok());
}
