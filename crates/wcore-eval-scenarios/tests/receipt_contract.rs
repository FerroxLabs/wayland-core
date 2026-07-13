use std::path::PathBuf;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use wcore_eval_scenarios::receipt::{
    AssertionEvidenceV1, AuthorityClaimV1, BoundaryEvidenceV1, BuildProvenanceV1,
    CanaryScanEvidenceV1, CellResultV1, DecisionEvidenceV1, Evidence, EvidenceReceiptV1,
    IdentityEvidenceV1, PolicyEvidenceV1, ProcessEvidenceV1, ProviderEvidenceV1, ReceiptBodyV1,
    ReceiptError, ReceiptMetadataV1, ReceiptVerifier, RecoveryEvidenceV1, SummaryEvidenceV1,
    TargetEvidenceV1, TimingEvidenceV1, VerificationPolicy, VerifiedAuthority,
};
use wcore_eval_scenarios::report::{ReportRenderError, render_receipt_reports};
use wcore_eval_scenarios::runner::{ExecutionEvidence, ScenarioResult};
use wcore_eval_scenarios::scenario::{ApprovalPolicy, Platform};
use wcore_eval_scenarios::{ProviderId, ToolTrace};

fn h64(ch: char) -> String {
    std::iter::repeat_n(ch, 64).collect()
}

fn body() -> ReceiptBodyV1 {
    let canary_scans = CanaryScanEvidenceV1 {
        scan_complete: true,
        protocol: 0,
        stdout: 0,
        stderr: 0,
        files: 0,
        logs: 0,
        telemetry: 0,
    };
    ReceiptBodyV1 {
        run_id: "run-001".to_string(),
        identity: IdentityEvidenceV1 {
            source_commit: "a".repeat(40),
            binary_sha256: h64('b'),
            config_sha256: h64('c'),
            fixture_sha256: h64('d'),
            provider: "openai".to_string(),
            model: "fixture-model-v1".to_string(),
            build: Evidence::observed(BuildProvenanceV1 {
                repository: "FerroxLabs/wayland-core".to_string(),
                source_ref: "refs/heads/frontier/m0".to_string(),
                workflow: "frontier-eval".to_string(),
                invocation_id: "ci-123".to_string(),
            }),
        },
        target: TargetEvidenceV1 {
            os: "linux".to_string(),
            architecture: "x86_64".to_string(),
            sandbox_backend: "cgroup-v2".to_string(),
        },
        policy: PolicyEvidenceV1 {
            posture: "approve_all".to_string(),
            effective_policy_sha256: h64('e'),
        },
        timings: TimingEvidenceV1 {
            boot_ms: Evidence::observed(100),
            ready_ms: Evidence::observed(110),
            prompt_ms: Evidence::observed(5),
            first_token_ms: Evidence::observed(20),
            tool_ms: Evidence::observed(30),
            approval_ms: Evidence::observed(2),
            completion_ms: Evidence::observed(150),
            shutdown_ms: Evidence::observed(10),
        },
        provider: ProviderEvidenceV1 {
            attempts: 1,
            typed_failures: Vec::new(),
            retries: 0,
            input_tokens: Evidence::observed(12),
            output_tokens: Evidence::observed(8),
            cache_read_tokens: Evidence::observed(0),
            cache_write_tokens: Evidence::observed(0),
            cost_microusd: 1_000,
            limit_microusd: 10_000,
        },
        tools: Vec::new(),
        decisions: vec![DecisionEvidenceV1 {
            actor: "evaluator".to_string(),
            action: "tool_approval".to_string(),
            resource_sha256: h64('2'),
            scope: "scenario".to_string(),
            decision: "approve_all".to_string(),
        }],
        boundaries: BoundaryEvidenceV1 {
            egress_attempted: Vec::new(),
            egress_allowed: Vec::new(),
            egress_denied: Vec::new(),
            filesystem_deltas: Vec::new(),
        },
        process: ProcessEvidenceV1 {
            tree_sha256: h64('f'),
            peak_memory_bytes: Evidence::observed(1024),
            peak_cpu_millis: Evidence::observed(10),
            cancellation_requested: false,
            orphan_count: Evidence::observed(0),
        },
        recovery: RecoveryEvidenceV1 {
            journal_cursor_sha256: Evidence::Unavailable {
                code: "not_applicable".to_string(),
            },
            action: "none".to_string(),
            unresolved_side_effects: Vec::new(),
        },
        canary_scans,
        assertions: vec![AssertionEvidenceV1 {
            assertion_id: "file-edited".to_string(),
            passed: true,
            failure_code: None,
        }],
        quarantines: Vec::new(),
        required_cells: vec!["deterministic-edit/openai/linux".to_string()],
        results: vec![CellResultV1 {
            cell_id: "deterministic-edit/openai/linux".to_string(),
            task: "deterministic-edit".to_string(),
            provider: "openai".to_string(),
            platform: "linux".to_string(),
            passed: true,
            failures: Vec::new(),
            usability: Vec::new(),
            wall_time_ms: 250,
            cost_microusd: 1_000,
        }],
        summary: SummaryEvidenceV1 {
            passed: 1,
            failed: 0,
            total_cost_microusd: 1_000,
            wall_time_ms: 250,
        },
    }
}

fn policy() -> VerificationPolicy {
    VerificationPolicy {
        source_commit: Some("a".repeat(40)),
        binary_sha256: Some(h64('b')),
        repository: Some("FerroxLabs/wayland-core".to_string()),
        source_ref: Some("refs/heads/frontier/m0".to_string()),
        workflow: Some("frontier-eval".to_string()),
    }
}

#[test]
fn local_receipt_is_valid_but_never_authoritative() {
    let receipt = EvidenceReceiptV1::local(body()).expect("valid local receipt");
    let verified = ReceiptVerifier::new()
        .verify(&receipt, &VerificationPolicy::default())
        .expect("local receipt integrity must verify");
    assert_eq!(verified.authority, VerifiedAuthority::LocalNonAuthoritative);
    assert!(verified.gate_passed);
}

#[test]
fn body_mutation_is_rejected_as_corruption() {
    let mut receipt = EvidenceReceiptV1::local(body()).expect("valid local receipt");
    receipt.body.identity.binary_sha256 = h64('9');
    let error = ReceiptVerifier::new()
        .verify(&receipt, &VerificationPolicy::default())
        .expect_err("mutated receipt must fail");
    assert_eq!(error, ReceiptError::DigestMismatch);
}

#[test]
fn ci_authority_requires_an_external_trust_anchor() {
    let signing_key = SigningKey::from_bytes(&[7; 32]);
    let receipt = EvidenceReceiptV1::local(body())
        .expect("valid receipt")
        .sign_ci("test-ci", &signing_key);

    let untrusted = ReceiptVerifier::new()
        .verify(&receipt, &policy())
        .expect_err("a self-carried claim must not establish authority");
    assert_eq!(untrusted, ReceiptError::UntrustedKey("test-ci".to_string()));

    let mut verifier = ReceiptVerifier::new();
    verifier.trust_ci_key("test-ci", signing_key.verifying_key());
    let verified = verifier
        .verify(&receipt, &policy())
        .expect("trusted detached signature must verify");
    assert_eq!(verified.authority, VerifiedAuthority::AuthoritativeCi);
}

#[test]
fn unsigned_or_mismatched_ci_provenance_is_rejected() {
    let signing_key = SigningKey::from_bytes(&[8; 32]);
    let mut verifier = ReceiptVerifier::new();
    verifier.trust_ci_key("ci", signing_key.verifying_key());

    let mut missing_build = body();
    missing_build.identity.build = Evidence::Unavailable {
        code: "missing_attestation".to_string(),
    };
    let missing_build = EvidenceReceiptV1::local(missing_build)
        .expect("structurally valid local receipt")
        .sign_ci("ci", &signing_key);
    assert_eq!(
        verifier
            .verify(&missing_build, &policy())
            .expect_err("CI claim without build evidence must fail"),
        ReceiptError::UnsignedAuthoritative
    );

    let receipt = EvidenceReceiptV1::local(body())
        .expect("valid receipt")
        .sign_ci("ci", &signing_key);
    let mut wrong_policy = policy();
    wrong_policy.binary_sha256 = Some(h64('0'));
    assert_eq!(
        verifier
            .verify(&receipt, &wrong_policy)
            .expect_err("binary provenance mismatch must fail"),
        ReceiptError::ProvenanceMismatch("binary digest".to_string())
    );
}

#[test]
fn incomplete_and_internally_inconsistent_receipts_are_rejected() {
    let mut value = serde_json::to_value(EvidenceReceiptV1::local(body()).unwrap()).unwrap();
    value["body"].as_object_mut().unwrap().remove("process");
    assert!(serde_json::from_value::<EvidenceReceiptV1>(value).is_err());

    let mut inconsistent = body();
    inconsistent.results[0]
        .failures
        .push(wcore_eval_scenarios::receipt::FailureEvidenceV1 {
            code: "runner_error".to_string(),
            detail_sha256: Evidence::observed(h64('1')),
        });
    assert!(matches!(
        EvidenceReceiptV1::local(inconsistent),
        Err(ReceiptError::InvalidEvidence(_))
    ));

    let mut empty_required_evidence = body();
    empty_required_evidence.process.tree_sha256.clear();
    assert!(matches!(
        EvidenceReceiptV1::local(empty_required_evidence),
        Err(ReceiptError::InvalidEvidence(_))
    ));

    let mut no_policy_decision = body();
    no_policy_decision.decisions.clear();
    assert!(matches!(
        EvidenceReceiptV1::local(no_policy_decision),
        Err(ReceiptError::InvalidEvidence(_))
    ));
}

#[test]
fn unsupported_schema_major_is_rejected() {
    let mut receipt = EvidenceReceiptV1::local(body()).expect("valid receipt");
    receipt.schema_version = 2;
    assert!(matches!(
        ReceiptVerifier::new().verify(&receipt, &VerificationPolicy::default()),
        Err(ReceiptError::UnsupportedSchema { version: 2, .. })
    ));
}

#[test]
fn claimed_ci_authority_cannot_be_unsigned() {
    let mut receipt = EvidenceReceiptV1::local(body()).expect("valid receipt");
    receipt.authority = AuthorityClaimV1::Ci {
        key_id: "ci".to_string(),
        signature_base64: String::new(),
    };
    let mut verifier = ReceiptVerifier::new();
    verifier.trust_ci_key("ci", SigningKey::from_bytes(&[9; 32]).verifying_key());
    assert_eq!(
        verifier
            .verify(&receipt, &policy())
            .expect_err("empty signature must fail"),
        ReceiptError::MalformedSignature
    );
}

#[test]
fn all_projections_share_the_receipt_and_exact_failure_identity() {
    let mut failed = body();
    failed.results[0].passed = false;
    failed.results[0]
        .failures
        .push(wcore_eval_scenarios::receipt::FailureEvidenceV1 {
            code: "secret_detected".to_string(),
            detail_sha256: Evidence::observed(h64('1')),
        });
    failed.summary.passed = 0;
    failed.summary.failed = 1;
    let receipt = EvidenceReceiptV1::local(failed).expect("valid failed receipt");
    let reports = render_receipt_reports(&receipt, &[]).expect("render projections");

    for output in [
        &reports.json,
        &reports.jsonl,
        &reports.junit,
        &reports.console,
        &reports.markdown,
    ] {
        assert!(output.contains(&receipt.body_sha256));
        assert!(output.contains("deterministic-edit"));
        assert!(output.contains("secret_detected"));
    }
    assert_eq!(reports.jsonl.lines().count(), 3);
    assert!(reports.junit.starts_with("<?xml version=\"1.0\""));
}

#[test]
fn projection_secret_scan_fails_before_any_report_is_returned() {
    let canary = "receipt-projection-canary".to_string();
    let mut contaminated = body();
    contaminated.run_id = canary.clone();
    let receipt = EvidenceReceiptV1::local(contaminated).expect("structurally valid receipt");
    assert!(matches!(
        render_receipt_reports(&receipt, &[canary]),
        Err(ReportRenderError::SecretDetected("JSON"))
    ));
}

#[test]
fn critical_usability_finding_is_a_receipt_gate_failure() {
    let result = ScenarioResult {
        name: "panic-regression".to_string(),
        provider: ProviderId::OpenAI,
        platform: Platform::Linux,
        approval: ApprovalPolicy::ApproveAll,
        passed: true,
        failures: Vec::new(),
        wall_time: Duration::from_millis(50),
        cost_usd: 0.0,
        trace: ToolTrace::default(),
        final_text: "apparently successful".to_string(),
        stderr_tail: "panic: background subsystem crashed".to_string(),
        turn_results: Vec::new(),
        workdir: PathBuf::from("/private/ephemeral"),
        boot_time: Duration::from_millis(10),
        info_events: Vec::new(),
        execution: ExecutionEvidence {
            config_sha256: h64('c'),
            sandbox_backend: "cgroup-v2".to_string(),
            process_tree_sha256: h64('f'),
            containment_authoritative: true,
            cleanup_verified: true,
            artifact_scan_complete: true,
            prompt_dispatch_time: Duration::from_millis(1),
            first_token_time: Some(Duration::from_millis(2)),
            approval_response_time: Duration::from_millis(1),
            shutdown_time: Duration::from_millis(5),
        },
    };
    let receipt = EvidenceReceiptV1::from_scenario_result(
        ReceiptMetadataV1 {
            run_id: "panic-run".to_string(),
            source_commit: "a".repeat(40),
            binary_sha256: h64('b'),
            fixture_sha256: h64('d'),
            model: "fixture-model-v1".to_string(),
            build: Evidence::Unavailable {
                code: "local_run".to_string(),
            },
        },
        &result,
        0.01,
    )
    .expect("receipt conversion");

    assert!(!receipt.body.results[0].passed);
    assert_eq!(receipt.body.results[0].usability[0].code, "panic");
    assert!(
        !ReceiptVerifier::new()
            .verify(&receipt, &VerificationPolicy::default())
            .expect("receipt integrity")
            .gate_passed
    );
    let reports = render_receipt_reports(&receipt, &[]).expect("safe projections");
    assert!(reports.console.contains("panic-regression"));
    assert!(!reports.json.contains("background subsystem crashed"));
    assert!(!reports.json.contains("/private/ephemeral"));
}

#[test]
fn parser_accepts_additive_v1_fields_and_rejects_ambiguous_json() {
    let receipt = EvidenceReceiptV1::local(body()).expect("valid receipt");
    let mut value = serde_json::to_value(&receipt).expect("receipt value");
    value["future_top_level"] = serde_json::json!({"ignored": true});
    value["body"]["future_body_field"] = serde_json::json!([1, 2, 3]);
    let additive = serde_json::to_vec(&value).expect("additive receipt");
    let (_, verified) = ReceiptVerifier::new()
        .parse_and_verify(&additive, &VerificationPolicy::default())
        .expect("v1 additive fields remain compatible");
    assert_eq!(verified.authority, VerifiedAuthority::LocalNonAuthoritative);

    let canonical = serde_json::to_string(&receipt).expect("receipt JSON");
    let duplicate = canonical.replacen("\"schema\":", "\"schema\":\"duplicate\",\"schema\":", 1);
    for invalid in [
        duplicate,
        format!("{canonical}{{}}"),
        canonical[..canonical.len() - 1].to_string(),
    ] {
        assert!(matches!(
            ReceiptVerifier::new()
                .parse_and_verify(invalid.as_bytes(), &VerificationPolicy::default()),
            Err(ReceiptError::InvalidJson(_))
        ));
    }
}

#[test]
fn golden_redacted_projection_digests_are_stable() {
    let mut failed = body();
    failed.results[0].passed = false;
    failed.results[0]
        .failures
        .push(wcore_eval_scenarios::receipt::FailureEvidenceV1 {
            code: "runner_error".to_string(),
            detail_sha256: Evidence::observed(h64('1')),
        });
    failed.summary.passed = 0;
    failed.summary.failed = 1;
    let receipt = EvidenceReceiptV1::local(failed).expect("golden receipt");
    let reports = render_receipt_reports(&receipt, &[]).expect("golden projections");
    let observed = [
        reports.json,
        reports.jsonl,
        reports.junit,
        reports.console,
        reports.markdown,
    ]
    .map(|report| format!("{:x}", Sha256::digest(report.as_bytes())));

    assert_eq!(
        observed,
        [
            "5da192ceb8e057f7b7ace0c68a9ba36cec36de8bf31901c07d68ea1d0bf059b3",
            "856bd2920c19edb63e97b6fc9b0fb762056a26e5353b72cec7630311591d96cf",
            "57e1b012086add98891e93b161adff9f5db3a41c2e0c4c0fb85ca56179f7be5b",
            "629988e9434e76f856c944f2fb0ec49f5c5377aab5ff7da30d9383dca64da5bc",
            "f150052cac35b70709a5c7e9d95cb686a07c6a8efcb9bf85ccf7bcabf64db167",
        ]
    );
}
