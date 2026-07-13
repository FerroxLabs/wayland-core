use ed25519_dalek::SigningKey;
use wcore_eval_scenarios::receipt::{
    AssertionEvidenceV1, AuthorityClaimV1, BoundaryEvidenceV1, BuildProvenanceV1,
    CanaryScanEvidenceV1, CellResultV1, Evidence, EvidenceReceiptV1, IdentityEvidenceV1,
    PolicyEvidenceV1, ProcessEvidenceV1, ProviderEvidenceV1, ReceiptBodyV1, ReceiptError,
    ReceiptVerifier, RecoveryEvidenceV1, SummaryEvidenceV1, TargetEvidenceV1, TimingEvidenceV1,
    VerificationPolicy, VerifiedAuthority,
};

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
            cost_usd: 0.001,
            limit_usd: 0.01,
        },
        tools: Vec::new(),
        decisions: Vec::new(),
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
            orphan_count: 0,
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
            cost_usd: 0.001,
        }],
        summary: SummaryEvidenceV1 {
            passed: 1,
            failed: 0,
            total_cost_usd: 0.001,
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
