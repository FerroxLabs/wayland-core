//! Versioned, content-addressed evaluation evidence receipts.
//!
//! The receipt body contains only structured, redacted evidence. Authority is
//! derived by [`ReceiptVerifier`] from a detached signature and externally
//! configured trusted key; it is never trusted from a boolean in the receipt.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::runner::{Failure, ScenarioResult};
use crate::usability::{self, Severity};

pub const RECEIPT_SCHEMA: &str = "wayland.eval.receipt";
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const BEHAVIOR_SCHEMA: &str = "wayland.eval.behavior";
pub const BEHAVIOR_SCHEMA_VERSION: u32 = 1;
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
    pub attempts: Evidence<u64>,
    pub typed_failures: Vec<String>,
    pub retries: Evidence<u64>,
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
    pub egress_attempted: Evidence<Vec<String>>,
    pub egress_allowed: Evidence<Vec<String>>,
    pub egress_denied: Evidence<Vec<String>>,
    pub filesystem_deltas: Evidence<Vec<FilesystemDeltaV1>>,
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

#[derive(Serialize)]
struct BehaviorProjectionV1<'a> {
    schema: &'static str,
    schema_version: u32,
    identity: BehaviorIdentityV1<'a>,
    target: &'a TargetEvidenceV1,
    policy: &'a PolicyEvidenceV1,
    provider: &'a ProviderEvidenceV1,
    tools: Vec<BehaviorToolV1<'a>>,
    decisions: &'a [DecisionEvidenceV1],
    boundaries: &'a BoundaryEvidenceV1,
    process: BehaviorProcessV1<'a>,
    recovery: BehaviorRecoveryV1<'a>,
    canary_scans: &'a CanaryScanEvidenceV1,
    assertions: &'a [AssertionEvidenceV1],
    quarantines: &'a [QuarantineEvidenceV1],
    required_cells: &'a [String],
    results: Vec<BehaviorResultV1<'a>>,
    summary: BehaviorSummaryV1,
}

#[derive(Serialize)]
struct BehaviorIdentityV1<'a> {
    source_commit: &'a str,
    binary_sha256: &'a str,
    config_sha256: &'a str,
    fixture_sha256: &'a str,
    provider: &'a str,
    model: &'a str,
}

#[derive(Serialize)]
struct BehaviorToolV1<'a> {
    tool_name: &'a str,
    request_sha256: &'a str,
    result_sha256: &'a str,
    exit_state: &'a str,
    idempotency_key_sha256: &'a Evidence<String>,
}

#[derive(Serialize)]
struct BehaviorProcessV1<'a> {
    cancellation_requested: bool,
    orphan_count: &'a Evidence<u64>,
}

#[derive(Serialize)]
struct BehaviorRecoveryV1<'a> {
    action: &'a str,
    unresolved_side_effects: &'a [String],
}

#[derive(Serialize)]
struct BehaviorResultV1<'a> {
    cell_id: &'a str,
    task: &'a str,
    provider: &'a str,
    platform: &'a str,
    passed: bool,
    failures: &'a [FailureEvidenceV1],
    usability: &'a [UsabilityEvidenceV1],
    cost_microusd: u64,
}

#[derive(Serialize)]
struct BehaviorSummaryV1 {
    passed: u64,
    failed: u64,
    total_cost_microusd: u64,
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

#[derive(Debug, Clone)]
pub struct ReceiptMetadataV1 {
    pub run_id: String,
    pub source_commit: String,
    pub binary_sha256: String,
    pub fixture_sha256: String,
    pub model: String,
    pub build: Evidence<BuildProvenanceV1>,
}

#[derive(Debug, Default)]
pub struct ReceiptVerifier {
    trusted_ci_keys: BTreeMap<String, VerifyingKey>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReceiptError {
    #[error("invalid receipt JSON: {0}")]
    InvalidJson(String),
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

    /// Hash the repeatable behavior contract while excluding run identity,
    /// provenance invocation, timings, process identity, and resource samples.
    /// The full receipt remains content-addressed by `body_sha256`; this second
    /// digest is only the cross-run determinism oracle.
    pub fn behavior_sha256(&self) -> Result<String, ReceiptError> {
        if self.schema != RECEIPT_SCHEMA || self.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(ReceiptError::UnsupportedSchema {
                schema: self.schema.clone(),
                version: self.schema_version,
            });
        }
        validate_body(&self.body)?;
        if body_digest(&self.body)? != self.body_sha256 {
            return Err(ReceiptError::DigestMismatch);
        }
        behavior_digest(&self.body)
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

    pub fn from_scenario_result(
        metadata: ReceiptMetadataV1,
        result: &ScenarioResult,
        limit_usd: f64,
    ) -> Result<Self, ReceiptError> {
        let cost_microusd = usd_to_microusd("result.cost_usd", result.cost_usd)?;
        let limit_microusd = usd_to_microusd("scenario.limit_usd", limit_usd)?;
        let failure_evidence = result
            .failures
            .iter()
            .map(|failure| {
                Ok(FailureEvidenceV1 {
                    code: failure_code(failure).to_string(),
                    detail_sha256: Evidence::observed(hash_serializable(failure)?),
                })
            })
            .collect::<Result<Vec<_>, ReceiptError>>()?;
        let usability = usability::scan(result)
            .into_iter()
            .map(|finding| UsabilityEvidenceV1 {
                severity: match finding.severity {
                    Severity::Low => "low",
                    Severity::Medium => "medium",
                    Severity::High => "high",
                }
                .to_string(),
                code: finding.category.to_string(),
                evidence_sha256: sha256(finding.evidence.as_bytes()),
            })
            .collect::<Vec<_>>();
        let critical_usability = usability
            .iter()
            .any(|finding| finding.severity == "high" || finding.severity == "critical");
        let passed = result.passed && failure_evidence.is_empty() && !critical_usability;
        let outcome_failure_code = failure_evidence
            .first()
            .map(|failure| failure.code.clone())
            .or_else(|| {
                usability
                    .iter()
                    .find(|finding| finding.severity == "high" || finding.severity == "critical")
                    .map(|finding| finding.code.clone())
            });
        let cell_id = format!(
            "{}/{}/{}",
            result.name,
            result.provider.cli_name(),
            result.platform
        );
        let tools = result
            .trace
            .entries
            .iter()
            .map(|entry| ToolEvidenceV1 {
                call_id_sha256: sha256(entry.call_id.as_bytes()),
                tool_name: entry.tool_name.clone(),
                request_sha256: sha256(entry.input.as_bytes()),
                result_sha256: sha256(entry.output.as_bytes()),
                duration_ms: entry.duration.map_or_else(
                    || Evidence::Unavailable {
                        code: "duration_not_observed".to_string(),
                    },
                    |duration| Evidence::observed(duration.as_millis() as u64),
                ),
                exit_state: if entry.is_error { "error" } else { "success" }.to_string(),
                idempotency_key_sha256: Evidence::Unavailable {
                    code: "not_emitted_by_protocol_v1".to_string(),
                },
            })
            .collect::<Vec<_>>();
        let tool_ms = result
            .trace
            .entries
            .iter()
            .filter_map(|entry| entry.duration)
            .fold(0_u64, |sum, duration| {
                sum.saturating_add(duration.as_millis() as u64)
            });
        let policy_sha256 = sha256(
            format!(
                "{}:{}:{}",
                result.approval, result.execution.config_sha256, result.execution.sandbox_backend
            )
            .as_bytes(),
        );
        let process_orphans = if result.execution.cleanup_verified {
            Evidence::observed(0)
        } else {
            Evidence::Unavailable {
                code: "cleanup_not_verified".to_string(),
            }
        };
        let mut canary_scans = CanaryScanEvidenceV1 {
            scan_complete: result.execution.artifact_scan_complete,
            protocol: 0,
            stdout: 0,
            stderr: 0,
            files: 0,
            logs: 0,
            telemetry: 0,
        };
        for failure in &result.failures {
            if let Failure::SecretDetected { sink } = failure {
                if sink == "stdout" {
                    canary_scans.stdout = canary_scans.stdout.saturating_add(1);
                } else if sink == "stderr" {
                    canary_scans.stderr = canary_scans.stderr.saturating_add(1);
                } else if sink.starts_with("artifact:") {
                    canary_scans.files = canary_scans.files.saturating_add(1);
                } else {
                    canary_scans.protocol = canary_scans.protocol.saturating_add(1);
                }
            }
        }
        let summary = SummaryEvidenceV1 {
            passed: u64::from(passed),
            failed: u64::from(!passed),
            total_cost_microusd: cost_microusd,
            wall_time_ms: result.wall_time.as_millis() as u64,
        };
        let mut decisions = vec![DecisionEvidenceV1 {
            actor: "evaluator".to_string(),
            action: "approval_posture".to_string(),
            resource_sha256: sha256(cell_id.as_bytes()),
            scope: "scenario".to_string(),
            decision: result.approval.to_string(),
        }];
        decisions.extend(result.execution.approval_commands.iter().map(|command| {
            DecisionEvidenceV1 {
                actor: "evaluator".to_string(),
                action: "tool_approval_command".to_string(),
                resource_sha256: sha256(command.call_id.as_bytes()),
                scope: "once".to_string(),
                decision: if command.approved {
                    "approve_sent".to_string()
                } else {
                    "deny_sent".to_string()
                },
            }
        }));
        let body = ReceiptBodyV1 {
            run_id: metadata.run_id,
            identity: IdentityEvidenceV1 {
                source_commit: metadata.source_commit,
                binary_sha256: metadata.binary_sha256,
                config_sha256: result.execution.config_sha256.clone(),
                fixture_sha256: metadata.fixture_sha256,
                provider: result.provider.cli_name().to_string(),
                model: metadata.model,
                build: metadata.build,
            },
            target: TargetEvidenceV1 {
                os: result.platform.to_string(),
                architecture: std::env::consts::ARCH.to_string(),
                sandbox_backend: result.execution.sandbox_backend.clone(),
            },
            policy: PolicyEvidenceV1 {
                posture: result.approval.to_string(),
                effective_policy_sha256: policy_sha256,
            },
            timings: TimingEvidenceV1 {
                boot_ms: Evidence::observed(result.boot_time.as_millis() as u64),
                ready_ms: Evidence::observed(result.boot_time.as_millis() as u64),
                prompt_ms: Evidence::observed(
                    result.execution.prompt_dispatch_time.as_millis() as u64
                ),
                first_token_ms: result.execution.first_token_time.map_or_else(
                    || Evidence::Unavailable {
                        code: "no_text_delta_observed".to_string(),
                    },
                    |duration| Evidence::observed(duration.as_millis() as u64),
                ),
                tool_ms: Evidence::observed(tool_ms),
                approval_ms: Evidence::observed(
                    result.execution.approval_response_time.as_millis() as u64,
                ),
                completion_ms: Evidence::observed(result.wall_time.as_millis() as u64),
                shutdown_ms: Evidence::observed(result.execution.shutdown_time.as_millis() as u64),
            },
            provider: ProviderEvidenceV1 {
                attempts: Evidence::Unavailable {
                    code: "provider_attempts_not_emitted".to_string(),
                },
                typed_failures: failure_evidence
                    .iter()
                    .map(|failure| failure.code.clone())
                    .collect(),
                retries: Evidence::Unavailable {
                    code: "provider_retries_not_emitted".to_string(),
                },
                input_tokens: Evidence::Unavailable {
                    code: "provider_usage_not_emitted".to_string(),
                },
                output_tokens: Evidence::Unavailable {
                    code: "provider_usage_not_emitted".to_string(),
                },
                cache_read_tokens: Evidence::Unavailable {
                    code: "provider_usage_not_emitted".to_string(),
                },
                cache_write_tokens: Evidence::Unavailable {
                    code: "provider_usage_not_emitted".to_string(),
                },
                cost_microusd,
                limit_microusd,
            },
            tools,
            decisions,
            boundaries: BoundaryEvidenceV1 {
                egress_attempted: Evidence::Unavailable {
                    code: "egress_recorder_not_enabled".to_string(),
                },
                egress_allowed: Evidence::Unavailable {
                    code: "egress_recorder_not_enabled".to_string(),
                },
                egress_denied: Evidence::Unavailable {
                    code: "egress_recorder_not_enabled".to_string(),
                },
                filesystem_deltas: Evidence::Unavailable {
                    code: "filesystem_delta_recorder_not_enabled".to_string(),
                },
            },
            process: ProcessEvidenceV1 {
                tree_sha256: result.execution.process_tree_sha256.clone(),
                peak_memory_bytes: Evidence::Unavailable {
                    code: "resource_sampler_not_enabled".to_string(),
                },
                peak_cpu_millis: Evidence::Unavailable {
                    code: "resource_sampler_not_enabled".to_string(),
                },
                cancellation_requested: result.execution.cancellation_requested,
                orphan_count: process_orphans,
            },
            recovery: RecoveryEvidenceV1 {
                journal_cursor_sha256: Evidence::Unavailable {
                    code: "no_recovery_journal_for_scenario".to_string(),
                },
                action: "none".to_string(),
                unresolved_side_effects: Vec::new(),
            },
            canary_scans,
            assertions: vec![AssertionEvidenceV1 {
                assertion_id: "scenario_outcome".to_string(),
                passed,
                failure_code: outcome_failure_code,
            }],
            quarantines: Vec::new(),
            required_cells: vec![cell_id.clone()],
            results: vec![CellResultV1 {
                cell_id,
                task: result.name.clone(),
                provider: result.provider.cli_name().to_string(),
                platform: result.platform.to_string(),
                passed,
                failures: failure_evidence,
                usability,
                wall_time_ms: result.wall_time.as_millis() as u64,
                cost_microusd,
            }],
            summary,
        };
        Self::local(body)
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

    pub fn parse_and_verify(
        &self,
        bytes: &[u8],
        policy: &VerificationPolicy,
    ) -> Result<(EvidenceReceiptV1, VerifiedReceipt), ReceiptError> {
        let checked: DuplicateCheckedValue = serde_json::from_slice(bytes)
            .map_err(|error| ReceiptError::InvalidJson(error.to_string()))?;
        let receipt = serde_json::from_value(checked.0)
            .map_err(|error| ReceiptError::InvalidJson(error.to_string()))?;
        let verified = self.verify(&receipt, policy)?;
        Ok((receipt, verified))
    }
}

struct DuplicateCheckedValue(serde_json::Value);

impl<'de> Deserialize<'de> for DuplicateCheckedValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(DuplicateCheckedVisitor)
    }
}

struct DuplicateCheckedVisitor;

impl<'de> Visitor<'de> for DuplicateCheckedVisitor {
    type Value = DuplicateCheckedValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(DuplicateCheckedValue(serde_json::Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(DuplicateCheckedValue(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(DuplicateCheckedValue(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .map(DuplicateCheckedValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(DuplicateCheckedValue(serde_json::Value::String(
            value.to_string(),
        )))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(DuplicateCheckedValue(serde_json::Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(DuplicateCheckedValue(serde_json::Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(DuplicateCheckedValue(serde_json::Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        DuplicateCheckedValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<DuplicateCheckedValue>()? {
            values.push(value.0);
        }
        Ok(DuplicateCheckedValue(serde_json::Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some((key, value)) = object.next_entry::<String, DuplicateCheckedValue>()? {
            if values.insert(key.clone(), value.0).is_some() {
                return Err(serde::de::Error::custom(format!(
                    "duplicate JSON object key: {key}"
                )));
            }
        }
        Ok(DuplicateCheckedValue(serde_json::Value::Object(values)))
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

fn behavior_digest(body: &ReceiptBodyV1) -> Result<String, ReceiptError> {
    let projection = BehaviorProjectionV1 {
        schema: BEHAVIOR_SCHEMA,
        schema_version: BEHAVIOR_SCHEMA_VERSION,
        identity: BehaviorIdentityV1 {
            source_commit: &body.identity.source_commit,
            binary_sha256: &body.identity.binary_sha256,
            config_sha256: &body.identity.config_sha256,
            fixture_sha256: &body.identity.fixture_sha256,
            provider: &body.identity.provider,
            model: &body.identity.model,
        },
        target: &body.target,
        policy: &body.policy,
        provider: &body.provider,
        tools: body
            .tools
            .iter()
            .map(|tool| BehaviorToolV1 {
                tool_name: &tool.tool_name,
                request_sha256: &tool.request_sha256,
                result_sha256: &tool.result_sha256,
                exit_state: &tool.exit_state,
                idempotency_key_sha256: &tool.idempotency_key_sha256,
            })
            .collect(),
        decisions: &body.decisions,
        boundaries: &body.boundaries,
        process: BehaviorProcessV1 {
            cancellation_requested: body.process.cancellation_requested,
            orphan_count: &body.process.orphan_count,
        },
        recovery: BehaviorRecoveryV1 {
            action: &body.recovery.action,
            unresolved_side_effects: &body.recovery.unresolved_side_effects,
        },
        canary_scans: &body.canary_scans,
        assertions: &body.assertions,
        quarantines: &body.quarantines,
        required_cells: &body.required_cells,
        results: body
            .results
            .iter()
            .map(|result| BehaviorResultV1 {
                cell_id: &result.cell_id,
                task: &result.task,
                provider: &result.provider,
                platform: &result.platform,
                passed: result.passed,
                failures: &result.failures,
                usability: &result.usability,
                cost_microusd: result.cost_microusd,
            })
            .collect(),
        summary: BehaviorSummaryV1 {
            passed: body.summary.passed,
            failed: body.summary.failed,
            total_cost_microusd: body.summary.total_cost_microusd,
        },
    };
    hash_serializable(&projection)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_serializable(value: &impl Serialize) -> Result<String, ReceiptError> {
    serde_json::to_vec(value)
        .map(|bytes| sha256(&bytes))
        .map_err(|error| ReceiptError::InvalidEvidence(format!("evidence serialization: {error}")))
}

fn usd_to_microusd(field: &str, value: f64) -> Result<u64, ReceiptError> {
    if !value.is_finite() || value < 0.0 || value > u64::MAX as f64 / 1_000_000.0 {
        return Err(ReceiptError::InvalidEvidence(format!(
            "{field} must be finite, non-negative, and representable"
        )));
    }
    Ok((value * 1_000_000.0).round() as u64)
}

fn failure_code(failure: &Failure) -> &'static str {
    match failure {
        Failure::OverTime { .. } => "over_time",
        Failure::OverCost { .. } => "over_cost",
        Failure::CostMissing => "cost_missing",
        Failure::Crashed { .. } => "crashed",
        Failure::Hung { .. } => "hung",
        Failure::ExpectedToolMissing(_) => "expected_tool_missing",
        Failure::ForbiddenToolUsed(_) => "forbidden_tool_used",
        Failure::AssertionFailed { .. } => "assertion_failed",
        Failure::TraceFailed { .. } => "trace_failed",
        Failure::StepsExceeded { .. } => "steps_exceeded",
        Failure::SessionBrick { .. } => "session_brick",
        Failure::SkippedInStrict { .. } => "skipped_in_strict",
        Failure::RunnerError(_) => "runner_error",
        Failure::SecretDetected { .. } => "secret_detected",
    }
}

fn validate_body(body: &ReceiptBodyV1) -> Result<(), ReceiptError> {
    require_nonempty("run_id", &body.run_id)?;
    require_sha256("identity.source_commit", &body.identity.source_commit, 40)?;
    require_sha256("identity.binary_sha256", &body.identity.binary_sha256, 64)?;
    require_sha256("identity.config_sha256", &body.identity.config_sha256, 64)?;
    require_sha256("identity.fixture_sha256", &body.identity.fixture_sha256, 64)?;
    require_nonempty("identity.provider", &body.identity.provider)?;
    require_nonempty("identity.model", &body.identity.model)?;
    match &body.identity.build {
        Evidence::Observed { value } => {
            require_nonempty("identity.build.repository", &value.repository)?;
            require_nonempty("identity.build.source_ref", &value.source_ref)?;
            require_nonempty("identity.build.workflow", &value.workflow)?;
            require_nonempty("identity.build.invocation_id", &value.invocation_id)?;
        }
        Evidence::Unavailable { code } => require_nonempty("identity.build.code", code)?,
    }
    require_nonempty("target.os", &body.target.os)?;
    require_nonempty("target.architecture", &body.target.architecture)?;
    require_nonempty("target.sandbox_backend", &body.target.sandbox_backend)?;
    require_nonempty("policy.posture", &body.policy.posture)?;
    require_sha256(
        "policy.effective_policy_sha256",
        &body.policy.effective_policy_sha256,
        64,
    )?;
    for (field, evidence) in [
        ("timings.boot_ms", &body.timings.boot_ms),
        ("timings.ready_ms", &body.timings.ready_ms),
        ("timings.prompt_ms", &body.timings.prompt_ms),
        ("timings.first_token_ms", &body.timings.first_token_ms),
        ("timings.tool_ms", &body.timings.tool_ms),
        ("timings.approval_ms", &body.timings.approval_ms),
        ("timings.completion_ms", &body.timings.completion_ms),
        ("timings.shutdown_ms", &body.timings.shutdown_ms),
    ] {
        validate_evidence(field, evidence)?;
    }
    for failure in &body.provider.typed_failures {
        require_nonempty("provider.typed_failures", failure)?;
    }
    validate_evidence("provider.attempts", &body.provider.attempts)?;
    validate_evidence("provider.retries", &body.provider.retries)?;
    for tool in &body.tools {
        require_sha256("tools.call_id_sha256", &tool.call_id_sha256, 64)?;
        require_nonempty("tools.tool_name", &tool.tool_name)?;
        require_sha256("tools.request_sha256", &tool.request_sha256, 64)?;
        require_sha256("tools.result_sha256", &tool.result_sha256, 64)?;
        validate_evidence("tools.duration_ms", &tool.duration_ms)?;
        require_nonempty("tools.exit_state", &tool.exit_state)?;
        validate_sha_evidence("tools.idempotency_key_sha256", &tool.idempotency_key_sha256)?;
    }
    if body.decisions.is_empty() {
        return Err(ReceiptError::InvalidEvidence(
            "decisions must contain the effective policy decision".to_string(),
        ));
    }
    for decision in &body.decisions {
        require_nonempty("decisions.actor", &decision.actor)?;
        require_nonempty("decisions.action", &decision.action)?;
        require_sha256("decisions.resource_sha256", &decision.resource_sha256, 64)?;
        require_nonempty("decisions.scope", &decision.scope)?;
        require_nonempty("decisions.decision", &decision.decision)?;
    }
    validate_evidence(
        "boundaries.egress_attempted",
        &body.boundaries.egress_attempted,
    )?;
    validate_evidence("boundaries.egress_allowed", &body.boundaries.egress_allowed)?;
    validate_evidence("boundaries.egress_denied", &body.boundaries.egress_denied)?;
    validate_evidence(
        "boundaries.filesystem_deltas",
        &body.boundaries.filesystem_deltas,
    )?;
    if let Evidence::Observed { value: deltas } = &body.boundaries.filesystem_deltas {
        for delta in deltas {
            require_sha256("filesystem.path_sha256", &delta.path_sha256, 64)?;
            require_nonempty("filesystem.operation", &delta.operation)?;
            validate_sha_evidence("filesystem.content_sha256", &delta.content_sha256)?;
        }
    }
    require_sha256("process.tree_sha256", &body.process.tree_sha256, 64)?;
    validate_evidence("process.peak_memory_bytes", &body.process.peak_memory_bytes)?;
    validate_evidence("process.peak_cpu_millis", &body.process.peak_cpu_millis)?;
    validate_evidence("process.orphan_count", &body.process.orphan_count)?;
    validate_sha_evidence(
        "recovery.journal_cursor_sha256",
        &body.recovery.journal_cursor_sha256,
    )?;
    require_nonempty("recovery.action", &body.recovery.action)?;
    if body.assertions.is_empty() {
        return Err(ReceiptError::InvalidEvidence(
            "assertions must not be empty".to_string(),
        ));
    }
    for assertion in &body.assertions {
        require_nonempty("assertions.assertion_id", &assertion.assertion_id)?;
        if !assertion.passed && assertion.failure_code.is_none() {
            return Err(ReceiptError::InvalidEvidence(format!(
                "failed assertion {} has no failure code",
                assertion.assertion_id
            )));
        }
    }
    for quarantine in &body.quarantines {
        require_nonempty("quarantines.assertion_id", &quarantine.assertion_id)?;
        require_nonempty("quarantines.owner", &quarantine.owner)?;
        require_nonempty("quarantines.expires_at", &quarantine.expires_at)?;
    }
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
        for failure in &result.failures {
            require_nonempty("result.failures.code", &failure.code)?;
            validate_sha_evidence("result.failures.detail_sha256", &failure.detail_sha256)?;
        }
        for finding in &result.usability {
            require_nonempty("result.usability.severity", &finding.severity)?;
            require_nonempty("result.usability.code", &finding.code)?;
            require_sha256(
                "result.usability.evidence_sha256",
                &finding.evidence_sha256,
                64,
            )?;
        }
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
        && matches!(&body.identity.build, Evidence::Observed { .. })
        && evidence_observed(&body.provider.attempts)
        && evidence_observed(&body.provider.retries)
        && evidence_observed(&body.provider.input_tokens)
        && evidence_observed(&body.provider.output_tokens)
        && evidence_observed(&body.provider.cache_read_tokens)
        && evidence_observed(&body.provider.cache_write_tokens)
        && evidence_observed(&body.boundaries.egress_attempted)
        && evidence_observed(&body.boundaries.egress_allowed)
        && evidence_observed(&body.boundaries.egress_denied)
        && evidence_observed(&body.boundaries.filesystem_deltas)
        && evidence_observed(&body.process.peak_memory_bytes)
        && evidence_observed(&body.process.peak_cpu_millis)
        && body.canary_scans.scan_complete
        && body.canary_scans.detections() == 0
        && matches!(body.process.orphan_count, Evidence::Observed { value: 0 })
        && body.recovery.unresolved_side_effects.is_empty()
        && body.assertions.iter().all(|assertion| assertion.passed)
}

fn evidence_observed<T>(evidence: &Evidence<T>) -> bool {
    matches!(evidence, Evidence::Observed { .. })
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

fn validate_evidence<T>(field: &str, evidence: &Evidence<T>) -> Result<(), ReceiptError> {
    if let Evidence::Unavailable { code } = evidence {
        require_nonempty(&format!("{field}.code"), code)?;
    }
    Ok(())
}

fn validate_sha_evidence(field: &str, evidence: &Evidence<String>) -> Result<(), ReceiptError> {
    match evidence {
        Evidence::Observed { value } => require_sha256(field, value, 64),
        Evidence::Unavailable { .. } => validate_evidence(field, evidence),
    }
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
