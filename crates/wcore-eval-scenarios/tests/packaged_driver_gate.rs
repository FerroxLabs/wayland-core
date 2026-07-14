#![cfg(feature = "packaged-driver-gate")]

use std::path::{Path, PathBuf};
use std::process::Output;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use tokio::process::Command;
use wcore_eval_scenarios::fixtures::manifest::{CompositeFixtureManifest, FixtureComponents};
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::receipt::{ReceiptVerifier, VerificationPolicy, VerifiedAuthority};
use wcore_eval_scenarios::receipt_policy::{
    AUTHORITY_POLICY_SCHEMA, AUTHORITY_POLICY_SCHEMA_VERSION, AuthoritativeReceiptPolicyV1,
    AuthorityError, CiProvenanceV1, sign_ci_receipt, verify_authoritative_receipt,
};
use wcore_eval_scenarios::runner::discover_binary;

fn expected_source_commit() -> String {
    let source = std::env::var("WAYLAND_BUILD_SOURCE_SHA")
        .expect("packaged-driver gate requires externally pinned WAYLAND_BUILD_SOURCE_SHA");
    assert!(
        source.len() == 40
            && source
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "WAYLAND_BUILD_SOURCE_SHA must be exactly 40 lowercase hexadecimal characters"
    );
    source
}

fn packaged_core() -> PathBuf {
    discover_binary().unwrap_or_else(|error| {
        panic!(
            "packaged-driver gate requires a packaged wayland-core binary; \
             build wcore-cli in this target directory first: {error}"
        )
    })
}

fn sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read packaged wayland-core bytes");
    format!("{:x}", Sha256::digest(bytes))
}

fn digest(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn fixture_manifest() -> CompositeFixtureManifest {
    CompositeFixtureManifest::new(
        FixtureComponents::new(
            digest(1),
            digest(2),
            digest(3),
            digest(4),
            digest(5),
            digest(6),
        )
        .expect("valid packaged fixture component identities"),
    )
}

async fn driver(core: &Path, source: &str, extra_args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-eval"));
    command
        .args(extra_args)
        .arg("--binary")
        .arg(core)
        .arg("--expected-source-commit")
        .arg(source)
        .env("OPENAI_API_KEY", "packaged-driver-fixture-key")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("WCORE_EVAL_BIN")
        .env_remove("WCORE_EVAL_PROVIDER");
    command.output().await.expect("execute wayland-eval driver")
}

fn context(output: &Output) -> String {
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[tokio::test]
async fn packaged_core_identity_and_driver_gates_are_enforced() {
    let source = expected_source_commit();
    let core = packaged_core();
    let digest = sha256(&core);

    let verified = driver(&core, &source, &["--verify-binary"]).await;
    assert!(verified.status.success(), "{}", context(&verified));
    let verified_stdout = String::from_utf8_lossy(&verified.stdout);
    assert!(
        verified_stdout.contains(&format!("sha256={digest}"))
            && verified_stdout.contains(&format!("source={source}")),
        "driver did not bind the expected source and exact packaged bytes: {}",
        context(&verified)
    );

    let passing_fixture = OpenAiFixtureScript::new([OpenAiStep::text("READY")])
        .start()
        .await
        .expect("start passing OpenAI fixture");
    let passing_base_url = passing_fixture.base_url().to_string();
    let evidence_root = tempfile::tempdir().expect("packaged authority evidence root");
    let manifest_path = evidence_root.path().join("fixture-manifest.json");
    let report_root = evidence_root.path().join("reports");
    let manifest = fixture_manifest();
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize fixture manifest"),
    )
    .expect("write fixture manifest");
    let passed = driver(
        &core,
        &source,
        &[
            "--scenario",
            "canary",
            "--provider",
            "openai",
            "--base-url",
            &passing_base_url,
            "--report-dir",
            report_root.to_str().expect("UTF-8 report root"),
            "--fixture-manifest",
            manifest_path.to_str().expect("UTF-8 manifest path"),
        ],
    )
    .await;
    let passing_observation = passing_fixture
        .shutdown()
        .await
        .expect("stop passing OpenAI fixture");
    assert!(passed.status.success(), "{}", context(&passed));
    let passed_stdout = String::from_utf8_lossy(&passed.stdout);
    assert!(
        passed_stdout.contains("PASS canary openai")
            && passed_stdout.contains("SUMMARY pass=1 fail=0 skip=0 aborted=0"),
        "{}",
        context(&passed)
    );
    assert!(
        passing_observation.complete(),
        "real packaged Core did not consume the passing fixture"
    );

    let receipt_path = std::fs::read_dir(&report_root)
        .expect("packaged report root")
        .next()
        .expect("one packaged report cell")
        .expect("packaged report entry")
        .path()
        .join("receipt.json");
    let local_json = std::fs::read(&receipt_path).expect("packaged wayland-eval receipt");
    let local: wcore_eval_scenarios::receipt::EvidenceReceiptV1 =
        serde_json::from_slice(&local_json).expect("parse packaged receipt");
    assert_eq!(
        local.body.identity.fixture_sha256,
        manifest.fixture_sha256(),
        "wayland-eval did not bind the supplied fixture manifest"
    );
    let signing_key = SigningKey::from_bytes(&[42; 32]);
    let provenance = CiProvenanceV1 {
        repository: "FerroxLabs/wayland-core".to_string(),
        source_ref: "refs/heads/frontier/m0".to_string(),
        workflow: "frontier-eval".to_string(),
        invocation_id: "packaged-driver-gate".to_string(),
    };
    let signed = sign_ci_receipt(
        &local_json,
        "release-ci",
        BASE64.encode(signing_key.to_bytes()).as_bytes(),
        provenance.clone(),
    )
    .expect("real wayland-eval receipt must enter the CI signer");
    let mut verifier = ReceiptVerifier::new();
    verifier.trust_ci_key("release-ci", signing_key.verifying_key());
    let verified = verifier
        .verify(
            &signed,
            &VerificationPolicy {
                source_commit: Some(signed.body.identity.source_commit.clone()),
                binary_sha256: Some(signed.body.identity.binary_sha256.clone()),
                repository: Some(provenance.repository.clone()),
                source_ref: Some(provenance.source_ref.clone()),
                workflow: Some(provenance.workflow.clone()),
            },
        )
        .expect("external trust must verify the packaged receipt signature");
    assert_eq!(verified.authority, VerifiedAuthority::AuthoritativeCi);

    let policy = AuthoritativeReceiptPolicyV1 {
        schema: AUTHORITY_POLICY_SCHEMA.to_string(),
        schema_version: AUTHORITY_POLICY_SCHEMA_VERSION,
        key_id: "release-ci".to_string(),
        public_key_base64: BASE64.encode(signing_key.verifying_key().as_bytes()),
        source_commit: signed.body.identity.source_commit.clone(),
        binary_sha256: signed.body.identity.binary_sha256.clone(),
        config_sha256: signed.body.identity.config_sha256.clone(),
        fixture_sha256: signed.body.identity.fixture_sha256.clone(),
        provider: signed.body.identity.provider.clone(),
        model: signed.body.identity.model.clone(),
        repository: provenance.repository,
        source_ref: provenance.source_ref,
        workflow: provenance.workflow,
        invocation_id: provenance.invocation_id,
        target_os: signed.body.target.os.clone(),
        target_architecture: signed.body.target.architecture.clone(),
        sandbox_backend: signed.body.target.sandbox_backend.clone(),
        policy_posture: signed.body.policy.posture.clone(),
        effective_policy_sha256: signed.body.policy.effective_policy_sha256.clone(),
        required_cells: signed.body.required_cells.clone(),
    };
    let signed_json = serde_json::to_vec(&signed).expect("signed packaged receipt JSON");
    assert!(matches!(
        verify_authoritative_receipt(&signed_json, &policy),
        Err(AuthorityError::MilestoneGateFailed(_))
    ));

    let failing_fixture = OpenAiFixtureScript::new([OpenAiStep::text("WRONG")])
        .start()
        .await
        .expect("start failing OpenAI fixture");
    let failing_base_url = failing_fixture.base_url().to_string();
    let failed = driver(
        &core,
        &source,
        &[
            "--scenario",
            "canary",
            "--provider",
            "openai",
            "--base-url",
            &failing_base_url,
        ],
    )
    .await;
    let failing_observation = failing_fixture
        .shutdown()
        .await
        .expect("stop failing OpenAI fixture");
    assert!(!failed.status.success(), "{}", context(&failed));
    let failed_stdout = String::from_utf8_lossy(&failed.stdout);
    assert!(
        failed_stdout.contains("FAIL canary openai")
            && failed_stdout.contains("SUMMARY pass=0 fail=1 skip=0 aborted=0"),
        "{}",
        context(&failed)
    );
    assert!(
        failing_observation.complete(),
        "real packaged Core did not consume the hard-gate fixture"
    );
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn packaged_candidate_cannot_replace_authenticated_egress_evidence() {
    let source = expected_source_commit();
    let core = packaged_core();
    let root = tempfile::tempdir().expect("egress replacement evidence root");
    let report_root = root.path().join("reports");
    let manifest_path = root.path().join("fixture-manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&fixture_manifest()).expect("serialize fixture manifest"),
    )
    .expect("write fixture manifest");
    let forged_digest = "0".repeat(64);
    let command = format!(
        "p=$(find .wayland-core -name 'eval-egress-*.jsonl' -print -quit); \
         test -n \"$p\"; rm -f -- \"$p\"; \
         printf '%s\\n' '{{\"record\":\"header\",\"version\":2}}' \
         '{{\"record\":\"footer\",\"complete\":true,\"event_count\":0,\"transcript_sha256\":\"{forged_digest}\",\"signature_base64\":\"AAAA\"}}' > \"$p\""
    );
    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "replace-egress-evidence",
            "Bash",
            serde_json::json!({"command": command}),
        ),
        OpenAiStep::text("READY"),
    ])
    .start()
    .await
    .expect("start replacement attack fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_wayland-eval"))
        .args([
            "--scenario",
            "canary",
            "--provider",
            "openai",
            "--base-url",
            fixture.base_url(),
            "--binary",
            core.to_str().expect("UTF-8 packaged Core path"),
            "--expected-source-commit",
            &source,
            "--report-dir",
            report_root.to_str().expect("UTF-8 report root"),
            "--fixture-manifest",
            manifest_path.to_str().expect("UTF-8 manifest path"),
        ])
        .env("OPENAI_API_KEY", "packaged-egress-attack-key")
        .env("WCORE_EVAL_REQUIRE_AUTHORITY_EVIDENCE", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .await
        .expect("run packaged egress replacement attack");
    let observation = fixture.shutdown().await.expect("stop attack fixture");
    assert!(!output.status.success(), "{}", context(&output));
    assert!(observation.complete(), "attack fixture was not consumed");

    let receipt_path = std::fs::read_dir(&report_root)
        .expect("attack report root")
        .next()
        .expect("one attack report cell")
        .expect("attack report entry")
        .path()
        .join("receipt.json");
    let receipt: wcore_eval_scenarios::receipt::EvidenceReceiptV1 =
        serde_json::from_slice(&std::fs::read(receipt_path).expect("attack receipt JSON"))
            .expect("parse attack receipt");
    assert!(
        receipt
            .body
            .tools
            .iter()
            .any(|tool| tool.tool_name == "Bash" && tool.exit_state == "success")
    );
    assert!(matches!(
        receipt.body.boundaries.egress_attempted,
        wcore_eval_scenarios::receipt::Evidence::Unavailable { ref code }
            if code == "managed_http_egress_recorder_incomplete"
    ));
    assert!(
        receipt.body.results[0]
            .failures
            .iter()
            .any(|failure| failure.code == "runner_error")
    );
}
