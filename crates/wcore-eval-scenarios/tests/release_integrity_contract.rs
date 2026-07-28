//! Contract suite for the Phase 29 release manifest, trust root and the closed
//! four-state release ledger.
//!
//! **Every test here first builds a PRISTINE input and asserts it is ACCEPTED,
//! and only then applies the mutation and asserts the refusal.** A suite
//! containing only rejections passes trivially against a verifier that rejects
//! everything, which is the supply-chain form of a gate that cannot go red.

use std::collections::BTreeSet;
use std::process::Command;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

use wcore_eval_scenarios::receipt::{
    AssertionEvidenceV1, AuthorityClaimV1, BoundaryEvidenceV1, BuildProvenanceV1,
    CanaryScanEvidenceV1, CellResultV1, DecisionEvidenceV1, Evidence, EvidenceReceiptV1,
    IdentityEvidenceV1, PolicyEvidenceV1, ProcessEvidenceV1, ProviderEvidenceV1, ReceiptBodyV1,
    ReceiptError, ReceiptVerifier, RecoveryEvidenceV1, SummaryEvidenceV1, TargetEvidenceV1,
    TimingEvidenceV1, VerificationPolicy, VerifiedAuthority,
};
use wcore_eval_scenarios::release_integrity::{
    ArtifactKind, CANONICAL_RELEASE_STATES, CertificationBindingV1, DependencyPolicyOutcomeV1,
    PackagedArtifactV1, PolicyResult, ReleaseAuthorityClaimV1, ReleaseIntegrityError,
    ReleaseManifestBodyV1, ReleaseManifestV1, ReleaseState, ReleaseTrustRootV1,
    ReproducibilityVerdictV1, SbomFormat, SbomReferenceV1, TrustedKeyV1, VarianceClass,
    verify_manifest,
};
use wcore_eval_scenarios::release_states::{
    ReleaseStateBodyV1, ReleaseStateRecordV1, StateEvidenceV1, verify_state_chain,
};

const NOW: u64 = 1_000;

// ---------------------------------------------------------------------------
// Fixtures — a throwaway trust root generated in-process. No real key is used,
// read, copied or printed anywhere in this suite.
// ---------------------------------------------------------------------------

fn key_for(seed_byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed_byte; 32])
}

fn digest_of(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn hex40(ch: char) -> String {
    std::iter::repeat_n(ch, 40).collect()
}

/// One key per role, plus a SECOND packaging-role key. The second packaging key
/// exists so a role-mismatch can be proved without also tripping the
/// distinct-key-id rule — otherwise the two refusals would be
/// indistinguishable and the test would prove the wrong thing.
fn trust_root() -> ReleaseTrustRootV1 {
    let mut keys: Vec<TrustedKeyV1> = CANONICAL_RELEASE_STATES
        .iter()
        .enumerate()
        .map(|(index, state)| TrustedKeyV1 {
            key_id: format!("{}-key", state.as_str()),
            public_key_base64: BASE64.encode(key_for(index as u8 + 1).verifying_key().to_bytes()),
            role: *state,
            valid_from: 0,
            retired_at: None,
        })
        .collect();
    keys.push(TrustedKeyV1 {
        key_id: "packaging-key-spare".to_string(),
        public_key_base64: BASE64.encode(key_for(9).verifying_key().to_bytes()),
        role: ReleaseState::Packaging,
        valid_from: 0,
        retired_at: None,
    });
    ReleaseTrustRootV1::new(keys)
}

fn signing_key_for(state: ReleaseState) -> SigningKey {
    key_for(state.ordinal() + 1)
}

fn key_id_for(state: ReleaseState) -> String {
    format!("{}-key", state.as_str())
}

fn manifest_body(certification: Evidence<CertificationBindingV1>) -> ReleaseManifestBodyV1 {
    ReleaseManifestBodyV1 {
        release_id: "v0.12.25-wayland-base".to_string(),
        source_commit: hex40('a'),
        artifacts: vec![
            PackagedArtifactV1 {
                name: "wayland-core-v0.12.25-x86_64-unknown-linux-gnu.tar.gz".to_string(),
                sha256: digest_of(b"archive-bytes"),
                byte_length: 24_000_000,
                kind: ArtifactKind::Archive,
            },
            PackagedArtifactV1 {
                name: "wayland-core-checksums.txt".to_string(),
                sha256: digest_of(b"checksums-bytes"),
                byte_length: 512,
                kind: ArtifactKind::Checksums,
            },
        ],
        sbom: Evidence::observed(SbomReferenceV1 {
            name: "wayland-core.cdx.json".to_string(),
            sha256: digest_of(b"sbom-bytes"),
            format: SbomFormat::CycloneDxJson,
        }),
        dependency_policy: Evidence::observed(DependencyPolicyOutcomeV1 {
            tool: "cargo-deny".to_string(),
            policy_sha256: digest_of(b"deny.toml-bytes"),
            result: PolicyResult::Pass,
        }),
        reproducibility: ReproducibilityVerdictV1::Variance {
            class: VarianceClass::BuildId,
            evidence_sha256: digest_of(b"variance-report"),
        },
        certification,
        // The lifecycle facts a keyless build attestation structurally cannot
        // carry: where this manifest sits in the sequence, when it was issued,
        // and what has been revoked. Consumed by the shipped updater's freeze
        // and revocation checks.
        sequence: 1,
        issued_at: 1_800_000_000,
        revocations: Vec::new(),
    }
}

fn observed_certification() -> Evidence<CertificationBindingV1> {
    Evidence::observed(CertificationBindingV1 {
        receipt_body_sha256: digest_of(b"receipt-body"),
        receipt_schema: "wayland.eval.receipt".to_string(),
        receipt_schema_version: 1,
        receipt_signing_key_id: "ci-eval-key".to_string(),
        source_commit: hex40('a'),
        binary_sha256: digest_of(b"extracted-binary"),
        target_os: "linux".to_string(),
        target_architecture: "x86_64".to_string(),
    })
}

fn signed_manifest(certification: Evidence<CertificationBindingV1>) -> ReleaseManifestV1 {
    ReleaseManifestV1::unsigned(manifest_body(certification))
        .expect("pristine manifest body must be valid")
        .sign(
            key_id_for(ReleaseState::Packaging),
            &signing_key_for(ReleaseState::Packaging),
        )
}

fn evidence_for(state: ReleaseState) -> Vec<StateEvidenceV1> {
    vec![StateEvidenceV1 {
        name: format!("{}-evidence", state.as_str()),
        sha256: digest_of(state.as_str().as_bytes()),
    }]
}

/// Build a signed chain covering the first `count` canonical states.
fn signed_chain(manifest: &ReleaseManifestV1, count: usize) -> Vec<ReleaseStateRecordV1> {
    let mut chain: Vec<ReleaseStateRecordV1> = Vec::new();
    for state in CANONICAL_RELEASE_STATES.iter().take(count) {
        let previous = chain.last().map(|record| record.body_sha256.clone());
        let record = ReleaseStateRecordV1::unsigned(
            *state,
            &manifest.body_sha256,
            previous,
            evidence_for(*state),
        )
        .expect("pristine state body must be valid");
        chain.push(record.sign(key_id_for(*state), &signing_key_for(*state)));
    }
    chain
}

/// Re-digest and re-sign a record after its body was mutated, so that a test
/// isolates the property under test instead of failing on a stale digest.
fn reseal(
    mut record: ReleaseStateRecordV1,
    key_id: &str,
    key: &SigningKey,
) -> ReleaseStateRecordV1 {
    record.body_sha256 = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&record.body).expect("body serializes"))
    );
    record.sign(key_id, key)
}

// ===========================================================================
// Task 2 — manifest, trust root, domain separation
// ===========================================================================

#[test]
fn manifest_signature_does_not_verify_under_the_receipt_domain() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — the manifest verifies under its own domain.
    verify_manifest(&manifest, &root, ReleaseState::Packaging, NOW)
        .expect("control: a correctly signed manifest must be ACCEPTED");

    // MUTATION — feed that same manifest signature to the real ReceiptVerifier
    // as though it were a receipt signature over the same digest. The receipt
    // verifier must not accept it.
    //
    // The trusted key here is the MANIFEST's key, so the only reason this can
    // fail is the domain separator. A fully populated `receipt_policy()` is
    // supplied deliberately: `VerificationPolicy::default()` makes receipt.rs
    // refuse EVERY CI receipt with `UnsignedAuthoritative`, which would make
    // this assertion pass for a reason that has nothing to do with domains.
    let receipt = signed_receipt();
    let mut hijacked = receipt.clone();
    hijacked.authority = AuthorityClaimV1::Ci {
        key_id: "ci-eval-key".to_string(),
        signature_base64: manifest.authority.signature_base64.clone(),
    };
    let mut verifier = ReceiptVerifier::new();
    verifier.trust_ci_key(
        "ci-eval-key".to_string(),
        signing_key_for(ReleaseState::Packaging).verifying_key(),
    );
    let json = serde_json::to_vec(&hijacked).expect("serialize");
    assert_eq!(
        verifier
            .parse_and_verify(&json, &receipt_policy())
            .expect_err("a release-manifest signature must NOT verify as a receipt signature"),
        ReceiptError::InvalidSignature,
        "the refusal must be a signature refusal — i.e. caused by the domain separator"
    );
}

#[test]
fn receipt_signature_does_not_verify_as_a_manifest_signature() {
    let root = trust_root();
    let manifest = signed_manifest(observed_certification());

    // PRISTINE CONTROL — accepted.
    verify_manifest(&manifest, &root, ReleaseState::Packaging, NOW)
        .expect("control: a correctly signed manifest must be ACCEPTED");

    // MUTATION — sign the SAME body digest the way receipt.rs signs, i.e.
    // under the receipt's domain separator, using the SAME key. Only the
    // domain differs, so this isolates domain separation as the sole cause.
    let key = signing_key_for(ReleaseState::Packaging);
    let mut receipt_domain_message = b"wayland.eval.receipt.v1\0".to_vec();
    receipt_domain_message.extend_from_slice(manifest.body_sha256.as_bytes());
    let cross_domain = key.sign(&receipt_domain_message);

    let mut forged = manifest.clone();
    forged.authority = ReleaseAuthorityClaimV1 {
        key_id: key_id_for(ReleaseState::Packaging),
        signature_base64: BASE64.encode(cross_domain.to_bytes()),
    };
    assert_eq!(
        verify_manifest(&forged, &root, ReleaseState::Packaging, NOW),
        Err(ReleaseIntegrityError::InvalidSignature),
        "a signature minted under the receipt domain must NOT verify as a manifest signature"
    );
}

#[test]
fn an_unknown_key_id_is_refused_rather_than_trusted() {
    let root = trust_root();
    let manifest = signed_manifest(observed_certification());

    // PRISTINE CONTROL — accepted.
    verify_manifest(&manifest, &root, ReleaseState::Packaging, NOW)
        .expect("control: a correctly signed manifest must be ACCEPTED");

    // MUTATION — a key id the trust root has never heard of. The signature
    // itself is untouched and cryptographically fine; authority is refused
    // because it is not read from the document.
    let mut unknown = manifest.clone();
    unknown.authority.key_id = "attacker-supplied-key".to_string();
    assert_eq!(
        verify_manifest(&unknown, &root, ReleaseState::Packaging, NOW),
        Err(ReleaseIntegrityError::UnknownKeyId(
            "attacker-supplied-key".to_string()
        ))
    );
}

#[test]
fn a_retired_key_is_refused_although_its_signature_is_valid() {
    let manifest = signed_manifest(observed_certification());

    // PRISTINE CONTROL — with a live key, accepted.
    let live = trust_root();
    verify_manifest(&manifest, &live, ReleaseState::Packaging, NOW)
        .expect("control: a live key must be ACCEPTED");

    // MUTATION — retire that same key. The signature is byte-identical and
    // still cryptographically valid; retirement alone must refuse it.
    let mut retired = trust_root();
    for key in &mut retired.keys {
        if key.key_id == key_id_for(ReleaseState::Packaging) {
            key.retired_at = Some(NOW - 1);
        }
    }
    assert_eq!(
        verify_manifest(&manifest, &retired, ReleaseState::Packaging, NOW),
        Err(ReleaseIntegrityError::RetiredKey(key_id_for(
            ReleaseState::Packaging
        )))
    );

    // And the boundary is inclusive at the retirement instant, not after it.
    assert_eq!(
        verify_manifest(&manifest, &retired, ReleaseState::Packaging, NOW - 1),
        Err(ReleaseIntegrityError::RetiredKey(key_id_for(
            ReleaseState::Packaging
        )))
    );
    // Strictly before retirement it still verifies — proving the refusal above
    // is caused by retirement and not by something incidental.
    verify_manifest(&manifest, &retired, ReleaseState::Packaging, NOW - 2)
        .expect("before retirement the same key must still be ACCEPTED");
}

#[test]
fn an_unknown_field_in_a_manifest_is_refused_at_deserialization() {
    let manifest = signed_manifest(observed_certification());
    let json = serde_json::to_string(&manifest).expect("serialize");

    // PRISTINE CONTROL — the unmodified document parses.
    serde_json::from_str::<ReleaseManifestV1>(&json)
        .expect("control: an unmodified manifest must PARSE");

    // MUTATION — one added top-level field.
    let injected = json.replacen('{', r#"{"attacker_added_field":"payload","#, 1);
    assert!(
        serde_json::from_str::<ReleaseManifestV1>(&injected).is_err(),
        "an unknown top-level field must be refused, not ignored"
    );

    // MUTATION — one added field nested inside the body, to prove the refusal
    // is not only at the outermost struct.
    let nested = json.replacen(r#""release_id""#, r#""smuggled":1,"release_id""#, 1);
    assert!(
        serde_json::from_str::<ReleaseManifestV1>(&nested).is_err(),
        "an unknown nested field must be refused, not ignored"
    );

    // MUTATION — an unknown field in the trust root and in a trusted key.
    let root_json = serde_json::to_string(&trust_root()).expect("serialize");
    serde_json::from_str::<ReleaseTrustRootV1>(&root_json)
        .expect("control: an unmodified trust root must PARSE");
    let root_injected = root_json.replacen(r#""keys""#, r#""bypass":true,"keys""#, 1);
    assert!(serde_json::from_str::<ReleaseTrustRootV1>(&root_injected).is_err());
}

#[test]
fn an_unavailable_certification_binding_is_representable_and_verifies() {
    let root = trust_root();

    // PRISTINE CONTROL — an OBSERVED binding verifies.
    let observed = signed_manifest(observed_certification());
    verify_manifest(&observed, &root, ReleaseState::Packaging, NOW)
        .expect("control: an observed certification binding must be ACCEPTED");

    // The Phase 28 seam: absence is explicit and still verifies, so Phase 29 is
    // fully buildable before Phase 28 lands.
    let unavailable = signed_manifest(Evidence::Unavailable {
        code: "phase_28_certification_binding_not_yet_available".to_string(),
    });
    verify_manifest(&unavailable, &root, ReleaseState::Packaging, NOW)
        .expect("an unavailable certification binding must remain representable and verify");

    // It survives a JSON round trip as an explicit absence, never as an empty
    // success — an absent field would be a different and much worse thing.
    let json = serde_json::to_string(&unavailable).expect("serialize");
    assert!(
        json.contains("phase_28_certification_binding_not_yet_available"),
        "absence must be explicit in the document"
    );
    let reparsed: ReleaseManifestV1 = serde_json::from_str(&json).expect("round trip");
    assert!(matches!(
        reparsed.body.certification,
        Evidence::Unavailable { .. }
    ));

    // ...but it may NOT be carried all the way to release acceptance. That is
    // what makes it impossible to ship a release whose certification never
    // happened. Proved in full by
    // `an_unavailable_certification_binding_cannot_reach_release_acceptance`.
}

#[test]
fn trust_root_init_never_prints_the_signing_seed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root_dir = dir.path().join("trust");

    let output = Command::new(env!("CARGO_BIN_EXE_wayland-release"))
        .args([
            "trust-root-init",
            "--directory",
            root_dir.to_str().expect("utf8"),
        ])
        .output()
        .expect("run wayland-release trust-root-init");
    assert!(
        output.status.success(),
        "control: trust-root init must SUCCEED, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // PRISTINE CONTROL — it really did emit the public material, so a trivially
    // empty stdout cannot be mistaken for "no seed was printed".
    assert!(stdout.contains("TRUST ROOT READY"), "stdout was: {stdout}");
    let key_lines = stdout
        .lines()
        .filter(|line| line.starts_with("KEY "))
        .count();
    assert_eq!(key_lines, 4, "expected one KEY line per state: {stdout}");

    // THE ASSERTION — no seed file's contents appear on either stream.
    let mut seeds_checked = 0;
    for entry in std::fs::read_dir(&root_dir).expect("read trust dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("seed") {
            continue;
        }
        seeds_checked += 1;
        let seed = std::fs::read_to_string(&path).expect("read seed");
        let seed = seed.trim();
        assert!(!seed.is_empty(), "seed file must not be empty");
        assert!(
            !stdout.contains(seed),
            "a signing seed leaked to stdout from {}",
            path.display()
        );
        assert!(
            !stderr.contains(seed),
            "a signing seed leaked to stderr from {}",
            path.display()
        );

        // The seed file itself must be owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "seed file {} must be mode 0600, got {:o}",
                path.display(),
                mode & 0o777
            );
        }
    }
    assert_eq!(seeds_checked, 4, "expected one seed file per release state");

    // And the CLI must expose no way to pass a seed as an argument.
    let help = Command::new(env!("CARGO_BIN_EXE_wayland-release"))
        .args(["manifest-sign", "--help"])
        .output()
        .expect("run --help");
    let help_text = String::from_utf8_lossy(&help.stdout).to_lowercase();
    assert!(
        !help_text.contains("--seed") && !help_text.contains("--private"),
        "no subcommand may accept a seed on the command line: {help_text}"
    );
}

// ===========================================================================
// Task 3 — the closed four-state ledger
// ===========================================================================

#[test]
fn an_invented_state_name_fails_to_deserialize() {
    // PRISTINE CONTROL — every canonical state name parses.
    for state in CANONICAL_RELEASE_STATES {
        let json = format!(r#""{}""#, state.as_str());
        assert_eq!(
            serde_json::from_str::<ReleaseState>(&json).expect("control: canonical name parses"),
            state
        );
    }

    // MUTATION — the exact string an agent on this program actually invented to
    // route around an artifact it wrongly believed unobtainable. It must fail
    // at DESERIALIZATION, before any verification logic runs. The reason lives
    // in this test rather than in a comment in the module.
    assert!(
        serde_json::from_str::<ReleaseState>(r#""termination_state_4""#).is_err(),
        "the invented state termination_state_4 must not deserialize"
    );

    // And it must not be smuggleable inside a whole record either.
    let manifest = signed_manifest(observed_certification());
    let record = &signed_chain(&manifest, 1)[0];
    let json = serde_json::to_string(record).expect("serialize");
    serde_json::from_str::<ReleaseStateRecordV1>(&json)
        .expect("control: a real record round-trips");
    let invented = json.replace(r#""state":"packaging""#, r#""state":"termination_state_4""#);
    assert!(
        serde_json::from_str::<ReleaseStateRecordV1>(&invented).is_err(),
        "a record naming termination_state_4 must not deserialize"
    );
    // The body struct refuses it too, not merely the outer record.
    assert!(
        serde_json::from_str::<ReleaseStateBodyV1>(
            r#"{"state":"termination_state_4","ordinal":4,"manifest_sha256":"aa","previous_record_sha256":null,"evidence":[]}"#
        )
        .is_err()
    );
}

#[test]
fn a_fifth_state_record_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — the full four-record chain is ACCEPTED and reaches
    // release acceptance.
    let chain = signed_chain(&manifest, 4);
    let progress = verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: the canonical four-record chain must be ACCEPTED");
    assert_eq!(
        progress.highest_state,
        Some(ReleaseState::ReleaseAcceptance)
    );
    assert!(progress.is_accepted());

    // MUTATION — append a fifth record. There are four states and no others,
    // so a fifth record cannot be anything but a repeat.
    let mut five = chain.clone();
    let extra = ReleaseStateRecordV1::unsigned(
        ReleaseState::ReleaseAcceptance,
        &manifest.body_sha256,
        Some(chain[3].body_sha256.clone()),
        vec![StateEvidenceV1 {
            name: "extra".to_string(),
            sha256: digest_of(b"extra-evidence"),
        }],
    )
    .expect("body valid");
    five.push(extra.sign("packaging-key-spare", &key_for(9)));
    assert_eq!(
        verify_state_chain(&five, &manifest, &root, NOW),
        Err(ReleaseIntegrityError::TooManyRecords { count: 5 })
    );
}

#[test]
fn release_acceptance_signed_by_the_packaging_key_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — accepted when each record is signed by its own role's key.
    let chain = signed_chain(&manifest, 4);
    verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: the canonical chain must be ACCEPTED");

    // MUTATION — sign the acceptance record with a key whose trust-root ROLE is
    // packaging. A DISTINCT packaging key is used deliberately: reusing
    // `packaging-key` would trip the distinct-key-id rule instead, and the test
    // would prove the wrong property.
    let mut forged = chain.clone();
    let acceptance = forged[3].clone();
    forged[3] = acceptance.sign("packaging-key-spare", &key_for(9));
    assert_eq!(
        verify_state_chain(&forged, &manifest, &root, NOW),
        Err(ReleaseIntegrityError::RoleMismatch {
            key_id: "packaging-key-spare".to_string(),
            required: ReleaseState::ReleaseAcceptance,
            bound: ReleaseState::Packaging,
        }),
        "a key bound to packaging must not be able to sign release acceptance"
    );
}

#[test]
fn a_packaging_signature_replayed_into_the_acceptance_slot_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — accepted.
    let chain = signed_chain(&manifest, 4);
    verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: the canonical chain must be ACCEPTED");

    // MUTATION — take the ACCEPTANCE record's own body digest and sign it with
    // the ACCEPTANCE key, but under the PACKAGING domain. Everything is held
    // constant except the domain separator, so the refusal isolates domain
    // separation as the sole cause.
    let mut replayed = chain.clone();
    let acceptance_key = signing_key_for(ReleaseState::ReleaseAcceptance);
    let mut packaging_domain_message = ReleaseState::Packaging.signature_domain().to_vec();
    packaging_domain_message.extend_from_slice(replayed[3].body_sha256.as_bytes());
    let wrong_domain = acceptance_key.sign(&packaging_domain_message);
    replayed[3].authority = ReleaseAuthorityClaimV1 {
        key_id: key_id_for(ReleaseState::ReleaseAcceptance),
        signature_base64: BASE64.encode(wrong_domain.to_bytes()),
    };
    assert_eq!(
        verify_state_chain(&replayed, &manifest, &root, NOW),
        Err(ReleaseIntegrityError::InvalidSignature),
        "a signature over the packaging domain must not verify in the acceptance slot"
    );

    // The same bytes DO verify in their own slot, proving the signature itself
    // is sound and only the domain rejected it.
    let mut packaging_only = signed_chain(&manifest, 1);
    let packaging_key = signing_key_for(ReleaseState::Packaging);
    let mut own_domain_message = ReleaseState::Packaging.signature_domain().to_vec();
    own_domain_message.extend_from_slice(packaging_only[0].body_sha256.as_bytes());
    packaging_only[0].authority = ReleaseAuthorityClaimV1 {
        key_id: key_id_for(ReleaseState::Packaging),
        signature_base64: BASE64.encode(packaging_key.sign(&own_domain_message).to_bytes()),
    };
    verify_state_chain(&packaging_only, &manifest, &root, NOW)
        .expect("the same construction in its own domain must be ACCEPTED");
}

#[test]
fn evidence_reused_from_an_earlier_state_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — accepted with disjoint evidence.
    let chain = signed_chain(&manifest, 4);
    verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: disjoint evidence must be ACCEPTED");

    // MUTATION — relabel PACKAGING's evidence digest as ACCEPTANCE's evidence.
    // This is precisely the move that turns four states into one state wearing
    // four labels, so it must be refused by digest and not by name.
    let mut relabelled = chain.clone();
    relabelled[3].body.evidence = vec![StateEvidenceV1 {
        name: "release-acceptance-evidence".to_string(),
        sha256: digest_of(ReleaseState::Packaging.as_str().as_bytes()),
    }];
    relabelled[3] = reseal(
        relabelled[3].clone(),
        &key_id_for(ReleaseState::ReleaseAcceptance),
        &signing_key_for(ReleaseState::ReleaseAcceptance),
    );
    assert_eq!(
        verify_state_chain(&relabelled, &manifest, &root, NOW),
        Err(ReleaseIntegrityError::EvidenceReuse {
            state: ReleaseState::ReleaseAcceptance,
            sha256: digest_of(ReleaseState::Packaging.as_str().as_bytes()),
        })
    );
}

#[test]
fn a_reordered_chain_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — accepted in canonical order.
    let chain = signed_chain(&manifest, 4);
    verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: canonical order must be ACCEPTED");

    // MUTATION — swap rollback rehearsal ahead of deployment preparation.
    let mut swapped = chain.clone();
    swapped.swap(1, 2);
    assert_eq!(
        verify_state_chain(&swapped, &manifest, &root, NOW),
        Err(ReleaseIntegrityError::NonCanonicalOrder {
            position: 1,
            expected: ReleaseState::DeploymentPreparation,
            found: ReleaseState::RollbackRehearsal,
        })
    );
}

#[test]
fn a_broken_previous_record_digest_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — accepted with an intact back-link.
    let chain = signed_chain(&manifest, 3);
    let progress = verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: an intact chain must be ACCEPTED");
    assert_eq!(
        progress.highest_state,
        Some(ReleaseState::RollbackRehearsal)
    );

    // MUTATION — point record 2's back-link at a digest that is not record 1's,
    // which is how a record would be lifted out of one chain into another.
    let mut lifted = chain.clone();
    lifted[2].body.previous_record_sha256 = Some(digest_of(b"some-other-chains-record"));
    lifted[2] = reseal(
        lifted[2].clone(),
        &key_id_for(ReleaseState::RollbackRehearsal),
        &signing_key_for(ReleaseState::RollbackRehearsal),
    );
    assert_eq!(
        verify_state_chain(&lifted, &manifest, &root, NOW),
        Err(ReleaseIntegrityError::PreviousDigestMismatch { position: 2 })
    );
}

#[test]
fn all_four_records_signed_by_one_key_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — four distinct key ids are ACCEPTED.
    let chain = signed_chain(&manifest, 4);
    verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: four distinct keys must be ACCEPTED");
    let distinct: BTreeSet<&str> = chain
        .iter()
        .map(|record| record.authority.key_id.as_str())
        .collect();
    assert_eq!(
        distinct.len(),
        4,
        "the control must really use four key ids"
    );

    // MUTATION — one key signs all four states. Each signature is individually
    // valid under its own state domain; the chain is refused because four
    // states signed by one key is one authority wearing four hats.
    let one_key = signing_key_for(ReleaseState::Packaging);
    let one_key_id = key_id_for(ReleaseState::Packaging);
    let collapsed: Vec<ReleaseStateRecordV1> = chain
        .iter()
        .map(|record| record.clone().sign(&one_key_id, &one_key))
        .collect();
    assert_eq!(
        verify_state_chain(&collapsed, &manifest, &root, NOW),
        Err(ReleaseIntegrityError::DuplicateKeyId {
            key_id: one_key_id.clone()
        })
    );
}

#[test]
fn withholding_the_acceptance_key_caps_progress_at_rollback_rehearsal() {
    let manifest = signed_manifest(observed_certification());

    // PRISTINE CONTROL — when the acceptance key IS available, the chain
    // reaches release acceptance. Without this control, "capped at rollback
    // rehearsal" would be indistinguishable from a verifier that can never
    // reach acceptance at all.
    let full_root = trust_root();
    let full_chain = signed_chain(&manifest, 4);
    let accepted = verify_state_chain(&full_chain, &manifest, &full_root, NOW)
        .expect("control: with the acceptance key, the chain must reach acceptance");
    assert_eq!(
        accepted.highest_state,
        Some(ReleaseState::ReleaseAcceptance)
    );
    assert!(accepted.is_accepted());

    // WITHHOLD the acceptance key. No record for that state can be minted, so
    // the chain simply stops at three records. That is NOT an error: it is the
    // honest report that the release has not been accepted.
    let capped_chain = signed_chain(&manifest, 3);
    let capped = verify_state_chain(&capped_chain, &manifest, &full_root, NOW)
        .expect("a short chain is not an error — it is unfinished progress");
    assert_eq!(
        capped.highest_state,
        Some(ReleaseState::RollbackRehearsal),
        "progress must cap at rollback rehearsal, not fail ambiguously"
    );
    assert_eq!(capped.records_verified, 3);
    assert!(
        !capped.is_accepted(),
        "a capped chain must never read as accepted"
    );

    // And the cap cannot be lifted by removing the acceptance key from the
    // trust root and signing acceptance with some other key.
    let mut rootless = trust_root();
    rootless
        .keys
        .retain(|key| key.role != ReleaseState::ReleaseAcceptance);
    let forged = signed_chain(&manifest, 4);
    assert!(
        verify_state_chain(&forged, &manifest, &rootless, NOW).is_err(),
        "with no acceptance key in the trust root, acceptance must be unreachable"
    );
}

#[test]
fn an_unavailable_certification_binding_cannot_reach_release_acceptance() {
    let root = trust_root();

    // PRISTINE CONTROL — with an OBSERVED binding, acceptance is reachable.
    let certified = signed_manifest(observed_certification());
    let certified_chain = signed_chain(&certified, 4);
    let progress = verify_state_chain(&certified_chain, &certified, &root, NOW)
        .expect("control: a certified release must reach acceptance");
    assert!(progress.is_accepted());

    // The same manifest with an UNAVAILABLE binding verifies happily through
    // rollback rehearsal...
    let uncertified = signed_manifest(Evidence::Unavailable {
        code: "phase_28_certification_binding_not_yet_available".to_string(),
    });
    let three = signed_chain(&uncertified, 3);
    let capped = verify_state_chain(&three, &uncertified, &root, NOW)
        .expect("an uncertified release may still be packaged and rehearsed");
    assert_eq!(capped.highest_state, Some(ReleaseState::RollbackRehearsal));

    // ...and is refused at release acceptance. This is what makes it impossible
    // to ship a release whose certification never happened.
    let four = signed_chain(&uncertified, 4);
    assert_eq!(
        verify_state_chain(&four, &uncertified, &root, NOW),
        Err(ReleaseIntegrityError::UnavailableCertificationAtAcceptance)
    );
}

#[test]
fn a_record_bound_to_a_different_manifest_is_rejected() {
    let manifest = signed_manifest(observed_certification());
    let root = trust_root();

    // PRISTINE CONTROL — accepted against its own manifest.
    let chain = signed_chain(&manifest, 2);
    verify_state_chain(&chain, &manifest, &root, NOW)
        .expect("control: records must be ACCEPTED against their own manifest");

    // MUTATION — verify the same chain against a DIFFERENT release.
    let other = ReleaseManifestV1::unsigned({
        let mut body = manifest_body(observed_certification());
        body.release_id = "v0.12.26-wayland-base".to_string();
        body
    })
    .expect("body valid")
    .sign(
        key_id_for(ReleaseState::Packaging),
        &signing_key_for(ReleaseState::Packaging),
    );
    assert_ne!(manifest.body_sha256, other.body_sha256);
    assert_eq!(
        verify_state_chain(&chain, &other, &root, NOW),
        Err(ReleaseIntegrityError::ManifestMismatch { position: 0 })
    );
}

#[test]
fn a_tampered_manifest_body_is_refused_by_digest() {
    let root = trust_root();
    let manifest = signed_manifest(observed_certification());

    // PRISTINE CONTROL — accepted.
    verify_manifest(&manifest, &root, ReleaseState::Packaging, NOW)
        .expect("control: an untampered manifest must be ACCEPTED");

    // MUTATION — swap one artifact digest, leaving the stated body digest and
    // the signature alone. This is the one-byte-mutation case the live CLI run
    // also exercises end to end.
    let mut tampered = manifest.clone();
    tampered.body.artifacts[0].sha256 = digest_of(b"substituted-archive-bytes");
    assert_eq!(
        verify_manifest(&tampered, &root, ReleaseState::Packaging, NOW),
        Err(ReleaseIntegrityError::DigestMismatch)
    );
}

// ---------------------------------------------------------------------------
// A real Phase 28 receipt, used so the cross-domain proof runs against the REAL
// ReceiptVerifier rather than against a re-declared copy of its domain string.
// ---------------------------------------------------------------------------

fn h64(ch: char) -> String {
    std::iter::repeat_n(ch, 64).collect()
}

fn signed_receipt() -> EvidenceReceiptV1 {
    let body = ReceiptBodyV1 {
        run_id: "run-29-01".to_string(),
        identity: IdentityEvidenceV1 {
            source_commit: hex40('a'),
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
            egress_scope: "core_managed_http_v1".to_string(),
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
        canary_scans: CanaryScanEvidenceV1 {
            scan_complete: true,
            protocol: 0,
            stdout: 0,
            stderr: 0,
            files: 0,
            logs: 0,
            telemetry: 0,
        },
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
    };
    EvidenceReceiptV1::local(body)
        .expect("receipt body must be valid")
        .sign_ci("ci-eval-key", &key_for(7))
}

/// The trust inputs `receipt.rs` requires before it will grant CI authority.
///
/// All five fields must be populated: `validate_ci_provenance` refuses a CI
/// receipt with `UnsignedAuthoritative` when ANY of them is `None`, so an empty
/// policy can never confer authority. That is correct fail-closed behaviour in
/// the product — and it is exactly why the anti-vacuity control below exists.
fn receipt_policy() -> VerificationPolicy {
    VerificationPolicy {
        source_commit: Some(hex40('a')),
        binary_sha256: Some(h64('b')),
        repository: Some("FerroxLabs/wayland-core".to_string()),
        source_ref: Some("refs/heads/frontier/m0".to_string()),
        workflow: Some("frontier-eval".to_string()),
    }
}

#[test]
fn the_real_receipt_fixture_verifies_so_the_cross_domain_proof_is_not_vacuous() {
    // ANTI-VACUITY CONTROL. If this receipt did not verify under its OWN
    // domain, `manifest_signature_does_not_verify_under_the_receipt_domain`
    // would pass for the wrong reason — the receipt verifier would simply be
    // rejecting everything handed to it.
    //
    // This control has already earned its place: the first version of this
    // suite passed `VerificationPolicy::default()`, receipt.rs refused every CI
    // receipt with `UnsignedAuthoritative`, and this test is what caught it.
    let receipt = signed_receipt();
    let mut verifier = ReceiptVerifier::new();
    verifier.trust_ci_key("ci-eval-key".to_string(), key_for(7).verifying_key());
    let json = serde_json::to_vec(&receipt).expect("serialize");
    let (_, verified) = verifier
        .parse_and_verify(&json, &receipt_policy())
        .expect("control: the receipt fixture must verify under the receipt domain");
    assert_eq!(verified.authority, VerifiedAuthority::AuthoritativeCi);

    // And an empty policy must NOT confer authority — the product's own
    // fail-closed rule, pinned here so a future change to it is visible.
    assert_eq!(
        verifier
            .parse_and_verify(&json, &VerificationPolicy::default())
            .expect_err("an empty verification policy must never confer CI authority"),
        ReceiptError::UnsignedAuthoritative
    );
}
