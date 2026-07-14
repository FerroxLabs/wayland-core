//! Authoritative release policy for evaluation receipts.
//!
//! Receipt integrity and release authority are separate decisions. The normal
//! evaluator emits local receipts; only this policy surface can promote a
//! receipt to authoritative release evidence.

use std::collections::BTreeSet;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::receipt::{
    AuthorityClaimV1, BuildProvenanceV1, Evidence, EvidenceReceiptV1, ReceiptError,
    ReceiptVerifier, VerificationPolicy, VerifiedAuthority, VerifiedReceipt,
    milestone_evidence_gaps,
};

pub const AUTHORITY_POLICY_SCHEMA: &str = "wayland.eval.authority-policy";
pub const AUTHORITY_POLICY_SCHEMA_VERSION: u32 = 1;

/// Trusted inputs supplied independently from the receipt under review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthoritativeReceiptPolicyV1 {
    pub schema: String,
    pub schema_version: u32,
    pub key_id: String,
    pub public_key_base64: String,
    pub source_commit: String,
    pub binary_sha256: String,
    pub config_sha256: String,
    pub fixture_sha256: String,
    pub provider: String,
    pub model: String,
    pub repository: String,
    pub source_ref: String,
    pub workflow: String,
    pub invocation_id: String,
    pub target_os: String,
    pub target_architecture: String,
    pub sandbox_backend: String,
    pub policy_posture: String,
    pub effective_policy_sha256: String,
    pub required_cells: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CiProvenanceV1 {
    pub repository: String,
    pub source_ref: String,
    pub workflow: String,
    pub invocation_id: String,
}

#[derive(Debug, Error)]
pub enum AuthorityError {
    #[error(transparent)]
    Receipt(#[from] ReceiptError),
    #[error("invalid authority policy: {0}")]
    InvalidPolicy(String),
    #[error("trusted public key is not a 32-byte base64 Ed25519 key")]
    InvalidPublicKey,
    #[error("signing key is not a 32-byte base64 Ed25519 key")]
    InvalidSigningKey,
    #[error("only a local receipt can enter the CI signing workflow")]
    NonLocalSigningInput,
    #[error("receipt is not signed by the policy's trusted authority")]
    WrongAuthority,
    #[error("receipt does not match authoritative policy field: {0}")]
    PolicyMismatch(&'static str),
    #[error("fixture digest is a synthetic binary/scenario label, not fixture provenance")]
    SyntheticFixtureDigest,
    #[error("authoritative receipt is missing required milestone evidence: {0:?}")]
    MilestoneGateFailed(Vec<String>),
}

struct SecretBytes(Vec<u8>);

impl Drop for SecretBytes {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Attach CI provenance and sign a local receipt. The secret is supplied as
/// bytes by the caller so command-line integrations never need a secret argv.
pub fn sign_ci_receipt(
    receipt_json: &[u8],
    key_id: &str,
    signing_key_base64: &[u8],
    provenance: CiProvenanceV1,
) -> Result<EvidenceReceiptV1, AuthorityError> {
    require_nonempty("key_id", key_id)?;
    validate_provenance(&provenance)?;

    let (receipt, _) =
        ReceiptVerifier::new().parse_and_verify(receipt_json, &VerificationPolicy::default())?;
    if !matches!(receipt.authority, AuthorityClaimV1::Local) {
        return Err(AuthorityError::NonLocalSigningInput);
    }
    reject_synthetic_fixture(&receipt)?;

    let mut key_bytes =
        decode_secret_32(signing_key_base64).ok_or(AuthorityError::InvalidSigningKey)?;
    let signing_key = SigningKey::from_bytes(&key_bytes);
    wipe(&mut key_bytes);
    let mut body = receipt.body;
    body.identity.build = Evidence::observed(BuildProvenanceV1 {
        repository: provenance.repository,
        source_ref: provenance.source_ref,
        workflow: provenance.workflow,
        invocation_id: provenance.invocation_id,
    });
    Ok(EvidenceReceiptV1::local(body)?.sign_ci(key_id, &signing_key))
}

/// Verify one receipt as authoritative release evidence. Success means the
/// signature, every exact policy binding, and the complete evidence gate pass.
pub fn verify_authoritative_receipt(
    receipt_json: &[u8],
    policy: &AuthoritativeReceiptPolicyV1,
) -> Result<(EvidenceReceiptV1, VerifiedReceipt), AuthorityError> {
    validate_authoritative_policy(policy)?;
    let public_key_bytes =
        decode_32(policy.public_key_base64.as_bytes()).ok_or(AuthorityError::InvalidPublicKey)?;
    let public_key = VerifyingKey::from_bytes(&public_key_bytes)
        .map_err(|_| AuthorityError::InvalidPublicKey)?;
    let mut verifier = ReceiptVerifier::new();
    verifier.trust_ci_key(policy.key_id.clone(), public_key);
    let verification_policy = VerificationPolicy {
        source_commit: Some(policy.source_commit.clone()),
        binary_sha256: Some(policy.binary_sha256.clone()),
        repository: Some(policy.repository.clone()),
        source_ref: Some(policy.source_ref.clone()),
        workflow: Some(policy.workflow.clone()),
    };
    let (receipt, verified) = verifier.parse_and_verify(receipt_json, &verification_policy)?;
    if verified.authority != VerifiedAuthority::AuthoritativeCi
        || !matches!(
            &receipt.authority,
            AuthorityClaimV1::Ci { key_id, .. } if key_id == &policy.key_id
        )
    {
        return Err(AuthorityError::WrongAuthority);
    }

    exact(
        "config_sha256",
        &policy.config_sha256,
        &receipt.body.identity.config_sha256,
    )?;
    exact(
        "fixture_sha256",
        &policy.fixture_sha256,
        &receipt.body.identity.fixture_sha256,
    )?;
    exact(
        "provider",
        &policy.provider,
        &receipt.body.identity.provider,
    )?;
    exact("model", &policy.model, &receipt.body.identity.model)?;
    let Evidence::Observed { value: build } = &receipt.body.identity.build else {
        return Err(AuthorityError::PolicyMismatch("invocation_id"));
    };
    exact("invocation_id", &policy.invocation_id, &build.invocation_id)?;
    exact("target_os", &policy.target_os, &receipt.body.target.os)?;
    exact(
        "target_architecture",
        &policy.target_architecture,
        &receipt.body.target.architecture,
    )?;
    exact(
        "sandbox_backend",
        &policy.sandbox_backend,
        &receipt.body.target.sandbox_backend,
    )?;
    exact(
        "policy_posture",
        &policy.policy_posture,
        &receipt.body.policy.posture,
    )?;
    exact(
        "effective_policy_sha256",
        &policy.effective_policy_sha256,
        &receipt.body.policy.effective_policy_sha256,
    )?;
    let expected_cells = policy.required_cells.iter().collect::<BTreeSet<_>>();
    let observed_cells = receipt.body.required_cells.iter().collect::<BTreeSet<_>>();
    if expected_cells != observed_cells {
        return Err(AuthorityError::PolicyMismatch("required_cells"));
    }
    reject_synthetic_fixture(&receipt)?;
    if !verified.gate_passed {
        return Err(AuthorityError::MilestoneGateFailed(
            milestone_evidence_gaps(&receipt.body)
                .into_iter()
                .map(str::to_string)
                .collect(),
        ));
    }
    Ok((receipt, verified))
}

pub fn validate_authoritative_policy(
    policy: &AuthoritativeReceiptPolicyV1,
) -> Result<(), AuthorityError> {
    if policy.schema != AUTHORITY_POLICY_SCHEMA
        || policy.schema_version != AUTHORITY_POLICY_SCHEMA_VERSION
    {
        return Err(AuthorityError::InvalidPolicy(
            "unsupported schema or schema version".to_string(),
        ));
    }
    require_nonempty("key_id", &policy.key_id)?;
    require_hex("source_commit", &policy.source_commit, 40)?;
    require_hex("binary_sha256", &policy.binary_sha256, 64)?;
    require_hex("config_sha256", &policy.config_sha256, 64)?;
    require_hex("fixture_sha256", &policy.fixture_sha256, 64)?;
    require_nonempty("provider", &policy.provider)?;
    require_nonempty("model", &policy.model)?;
    require_nonempty("repository", &policy.repository)?;
    require_nonempty("source_ref", &policy.source_ref)?;
    require_nonempty("workflow", &policy.workflow)?;
    require_nonempty("invocation_id", &policy.invocation_id)?;
    require_nonempty("target_os", &policy.target_os)?;
    require_nonempty("target_architecture", &policy.target_architecture)?;
    require_nonempty("sandbox_backend", &policy.sandbox_backend)?;
    require_nonempty("policy_posture", &policy.policy_posture)?;
    require_hex(
        "effective_policy_sha256",
        &policy.effective_policy_sha256,
        64,
    )?;
    if policy.required_cells.is_empty()
        || policy
            .required_cells
            .iter()
            .any(|cell| cell.trim().is_empty())
        || policy.required_cells.iter().collect::<BTreeSet<_>>().len()
            != policy.required_cells.len()
    {
        return Err(AuthorityError::InvalidPolicy(
            "required_cells must be a non-empty unique manifest".to_string(),
        ));
    }
    Ok(())
}

fn validate_provenance(provenance: &CiProvenanceV1) -> Result<(), AuthorityError> {
    require_nonempty("repository", &provenance.repository)?;
    require_nonempty("source_ref", &provenance.source_ref)?;
    require_nonempty("workflow", &provenance.workflow)?;
    require_nonempty("invocation_id", &provenance.invocation_id)
}

fn reject_synthetic_fixture(receipt: &EvidenceReceiptV1) -> Result<(), AuthorityError> {
    if receipt.body.results.iter().any(|result| {
        let synthetic = format!(
            "{:x}",
            Sha256::digest(format!(
                "{}:{}",
                receipt.body.identity.binary_sha256, result.task
            ))
        );
        synthetic == receipt.body.identity.fixture_sha256
    }) {
        return Err(AuthorityError::SyntheticFixtureDigest);
    }
    Ok(())
}

fn exact(field: &'static str, expected: &str, observed: &str) -> Result<(), AuthorityError> {
    if expected != observed {
        return Err(AuthorityError::PolicyMismatch(field));
    }
    Ok(())
}

fn decode_32(encoded: &[u8]) -> Option<[u8; 32]> {
    let decoded = BASE64.decode(trim_ascii(encoded)).ok()?;
    decoded.try_into().ok()
}

fn decode_secret_32(encoded: &[u8]) -> Option<[u8; 32]> {
    let decoded = SecretBytes(BASE64.decode(trim_ascii(encoded)).ok()?);
    decoded.0.as_slice().try_into().ok()
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: `byte` is a valid unique reference for this write. Volatile
        // prevents the compiler from eliding this security-sensitive wipe.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn require_nonempty(field: &str, value: &str) -> Result<(), AuthorityError> {
    if value.trim().is_empty() {
        return Err(AuthorityError::InvalidPolicy(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn require_hex(field: &str, value: &str, length: usize) -> Result<(), AuthorityError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AuthorityError::InvalidPolicy(format!(
            "{field} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(())
}
