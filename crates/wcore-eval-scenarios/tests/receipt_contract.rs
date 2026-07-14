use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use wcore_eval_scenarios::receipt::{
    AssertionEvidenceV1, AuthorityClaimV1, BoundaryEvidenceV1, BuildProvenanceV1,
    CanaryScanEvidenceV1, CellResultV1, DecisionEvidenceV1, Evidence, EvidenceReceiptV1,
    IdentityEvidenceV1, PolicyEvidenceV1, ProcessEvidenceV1, ProviderEvidenceV1, ReceiptBodyV1,
    ReceiptError, ReceiptMetadataV1, ReceiptVerifier, RecoveryEvidenceV1, SummaryEvidenceV1,
    TargetEvidenceV1, TimingEvidenceV1, ToolEvidenceV1, VerificationPolicy, VerifiedAuthority,
};
use wcore_eval_scenarios::receipt_policy::{
    AUTHORITY_POLICY_SCHEMA, AUTHORITY_POLICY_SCHEMA_VERSION, AuthoritativeReceiptPolicyV1,
    AuthorityError, CiProvenanceV1, sign_ci_receipt, verify_authoritative_receipt,
};
use wcore_eval_scenarios::report::{ReportRenderError, render_receipt_reports};
use wcore_eval_scenarios::runner::{ApprovalCommandEvidence, ExecutionEvidence, ScenarioResult};
use wcore_eval_scenarios::scenario::{ApprovalPolicy, Platform};
use wcore_eval_scenarios::trace::TraceEntry;
use wcore_eval_scenarios::{ProviderId, ToolTrace};

fn h64(ch: char) -> String {
    std::iter::repeat_n(ch, 64).collect()
}

fn traced_result(workdir: &str, external_path: &str) -> ScenarioResult {
    ScenarioResult {
        name: "workspace-normalization".to_string(),
        provider: ProviderId::OpenAI,
        platform: Platform::Linux,
        approval: ApprovalPolicy::ApproveAll,
        passed: true,
        failures: Vec::new(),
        wall_time: Duration::from_millis(50),
        cost_usd: 0.0,
        trace: ToolTrace {
            entries: vec![TraceEntry {
                call_id: "volatile-call-id".to_string(),
                tool_name: "Edit".to_string(),
                input: serde_json::json!({
                    "file_path": format!("{workdir}/src/settings.toml"),
                    "policy_path": external_path,
                })
                .to_string(),
                output: format!("Edited {workdir}/src/settings.toml"),
                is_error: false,
                duration: Some(Duration::from_millis(2)),
                turn: 0,
            }],
        },
        final_text: "done".to_string(),
        stderr_tail: String::new(),
        turn_results: Vec::new(),
        workdir: PathBuf::from(workdir),
        boot_time: Duration::from_millis(10),
        info_events: Vec::new(),
        execution: ExecutionEvidence {
            config_sha256: h64('c'),
            sandbox_backend: "fixture".to_string(),
            process_tree_sha256: h64('f'),
            containment_authoritative: true,
            cleanup_verified: true,
            artifact_scan_complete: true,
            prompt_dispatch_time: Duration::from_millis(1),
            first_token_time: Some(Duration::from_millis(2)),
            approval_response_time: Duration::ZERO,
            approval_commands: Vec::new(),
            provider_attempts: Some(1),
            provider_retries: Some(0),
            provider_typed_failures: Vec::new(),
            provider_usage: None,
            cancellation_requested: false,
            shutdown_time: Duration::from_millis(1),
        },
    }
}

fn try_receipt_from_trace(
    run_id: &str,
    result: &ScenarioResult,
) -> Result<EvidenceReceiptV1, ReceiptError> {
    EvidenceReceiptV1::from_scenario_result(
        ReceiptMetadataV1 {
            run_id: run_id.to_string(),
            source_commit: "a".repeat(40),
            binary_sha256: h64('b'),
            fixture_sha256: h64('d'),
            model: "fixture-model-v1".to_string(),
            build: Evidence::Unavailable {
                code: "local_run".to_string(),
            },
        },
        result,
        0.01,
    )
}

fn receipt_from_trace(run_id: &str, result: &ScenarioResult) -> EvidenceReceiptV1 {
    try_receipt_from_trace(run_id, result).expect("receipt conversion")
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
            attempts: Evidence::observed(1),
            typed_failures: Vec::new(),
            retries: Evidence::observed(0),
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
            egress_attempted: Evidence::observed(Vec::new()),
            egress_allowed: Evidence::observed(Vec::new()),
            egress_denied: Evidence::observed(Vec::new()),
            filesystem_deltas: Evidence::observed(Vec::new()),
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

fn authoritative_policy(signing_key: &SigningKey) -> AuthoritativeReceiptPolicyV1 {
    AuthoritativeReceiptPolicyV1 {
        schema: AUTHORITY_POLICY_SCHEMA.to_string(),
        schema_version: AUTHORITY_POLICY_SCHEMA_VERSION,
        key_id: "release-ci".to_string(),
        public_key_base64: BASE64.encode(signing_key.verifying_key().as_bytes()),
        source_commit: "a".repeat(40),
        binary_sha256: h64('b'),
        config_sha256: h64('c'),
        fixture_sha256: h64('d'),
        provider: "openai".to_string(),
        model: "fixture-model-v1".to_string(),
        repository: "FerroxLabs/wayland-core".to_string(),
        source_ref: "refs/heads/frontier/m0".to_string(),
        workflow: "frontier-eval".to_string(),
        invocation_id: "ci-456".to_string(),
        target_os: "linux".to_string(),
        target_architecture: "x86_64".to_string(),
        sandbox_backend: "cgroup-v2".to_string(),
        policy_posture: "approve_all".to_string(),
        effective_policy_sha256: h64('e'),
        required_cells: vec!["deterministic-edit/openai/linux".to_string()],
    }
}

fn ci_provenance() -> CiProvenanceV1 {
    CiProvenanceV1 {
        repository: "FerroxLabs/wayland-core".to_string(),
        source_ref: "refs/heads/frontier/m0".to_string(),
        workflow: "frontier-eval".to_string(),
        invocation_id: "ci-456".to_string(),
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
fn explicit_unavailable_measurements_cannot_satisfy_the_milestone_gate() {
    let mut incomplete = body();
    incomplete.boundaries.egress_attempted = Evidence::Unavailable {
        code: "recorder_not_enabled".to_string(),
    };
    let receipt = EvidenceReceiptV1::local(incomplete).expect("honest local receipt");
    let verified = ReceiptVerifier::new()
        .verify(&receipt, &VerificationPolicy::default())
        .expect("receipt remains structurally valid");
    assert!(!verified.gate_passed);
}

#[test]
fn behavior_digest_excludes_volatile_execution_identity() {
    let first = EvidenceReceiptV1::local(body()).expect("first receipt");
    let mut repeated = body();
    repeated.run_id = "run-002".to_string();
    repeated.identity.build = Evidence::observed(BuildProvenanceV1 {
        repository: "FerroxLabs/wayland-core".to_string(),
        source_ref: "refs/heads/frontier/m0".to_string(),
        workflow: "frontier-eval".to_string(),
        invocation_id: "ci-456".to_string(),
    });
    repeated.timings.boot_ms = Evidence::observed(211);
    repeated.timings.first_token_ms = Evidence::observed(77);
    repeated.timings.completion_ms = Evidence::observed(399);
    repeated.process.tree_sha256 = h64('9');
    repeated.process.peak_memory_bytes = Evidence::observed(2048);
    repeated.process.peak_cpu_millis = Evidence::observed(20);
    repeated.results[0].wall_time_ms = 499;
    repeated.summary.wall_time_ms = 499;
    let repeated = EvidenceReceiptV1::local(repeated).expect("repeated receipt");

    assert_ne!(first.body_sha256, repeated.body_sha256);
    assert_eq!(
        first.behavior_sha256().expect("first behavior digest"),
        repeated
            .behavior_sha256()
            .expect("repeated behavior digest")
    );
}

#[test]
fn workspace_token_cannot_collide_with_literal_or_sibling_paths() {
    let rooted = receipt_from_trace(
        "run-rooted",
        &traced_result("/private/run-a", "/etc/wayland/policy.toml"),
    );
    let mut literal_result = traced_result("/private/run-b", "/etc/wayland/policy.toml");
    literal_result.trace.entries[0].input = serde_json::json!({
        "file_path": "<WORKSPACE>/src/settings.toml",
        "policy_path": "/etc/wayland/policy.toml",
    })
    .to_string();
    literal_result.trace.entries[0].output = "Edited <WORKSPACE>/src/settings.toml".to_string();
    let literal = receipt_from_trace("run-literal", &literal_result);

    assert_ne!(
        rooted.behavior_sha256().expect("rooted behavior digest"),
        literal.behavior_sha256().expect("literal behavior digest")
    );

    let first_sibling = receipt_from_trace(
        "run-sibling-a",
        &traced_result("/private/run-a", "/private/run-a-outside/policy.toml"),
    );
    let second_sibling = receipt_from_trace(
        "run-sibling-b",
        &traced_result("/private/run-b", "/private/run-b-outside/policy.toml"),
    );
    assert_ne!(
        first_sibling
            .behavior_sha256()
            .expect("first sibling behavior digest"),
        second_sibling
            .behavior_sha256()
            .expect("second sibling behavior digest")
    );
}

#[test]
fn trace_evidence_rejects_an_unsafe_workspace_root() {
    for workdir in ["", ".", "/"] {
        let error = try_receipt_from_trace(
            "unsafe-workspace",
            &traced_result(workdir, "/etc/wayland/policy.toml"),
        )
        .expect_err("unsafe workspace must fail closed");
        assert!(matches!(error, ReceiptError::InvalidEvidence(_)));
    }
}

#[test]
fn behavior_digest_binds_fixture_tool_and_result_semantics() {
    let baseline = EvidenceReceiptV1::local(body()).expect("baseline receipt");
    let baseline_digest = baseline.behavior_sha256().expect("baseline digest");

    let mut changed_fixture = body();
    changed_fixture.identity.fixture_sha256 = h64('8');
    assert_ne!(
        baseline_digest,
        EvidenceReceiptV1::local(changed_fixture)
            .expect("changed fixture receipt")
            .behavior_sha256()
            .expect("changed fixture digest")
    );

    let mut changed_tool = body();
    changed_tool.tools.push(ToolEvidenceV1 {
        call_id_sha256: h64('1'),
        tool_name: "Write".to_string(),
        request_sha256: h64('2'),
        result_sha256: h64('3'),
        duration_ms: Evidence::observed(12),
        exit_state: "success".to_string(),
        idempotency_key_sha256: Evidence::Unavailable {
            code: "not_emitted".to_string(),
        },
    });
    assert_ne!(
        baseline_digest,
        EvidenceReceiptV1::local(changed_tool)
            .expect("changed tool receipt")
            .behavior_sha256()
            .expect("changed tool digest")
    );

    let mut changed_result = body();
    changed_result.results[0].task = "different-task".to_string();
    assert_ne!(
        baseline_digest,
        EvidenceReceiptV1::local(changed_result)
            .expect("changed result receipt")
            .behavior_sha256()
            .expect("changed result digest")
    );
}

#[test]
fn scenario_receipt_normalizes_only_the_owned_workspace() {
    let first = receipt_from_trace(
        "run-a",
        &traced_result("/private/run-a", "/etc/wayland/policy.toml"),
    );
    let repeated = receipt_from_trace(
        "run-b",
        &traced_result("/private/run-b", "/etc/wayland/policy.toml"),
    );
    let changed_external = receipt_from_trace(
        "run-c",
        &traced_result("/private/run-c", "/etc/wayland/other.toml"),
    );

    assert_ne!(first.body_sha256, repeated.body_sha256);
    assert_eq!(
        first.body.tools[0].request_sha256,
        repeated.body.tools[0].request_sha256
    );
    assert_eq!(
        first.body.tools[0].result_sha256,
        repeated.body.tools[0].result_sha256
    );
    assert_eq!(
        first.behavior_sha256().expect("first behavior digest"),
        repeated
            .behavior_sha256()
            .expect("repeated behavior digest")
    );
    assert_ne!(
        first.behavior_sha256().expect("first behavior digest"),
        changed_external
            .behavior_sha256()
            .expect("changed behavior digest")
    );
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
fn authoritative_workflow_requires_external_key_and_complete_exact_policy() {
    let signing_key = SigningKey::from_bytes(&[10; 32]);
    let local = EvidenceReceiptV1::local(body()).expect("valid local receipt");
    let local_json = serde_json::to_vec(&local).expect("local receipt JSON");
    let signed = sign_ci_receipt(
        &local_json,
        "release-ci",
        BASE64.encode(signing_key.to_bytes()).as_bytes(),
        ci_provenance(),
    )
    .expect("CI signer workflow");
    let signed_json = serde_json::to_vec(&signed).expect("signed receipt JSON");

    let (_, verified) =
        verify_authoritative_receipt(&signed_json, &authoritative_policy(&signing_key))
            .expect("complete trusted receipt");
    assert_eq!(verified.authority, VerifiedAuthority::AuthoritativeCi);
    assert!(verified.gate_passed);

    let mut wrong_fixture = authoritative_policy(&signing_key);
    wrong_fixture.fixture_sha256 = h64('9');
    assert!(matches!(
        verify_authoritative_receipt(&signed_json, &wrong_fixture),
        Err(AuthorityError::PolicyMismatch("fixture_sha256"))
    ));

    let mut wrong_manifest = authoritative_policy(&signing_key);
    wrong_manifest.required_cells = vec!["canary/openai/linux".to_string()];
    assert!(matches!(
        verify_authoritative_receipt(&signed_json, &wrong_manifest),
        Err(AuthorityError::PolicyMismatch("required_cells"))
    ));

    let wrong_key = SigningKey::from_bytes(&[11; 32]);
    assert!(verify_authoritative_receipt(&signed_json, &authoritative_policy(&wrong_key)).is_err());
}

#[test]
fn authoritative_workflow_rejects_local_incomplete_and_synthetic_receipts() {
    let signing_key = SigningKey::from_bytes(&[12; 32]);
    let local = EvidenceReceiptV1::local(body()).expect("valid local receipt");
    assert!(matches!(
        sign_ci_receipt(
            &serde_json::to_vec(&local).unwrap(),
            "release-ci",
            b"not-a-signing-key",
            ci_provenance(),
        ),
        Err(AuthorityError::InvalidSigningKey)
    ));
    assert!(matches!(
        verify_authoritative_receipt(
            &serde_json::to_vec(&local).unwrap(),
            &authoritative_policy(&signing_key)
        ),
        Err(AuthorityError::Receipt(ReceiptError::UntrustedKey(_)))
            | Err(AuthorityError::WrongAuthority)
    ));

    let mut incomplete = body();
    incomplete.boundaries.egress_attempted = Evidence::Unavailable {
        code: "recorder_not_enabled".to_string(),
    };
    let incomplete = EvidenceReceiptV1::local(incomplete).expect("honest incomplete receipt");
    let incomplete = sign_ci_receipt(
        &serde_json::to_vec(&incomplete).unwrap(),
        "release-ci",
        BASE64.encode(signing_key.to_bytes()).as_bytes(),
        ci_provenance(),
    )
    .expect("failed evidence may still be attested");
    assert!(matches!(
        verify_authoritative_receipt(
            &serde_json::to_vec(&incomplete).unwrap(),
            &authoritative_policy(&signing_key)
        ),
        Err(AuthorityError::MilestoneGateFailed)
    ));

    let mut synthetic = body();
    synthetic.identity.fixture_sha256 = format!(
        "{:x}",
        Sha256::digest(format!(
            "{}:{}",
            synthetic.identity.binary_sha256, synthetic.results[0].task
        ))
    );
    let synthetic = EvidenceReceiptV1::local(synthetic).expect("structurally valid receipt");
    assert!(matches!(
        sign_ci_receipt(
            &serde_json::to_vec(&synthetic).unwrap(),
            "release-ci",
            BASE64.encode(signing_key.to_bytes()).as_bytes(),
            ci_provenance(),
        ),
        Err(AuthorityError::SyntheticFixtureDigest)
    ));
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
        trace: ToolTrace {
            entries: vec![TraceEntry {
                call_id: "call-approved-write".to_string(),
                tool_name: "Write".to_string(),
                input: r#"{"content":"HELLO"}"#.to_string(),
                output: "Created approved.txt".to_string(),
                is_error: false,
                duration: Some(Duration::from_millis(2)),
                turn: 0,
            }],
        },
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
            approval_commands: vec![ApprovalCommandEvidence {
                call_id: "call-approved-write".to_string(),
                approved: true,
            }],
            provider_attempts: Some(3),
            provider_retries: Some(2),
            provider_typed_failures: vec!["http_503".to_string()],
            provider_usage: Some(wcore_eval_scenarios::runner::ProviderUsageEvidence {
                input_tokens: 12,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            }),
            cancellation_requested: false,
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

    assert_eq!(receipt.body.decisions.len(), 2);
    assert_eq!(receipt.body.decisions[0].action, "approval_posture");
    assert_eq!(receipt.body.decisions[0].decision, "approve_all");
    assert_eq!(receipt.body.decisions[1].action, "tool_approval_command");
    assert_eq!(receipt.body.decisions[1].decision, "approve_sent");
    assert_eq!(receipt.body.provider.attempts, Evidence::observed(3));
    assert_eq!(receipt.body.provider.retries, Evidence::observed(2));
    assert_eq!(receipt.body.provider.input_tokens, Evidence::observed(12));
    assert_eq!(receipt.body.provider.output_tokens, Evidence::observed(1));
    assert_eq!(receipt.body.provider.typed_failures, ["http_503"]);
    assert_eq!(
        receipt.body.decisions[1].resource_sha256,
        format!("{:x}", Sha256::digest(b"call-approved-write"))
    );
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
            "9144f78563aa1fa53bc4d5cbbe7ba53ec11e98d7a206d3d566839e7a7e81378d",
            "32ef4f6f462394d99889f26f03568183082e5482ff5ee49ef9234467b6443712",
            "70611de0f4fad801be159cf5f254765a820d78f867797f566bb7c211ff7988a1",
            "fe43f6240e6bf472b338985d8ae5d8d9e17eceeff80242e81f9d084945901ed8",
            "58e7b1decf89e27f3d2fb46b40f9da9b4c50ac7c83ab51124bf6c2a5fdad9f17",
        ]
    );
}
