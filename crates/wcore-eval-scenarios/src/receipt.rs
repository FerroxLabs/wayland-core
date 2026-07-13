//! Versioned, content-addressed evaluation evidence receipts.
//!
//! The receipt body contains only structured, redacted evidence. Authority is
//! derived by [`ReceiptVerifier`] from a detached signature and externally
//! configured trusted key; it is never trusted from a boolean in the receipt.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const RECEIPT_SCHEMA: &str = "wayland.eval.receipt";
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;
const SIGNATURE_DOMAIN: &[u8] = b"wayland.eval.receipt.v1\0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Evidence<T> {
    Observed { value: T },
    Unavailable { code: String },
}

impl<T> Evidence<T> {
    pub fn observed(value: T) -> Self {
        Self::Observed { value }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceReceiptV1 {
    pub schema: String,
    pub schema_version: u32,
    pub body_sha256: String,
    pub body: ReceiptBodyV1,
    pub authority: AuthorityClaimV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthorityClaimV1 {
    Local,
    Ci {
        key_id: String,
        signature_base64: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptBodyV1 {
    pub run_id: String,
    pub identity: IdentityEvidenceV1,
    pub target: TargetEvidenceV1,
    pub policy: PolicyEvidenceV1,
    pub timings: TimingEvidenceV1,
    pub provider: ProviderEvidenceV1,
    pub tools: Vec<ToolEvidenceV1>,
    pub decisions: Vec<DecisionEvidenceV1>,
    pub boundaries: BoundaryEvidenceV1,
    pub process: ProcessEvidenceV1,
    pub recovery: RecoveryEvidenceV1,
    pub canary_scans: CanaryScanEvidenceV1,
    pub assertions: Vec<AssertionEvidenceV1>,
    pub quarantines: Vec<QuarantineEvidenceV1>,
    pub required_cells: Vec<String>,
    pub results: Vec<CellResultV1>,
    pub summary: SummaryEvidenceV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityEvidenceV1 {
    pub source_commit: String,
    pub binary_sha256: String,
    pub config_sha256: String,
    pub fixture_sha256: String,
    pub provider: String,
    pub model: String,
    pub build: Evidence<BuildProvenanceV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildProvenanceV1 {
    pub repository: String,
    pub source_ref: String,
    pub workflow: String,
    pub invocation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetEvidenceV1 {
    pub os: String,
    pub architecture: String,
    pub sandbox_backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyEvidenceV1 {
    pub posture: String,
    pub effective_policy_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingEvidenceV1 {
    pub boot_ms: Evidence<u64>,
    pub ready_ms: Evidence<u64>,
    pub prompt_ms: Evidence<u64>,
    pub first_token_ms: Evidence<u64>,
    pub tool_ms: Evidence<u64>,
    pub approval_ms: Evidence<u64>,
    pub completion_ms: Evidence<u64>,
    pub shutdown_ms: Evidence<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEvidenceV1 {
    pub attempts: u64,
    pub typed_failures: Vec<String>,
    pub retries: u64,
    pub input_tokens: Evidence<u64>,
    pub output_tokens: Evidence<u64>,
    pub cache_read_tokens: Evidence<u64>,
    pub cache_write_tokens: Evidence<u64>,
    pub cost_microusd: u64,
    pub limit_microusd: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEvidenceV1 {
    pub call_id_sha256: String,
    pub tool_name: String,
    pub request_sha256: String,
    pub result_sha256: String,
    pub duration_ms: Evidence<u64>,
    pub exit_state: String,
    pub idempotency_key_sha256: Evidence<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEvidenceV1 {
    pub actor: String,
    pub action: String,
    pub resource_sha256: String,
    pub scope: String,
    pub decision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryEvidenceV1 {
    pub egress_attempted: Vec<String>,
    pub egress_allowed: Vec<String>,
    pub egress_denied: Vec<String>,
    pub filesystem_deltas: Vec<FilesystemDeltaV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemDeltaV1 {
    pub path_sha256: String,
    pub operation: String,
    pub content_sha256: Evidence<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEvidenceV1 {
    pub tree_sha256: String,
    pub peak_memory_bytes: Evidence<u64>,
    pub peak_cpu_millis: Evidence<u64>,
    pub cancellation_requested: bool,
    pub orphan_count: Evidence<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryEvidenceV1 {
    pub journal_cursor_sha256: Evidence<String>,
    pub action: String,
    pub unresolved_side_effects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanaryScanEvidenceV1 {
    pub scan_complete: bool,
    pub protocol: u64,
    pub stdout: u64,
    pub stderr: u64,
    pub files: u64,
    pub logs: u64,
    pub telemetry: u64,
}

impl CanaryScanEvidenceV1 {
    fn detections(&self) -> u64 {
        self.protocol
            .saturating_add(self.stdout)
            .saturating_add(self.stderr)
            .saturating_add(self.files)
            .saturating_add(self.logs)
            .saturating_add(self.telemetry)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionEvidenceV1 {
    pub assertion_id: String,
    pub passed: bool,
    pub failure_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineEvidenceV1 {
    pub assertion_id: String,
    pub owner: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellResultV1 {
    pub cell_id: String,
    pub task: String,
    pub provider: String,
    pub platform: String,
    pub passed: bool,
    pub failures: Vec<FailureEvidenceV1>,
    pub usability: Vec<UsabilityEvidenceV1>,
    pub wall_time_ms: u64,
    pub cost_microusd: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureEvidenceV1 {
    pub code: String,
    pub detail_sha256: Evidence<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsabilityEvidenceV1 {
    pub severity: String,
    pub code: String,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryEvidenceV1 {
    pub passed: u64,
    pub failed: u64,
    pub total_cost_microusd: u64,
    pub wall_time_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedAuthority {
    LocalNonAuthoritative,
    AuthoritativeCi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedReceipt {
    pub authority: VerifiedAuthority,
    pub gate_passed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct VerificationPolicy {
    pub source_commit: Option<String>,
    pub binary_sha256: Option<String>,
    pub repository: Option<String>,
    pub source_ref: Option<String>,
    pub workflow: Option<String>,
}

#[derive(Debug, Default)]
pub struct ReceiptVerifier {
    trusted_ci_keys: BTreeMap<String, VerifyingKey>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReceiptError {
    #[error("unsupported receipt schema {schema} version {version}")]
    UnsupportedSchema { schema: String, version: u32 },
    #[error("receipt body digest mismatch")]
    DigestMismatch,
    #[error("missing or invalid receipt evidence: {0}")]
    InvalidEvidence(String),
    #[error("authoritative receipt has no trusted CI provenance")]
    UnsignedAuthoritative,
    #[error("CI provenance key is not trusted: {0}")]
    UntrustedKey(String),
    #[error("CI provenance signature is malformed")]
    MalformedSignature,
    #[error("CI provenance signature verification failed")]
    InvalidSignature,
    #[error("CI provenance does not match verification policy: {0}")]
    ProvenanceMismatch(String),
}

impl EvidenceReceiptV1 {
    pub fn local(body: ReceiptBodyV1) -> Result<Self, ReceiptError> {
        validate_body(&body)?;
        Ok(Self {
            schema: RECEIPT_SCHEMA.to_string(),
            schema_version: RECEIPT_SCHEMA_VERSION,
            body_sha256: body_digest(&body)?,
            body,
            authority: AuthorityClaimV1::Local,
        })
    }

    /// Attach a detached CI signature. Possession of this object never makes
    /// it authoritative; a verifier must trust `key_id` out of band.
    pub fn sign_ci(mut self, key_id: impl Into<String>, key: &SigningKey) -> Self {
        let signature = key.sign(&signature_message(&self.body_sha256));
        self.authority = AuthorityClaimV1::Ci {
            key_id: key_id.into(),
            signature_base64: BASE64.encode(signature.to_bytes()),
        };
        self
    }
}

impl ReceiptVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trust_ci_key(&mut self, key_id: impl Into<String>, key: VerifyingKey) {
        self.trusted_ci_keys.insert(key_id.into(), key);
    }

    pub fn verify(
        &self,
        receipt: &EvidenceReceiptV1,
        policy: &VerificationPolicy,
    ) -> Result<VerifiedReceipt, ReceiptError> {
        if receipt.schema != RECEIPT_SCHEMA || receipt.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(ReceiptError::UnsupportedSchema {
                schema: receipt.schema.clone(),
                version: receipt.schema_version,
            });
        }
        validate_body(&receipt.body)?;
        if body_digest(&receipt.body)? != receipt.body_sha256 {
            return Err(ReceiptError::DigestMismatch);
        }

        let gate_passed = gate_passed(&receipt.body);
        match &receipt.authority {
            AuthorityClaimV1::Local => Ok(VerifiedReceipt {
                authority: VerifiedAuthority::LocalNonAuthoritative,
                gate_passed,
            }),
            AuthorityClaimV1::Ci {
                key_id,
                signature_base64,
            } => {
                validate_ci_provenance(&receipt.body, policy)?;
                let key = self
                    .trusted_ci_keys
                    .get(key_id)
                    .ok_or_else(|| ReceiptError::UntrustedKey(key_id.clone()))?;
                let signature_bytes = BASE64
                    .decode(signature_base64)
                    .map_err(|_| ReceiptError::MalformedSignature)?;
                let signature = Signature::from_slice(&signature_bytes)
                    .map_err(|_| ReceiptError::MalformedSignature)?;
                key.verify(&signature_message(&receipt.body_sha256), &signature)
                    .map_err(|_| ReceiptError::InvalidSignature)?;
                Ok(VerifiedReceipt {
                    authority: VerifiedAuthority::AuthoritativeCi,
                    gate_passed,
                })
            }
        }
    }
}

fn signature_message(body_sha256: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNATURE_DOMAIN.len() + body_sha256.len());
    message.extend_from_slice(SIGNATURE_DOMAIN);
    message.extend_from_slice(body_sha256.as_bytes());
    message
}

fn body_digest(body: &ReceiptBodyV1) -> Result<String, ReceiptError> {
    let bytes = serde_json::to_vec(body)
        .map_err(|error| ReceiptError::InvalidEvidence(format!("canonical JSON: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_body(body: &ReceiptBodyV1) -> Result<(), ReceiptError> {
    require_nonempty("run_id", &body.run_id)?;
    require_sha256("identity.source_commit", &body.identity.source_commit, 40)?;
    require_sha256("identity.binary_sha256", &body.identity.binary_sha256, 64)?;
    require_sha256("identity.config_sha256", &body.identity.config_sha256, 64)?;
    require_sha256("identity.fixture_sha256", &body.identity.fixture_sha256, 64)?;
    require_nonempty("identity.provider", &body.identity.provider)?;
    require_nonempty("identity.model", &body.identity.model)?;
    require_nonempty("target.os", &body.target.os)?;
    require_nonempty("target.architecture", &body.target.architecture)?;
    require_nonempty("target.sandbox_backend", &body.target.sandbox_backend)?;
    require_nonempty("policy.posture", &body.policy.posture)?;
    require_sha256(
        "policy.effective_policy_sha256",
        &body.policy.effective_policy_sha256,
        64,
    )?;
    if body.required_cells.is_empty() {
        return Err(ReceiptError::InvalidEvidence(
            "required_cells must not be empty".to_string(),
        ));
    }
    let required = unique_set("required_cells", &body.required_cells)?;
    let result_ids = body
        .results
        .iter()
        .map(|result| result.cell_id.clone())
        .collect::<Vec<_>>();
    let actual = unique_set("results.cell_id", &result_ids)?;
    if required != actual {
        return Err(ReceiptError::InvalidEvidence(
            "required cell manifest does not exactly match results".to_string(),
        ));
    }

    let mut passed = 0_u64;
    let mut failed = 0_u64;
    let mut total_cost_microusd = 0_u64;
    let mut wall_time_ms = 0_u64;
    for result in &body.results {
        require_nonempty("result.cell_id", &result.cell_id)?;
        require_nonempty("result.task", &result.task)?;
        require_nonempty("result.provider", &result.provider)?;
        require_nonempty("result.platform", &result.platform)?;
        let critical_usability = result
            .usability
            .iter()
            .any(|finding| finding.severity == "high" || finding.severity == "critical");
        if result.passed && (!result.failures.is_empty() || critical_usability) {
            return Err(ReceiptError::InvalidEvidence(format!(
                "result {} passes despite failure evidence",
                result.cell_id
            )));
        }
        if !result.passed && result.failures.is_empty() && !critical_usability {
            return Err(ReceiptError::InvalidEvidence(format!(
                "result {} fails without a stable reason",
                result.cell_id
            )));
        }
        if result.passed {
            passed += 1;
        } else {
            failed += 1;
        }
        total_cost_microusd = total_cost_microusd.saturating_add(result.cost_microusd);
        wall_time_ms = wall_time_ms.saturating_add(result.wall_time_ms);
    }
    if body.summary.passed != passed
        || body.summary.failed != failed
        || body.summary.wall_time_ms != wall_time_ms
        || body.summary.total_cost_microusd != total_cost_microusd
    {
        return Err(ReceiptError::InvalidEvidence(
            "summary does not match derived result totals".to_string(),
        ));
    }
    Ok(())
}

fn validate_ci_provenance(
    body: &ReceiptBodyV1,
    policy: &VerificationPolicy,
) -> Result<(), ReceiptError> {
    let Evidence::Observed { value: build } = &body.identity.build else {
        return Err(ReceiptError::UnsignedAuthoritative);
    };
    check_expected(
        "source commit",
        policy.source_commit.as_deref(),
        &body.identity.source_commit,
    )?;
    check_expected(
        "binary digest",
        policy.binary_sha256.as_deref(),
        &body.identity.binary_sha256,
    )?;
    check_expected(
        "repository",
        policy.repository.as_deref(),
        &build.repository,
    )?;
    check_expected(
        "source ref",
        policy.source_ref.as_deref(),
        &build.source_ref,
    )?;
    check_expected("workflow", policy.workflow.as_deref(), &build.workflow)?;
    if policy.source_commit.is_none()
        || policy.binary_sha256.is_none()
        || policy.repository.is_none()
        || policy.source_ref.is_none()
        || policy.workflow.is_none()
    {
        return Err(ReceiptError::UnsignedAuthoritative);
    }
    Ok(())
}

fn check_expected(field: &str, expected: Option<&str>, observed: &str) -> Result<(), ReceiptError> {
    if expected.is_some_and(|expected| expected != observed) {
        return Err(ReceiptError::ProvenanceMismatch(field.to_string()));
    }
    Ok(())
}

fn gate_passed(body: &ReceiptBodyV1) -> bool {
    body.results.iter().all(|result| result.passed)
        && body.canary_scans.scan_complete
        && body.canary_scans.detections() == 0
        && matches!(body.process.orphan_count, Evidence::Observed { value: 0 })
        && body.recovery.unresolved_side_effects.is_empty()
        && body.assertions.iter().all(|assertion| assertion.passed)
}

fn require_nonempty(field: &str, value: &str) -> Result<(), ReceiptError> {
    if value.trim().is_empty() {
        return Err(ReceiptError::InvalidEvidence(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn require_sha256(field: &str, value: &str, length: usize) -> Result<(), ReceiptError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ReceiptError::InvalidEvidence(format!(
            "{field} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn unique_set(field: &str, values: &[String]) -> Result<BTreeSet<String>, ReceiptError> {
    let set = values.iter().cloned().collect::<BTreeSet<_>>();
    if set.len() != values.len() {
        return Err(ReceiptError::InvalidEvidence(format!(
            "{field} contains duplicates"
        )));
    }
    Ok(set)
}
