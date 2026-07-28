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
use crate::workspace_evidence;

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
    /// Domain-separated semantic hashes. Only the evaluator-owned absolute
    /// workspace root is erased; path suffixes and all outside data remain.
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
    /// Coverage of the egress fields below. This is deliberately narrower
    /// than whole-process network egress: it covers only Core-managed HTTP
    /// clients that pass through `wcore-egress`.
    pub egress_scope: String,
    pub egress_attempted: Evidence<Vec<String>>,
    pub egress_allowed: Evidence<Vec<String>>,
    pub egress_denied: Evidence<Vec<String>>,
    pub filesystem_deltas: Evidence<Vec<FilesystemDeltaV1>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemDeltaV1 {
    pub scope: String,
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
    boundaries: BehaviorBoundaryV1<'a>,
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
struct BehaviorBoundaryV1<'a> {
    egress_scope: &'a str,
    egress_attempted: &'a Evidence<Vec<String>>,
    egress_allowed: &'a Evidence<Vec<String>>,
    egress_denied: &'a Evidence<Vec<String>>,
    /// Engine state is still present in the full receipt, but session IDs,
    /// logs, and traces are run evidence rather than a repeatable user-visible
    /// filesystem outcome.
    workspace_filesystem_deltas: Evidence<Vec<&'a FilesystemDeltaV1>>,
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
        let provider_typed_failures = result.execution.provider_typed_failures.clone();
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
            .map(|entry| {
                let request_sha256 = workspace_evidence::semantic_sha256(
                    b"tool-request",
                    entry.input.as_bytes(),
                    &result.workdir,
                )
                .map_err(|error| ReceiptError::InvalidEvidence(error.to_string()))?;
                let result_sha256 = workspace_evidence::semantic_sha256(
                    b"tool-result",
                    entry.output.as_bytes(),
                    &result.workdir,
                )
                .map_err(|error| ReceiptError::InvalidEvidence(error.to_string()))?;
                Ok(ToolEvidenceV1 {
                    call_id_sha256: sha256(entry.call_id.as_bytes()),
                    tool_name: entry.tool_name.clone(),
                    request_sha256,
                    result_sha256,
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
            })
            .collect::<Result<Vec<_>, ReceiptError>>()?;
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
        let process_orphans =
            if result.execution.cleanup_verified && result.execution.containment_authoritative {
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
                attempts: result.execution.provider_attempts.map_or_else(
                    || Evidence::Unavailable {
                        code: "provider_attempts_not_emitted".to_string(),
                    },
                    Evidence::observed,
                ),
                typed_failures: provider_typed_failures,
                retries: result.execution.provider_retries.map_or_else(
                    || Evidence::Unavailable {
                        code: "provider_retries_not_emitted".to_string(),
                    },
                    Evidence::observed,
                ),
                input_tokens: result.execution.provider_usage.as_ref().map_or_else(
                    || Evidence::Unavailable {
                        code: "provider_usage_not_emitted".to_string(),
                    },
                    |usage| Evidence::observed(usage.input_tokens),
                ),
                output_tokens: result.execution.provider_usage.as_ref().map_or_else(
                    || Evidence::Unavailable {
                        code: "provider_usage_not_emitted".to_string(),
                    },
                    |usage| Evidence::observed(usage.output_tokens),
                ),
                cache_read_tokens: result.execution.provider_usage.as_ref().map_or_else(
                    || Evidence::Unavailable {
                        code: "provider_usage_not_emitted".to_string(),
                    },
                    |usage| Evidence::observed(usage.cache_read_tokens),
                ),
                cache_write_tokens: result.execution.provider_usage.as_ref().map_or_else(
                    || Evidence::Unavailable {
                        code: "provider_usage_not_emitted".to_string(),
                    },
                    |usage| Evidence::observed(usage.cache_write_tokens),
                ),
                cost_microusd,
                limit_microusd,
            },
            tools,
            decisions,
            boundaries: BoundaryEvidenceV1 {
                egress_scope: "core_managed_http_v1".to_string(),
                egress_attempted: result.execution.managed_http_egress.as_ref().map_or_else(
                    || Evidence::Unavailable {
                        code: "managed_http_egress_recorder_incomplete".to_string(),
                    },
                    |egress| Evidence::observed(egress.attempted.clone()),
                ),
                egress_allowed: result.execution.managed_http_egress.as_ref().map_or_else(
                    || Evidence::Unavailable {
                        code: "managed_http_egress_recorder_incomplete".to_string(),
                    },
                    |egress| Evidence::observed(egress.allowed.clone()),
                ),
                egress_denied: result.execution.managed_http_egress.as_ref().map_or_else(
                    || Evidence::Unavailable {
                        code: "managed_http_egress_recorder_incomplete".to_string(),
                    },
                    |egress| Evidence::observed(egress.denied.clone()),
                ),
                filesystem_deltas: match (
                    result.execution.filesystem_snapshot_complete,
                    result.execution.filesystem_deltas.as_ref(),
                ) {
                    (true, Some(deltas)) => Evidence::observed(
                        deltas
                            .iter()
                            .map(|delta| FilesystemDeltaV1 {
                                scope: delta.scope.clone(),
                                path_sha256: delta.path_sha256.clone(),
                                operation: delta.operation.clone(),
                                content_sha256: delta.content_sha256.clone().map_or_else(
                                    || Evidence::Unavailable {
                                        code: "file_deleted".to_string(),
                                    },
                                    Evidence::observed,
                                ),
                            })
                            .collect(),
                    ),
                    _ => Evidence::Unavailable {
                        code: "filesystem_delta_recorder_incomplete".to_string(),
                    },
                },
            },
            process: ProcessEvidenceV1 {
                tree_sha256: result.execution.process_tree_sha256.clone(),
                peak_memory_bytes: result.execution.peak_memory_bytes.map_or_else(
                    || Evidence::Unavailable {
                        code: "resource_sampler_not_enabled".to_string(),
                    },
                    Evidence::observed,
                ),
                peak_cpu_millis: result.execution.peak_cpu_millis.map_or_else(
                    || Evidence::Unavailable {
                        code: "resource_sampler_not_enabled".to_string(),
                    },
                    Evidence::observed,
                ),
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
        boundaries: BehaviorBoundaryV1 {
            egress_scope: &body.boundaries.egress_scope,
            egress_attempted: &body.boundaries.egress_attempted,
            egress_allowed: &body.boundaries.egress_allowed,
            egress_denied: &body.boundaries.egress_denied,
            workspace_filesystem_deltas: match &body.boundaries.filesystem_deltas {
                Evidence::Observed { value } => Evidence::observed(
                    value
                        .iter()
                        .filter(|delta| delta.scope == "workspace")
                        .collect(),
                ),
                Evidence::Unavailable { code } => Evidence::Unavailable { code: code.clone() },
            },
        },
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
    if body.boundaries.egress_scope != "core_managed_http_v1" {
        return Err(ReceiptError::InvalidEvidence(format!(
            "boundaries.egress_scope has unsupported value {}",
            body.boundaries.egress_scope
        )));
    }
    validate_evidence(
        "boundaries.egress_attempted",
        &body.boundaries.egress_attempted,
    )?;
    validate_evidence("boundaries.egress_allowed", &body.boundaries.egress_allowed)?;
    validate_evidence("boundaries.egress_denied", &body.boundaries.egress_denied)?;
    validate_managed_http_egress(&body.boundaries)?;
    validate_evidence(
        "boundaries.filesystem_deltas",
        &body.boundaries.filesystem_deltas,
    )?;
    if let Evidence::Observed { value: deltas } = &body.boundaries.filesystem_deltas {
        for delta in deltas {
            if !matches!(delta.scope.as_str(), "workspace" | "engine_state") {
                return Err(ReceiptError::InvalidEvidence(format!(
                    "filesystem.scope has unsupported value {}",
                    delta.scope
                )));
            }
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

fn validate_managed_http_egress(boundaries: &BoundaryEvidenceV1) -> Result<(), ReceiptError> {
    for (field, evidence) in [
        ("attempted", &boundaries.egress_attempted),
        ("allowed", &boundaries.egress_allowed),
        ("denied", &boundaries.egress_denied),
    ] {
        if let Evidence::Observed { value } = evidence {
            for fingerprint in value {
                let digest = fingerprint
                    .strip_prefix("managed_http:v1:")
                    .ok_or_else(|| {
                        ReceiptError::InvalidEvidence(format!(
                            "boundaries.egress_{field} contains an unsupported fingerprint"
                        ))
                    })?;
                if digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(ReceiptError::InvalidEvidence(format!(
                        "boundaries.egress_{field} contains an invalid fingerprint digest"
                    )));
                }
            }
        }
    }

    let (
        Evidence::Observed { value: attempted },
        Evidence::Observed { value: allowed },
        Evidence::Observed { value: denied },
    ) = (
        &boundaries.egress_attempted,
        &boundaries.egress_allowed,
        &boundaries.egress_denied,
    )
    else {
        return Ok(());
    };
    let counts = |values: &[String]| {
        let mut counts = BTreeMap::<String, usize>::new();
        for value in values {
            *counts.entry(value.clone()).or_default() += 1;
        }
        counts
    };
    let mut decided = allowed.clone();
    decided.extend(denied.iter().cloned());
    if counts(attempted) != counts(&decided) {
        return Err(ReceiptError::InvalidEvidence(
            "managed HTTP egress attempts are not exactly partitioned into allowed and denied"
                .to_string(),
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
    milestone_evidence_gaps(body).is_empty()
}

/// Return every field that prevents this receipt from satisfying the release
/// evidence gate. A signer may attest incomplete evidence, but release tooling
/// must fail closed with this exact list instead of a generic rejection.
pub fn milestone_evidence_gaps(body: &ReceiptBodyV1) -> Vec<&'static str> {
    let mut gaps = Vec::new();
    if !body.results.iter().all(|result| result.passed) {
        gaps.push("results.passed");
    }
    if !matches!(&body.identity.build, Evidence::Observed { .. }) {
        gaps.push("identity.build");
    }
    push_unobserved(&mut gaps, "provider.attempts", &body.provider.attempts);
    push_unobserved(&mut gaps, "provider.retries", &body.provider.retries);
    push_unobserved(
        &mut gaps,
        "provider.input_tokens",
        &body.provider.input_tokens,
    );
    push_unobserved(
        &mut gaps,
        "provider.output_tokens",
        &body.provider.output_tokens,
    );
    push_unobserved(
        &mut gaps,
        "provider.cache_read_tokens",
        &body.provider.cache_read_tokens,
    );
    push_unobserved(
        &mut gaps,
        "provider.cache_write_tokens",
        &body.provider.cache_write_tokens,
    );
    push_unobserved(
        &mut gaps,
        "boundaries.egress_attempted",
        &body.boundaries.egress_attempted,
    );
    push_unobserved(
        &mut gaps,
        "boundaries.egress_allowed",
        &body.boundaries.egress_allowed,
    );
    push_unobserved(
        &mut gaps,
        "boundaries.egress_denied",
        &body.boundaries.egress_denied,
    );
    push_unobserved(
        &mut gaps,
        "boundaries.filesystem_deltas",
        &body.boundaries.filesystem_deltas,
    );
    push_unobserved(
        &mut gaps,
        "process.peak_memory_bytes",
        &body.process.peak_memory_bytes,
    );
    push_unobserved(
        &mut gaps,
        "process.peak_cpu_millis",
        &body.process.peak_cpu_millis,
    );
    if !body.canary_scans.scan_complete {
        gaps.push("canary_scans.scan_complete");
    }
    if body.canary_scans.detections() != 0 {
        gaps.push("canary_scans.detections");
    }
    if !matches!(body.process.orphan_count, Evidence::Observed { value: 0 }) {
        gaps.push("process.orphan_count");
    }
    if !body.recovery.unresolved_side_effects.is_empty() {
        gaps.push("recovery.unresolved_side_effects");
    }
    if !body.assertions.iter().all(|assertion| assertion.passed) {
        gaps.push("assertions.passed");
    }
    gaps
}

fn push_unobserved<T>(gaps: &mut Vec<&'static str>, field: &'static str, value: &Evidence<T>) {
    if !evidence_observed(value) {
        gaps.push(field);
    }
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

// ===========================================================================================
// Phase 28 certification receipt (F28-03) — schema v2.
//
// This EXTENDS the v1 evidence receipt above rather than replacing it. The one design property
// of the v1 receipt that most needs to survive is preserved verbatim here: authority is derived
// by a verifier from a detached signature against an EXTERNALLY configured trusted key, never
// from a flag inside the body. A `CertificationReceiptV2` you hold is not authoritative because
// it says so; it is authoritative because a verifier you configured trusts the key that signed
// its body digest.
//
// The v1 receipt binds ONE scenario run. A Phase 28 certification binds a whole phase: a
// matrix, a soak, an observability control and a candidate resolution, across three OS
// families and (as it turned out) two candidates. Four of the eight F28-03 bindings already
// have v1 analogues — candidate (source commit + binary digest), platform (target evidence),
// posture (policy evidence) and fixture corpus (corpus digest). The four that did not exist
// are environment, artifacts, logs and the skipped-case policy, and they are added here
// alongside the finding ledger as a first-class section.
//
// SCHEMA VERSIONING IS FAIL-CLOSED. The schema string and version are checked before anything
// else, and every struct is `deny_unknown_fields`. A reader compiled against v1 sees an
// unknown schema and errors; a reader compiled against v2 that meets a v3 section errors
// rather than silently ignoring it. There is no "ignore what you do not understand" path.
//
// AMENDMENT A3 IS ENFORCED HERE, NOT IN A COMMENT. The receipt may assert exactly three
// things. A receipt asserting "zero known defects" or "zero findings" is REJECTED with its own
// failure code, and `tests/f28_receipt_contract.rs` trips that code with a fixture.
// ===========================================================================================

pub const CERT_RECEIPT_SCHEMA: &str = "wayland.cert.receipt";
pub const CERT_RECEIPT_SCHEMA_VERSION: u32 = 2;
const CERT_SIGNATURE_DOMAIN: &[u8] = b"wayland.cert.receipt.v2\0";

/// The ONLY three claims a Phase 28 certification receipt may assert (amendment A3).
///
/// Every one of these is a statement about the LEDGER, not about the product. None of them
/// says "there are no defects". That distinction is the whole of A3 and the reason this
/// constant is an allowlist rather than a comment.
pub const CERT_PERMITTED_CLAIMS: [&str; 3] = [
    "zero_undispositioned_findings",
    "zero_skipped_critical_cases",
    "zero_unresolved_critical_or_high",
];

/// The four skip classes fixed by `28-01-CERTIFICATION-CONTRACT.md` §4. There is no fifth,
/// and a receipt naming one is rejected — inventing a class is how a skip becomes a pass.
pub const CERT_SKIP_CLASSES: [&str; 4] = [
    "platform-inapplicability",
    "observation-blocked",
    "architectural-impossibility",
    "unresolved-surface",
];

pub const CERT_SEVERITIES: [&str; 4] = ["CRITICAL", "HIGH", "MEDIUM", "LOW"];
/// The four TERMINAL dispositions. A ledger in which every finding carries one of these is
/// what "zero undispositioned findings" means.
pub const CERT_TERMINAL_DISPOSITIONS: [&str; 4] = ["FIXED", "DISPROVED", "ACCEPTED", "DEFERRED"];
/// The dispositions a receipt may RECORD. `OPEN` is deliberately included and is deliberately
/// NOT terminal.
///
/// This is the single most important schema decision in the file. A receipt that could not
/// represent an unresolved finding would make the honest outcome UNSAYABLE, and an executor
/// facing a CRITICAL or HIGH it can neither fix nor disprove would be forced to launder it
/// into a terminal disposition to produce a valid artifact at all. So `OPEN` is legal to
/// record, and recording one makes `zero_undispositioned_findings` false — and, at CRITICAL
/// or HIGH, `zero_unresolved_critical_or_high` false too. The receipt then states its own
/// gate as not passed, which is a PASSING state for the artifact and an honest one for the
/// phase. A reported red is worth far more than an engineered green.
pub const CERT_DISPOSITIONS: [&str; 5] = ["FIXED", "DISPROVED", "ACCEPTED", "DEFERRED", "OPEN"];
const CERT_PAPER_DISPOSITIONS: [&str; 2] = ["ACCEPTED", "DEFERRED"];
const CERT_BLOCKING_SEVERITIES: [&str; 2] = ["CRITICAL", "HIGH"];

/// Every rejection carries a DISTINCT machine-readable code. A generic failure would let one
/// rule silently stop working while the gate stayed green — which is the exact defect class
/// this whole phase exists to catch.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CertError {
    #[error("F28R-SCHEMA: unsupported certification schema {schema} version {version}")]
    UnsupportedSchema { schema: String, version: u32 },
    #[error("F28R-JSON: invalid certification receipt JSON: {0}")]
    InvalidJson(String),
    #[error("F28R-DIGEST: certification body digest does not match its body")]
    DigestMismatch,
    #[error("F28R-B{index:02}: binding {binding} is missing or empty")]
    MissingBinding { index: u8, binding: &'static str },
    #[error("F28R-FIELD: {0}")]
    InvalidField(String),
    #[error(
        "F28R-SKIPCLASS: skipped cell {cell} names class {class:?}, which is not one of the four contract classes"
    )]
    IllegalSkipClass { cell: String, class: String },
    #[error(
        "F28R-SKIPEVID: skipped cell {cell} carries no required-evidence reference for its class"
    )]
    SkipWithoutEvidence { cell: String },
    #[error(
        "F28R-SKIPCRIT: cell {cell} is a skipped CRITICAL case; Success Criterion 1 forbids one"
    )]
    SkippedCriticalCase { cell: String },
    #[error("F28R-NODISP: finding {id} carries disposition {disposition:?}, which is not terminal")]
    UndispositionedFinding { id: String, disposition: String },
    #[error("F28R-SEV: finding {id} carries unknown Phase 28 severity {severity:?}")]
    UnknownSeverity { id: String, severity: String },
    #[error(
        "F28R-PROV: finding {id} records no inherited severity; A1 requires it as provenance ('-' for none)"
    )]
    MissingProvenance { id: String },
    #[error(
        "F28R-PAPERSEV: finding {id} is {disposition} at {severity}; CRITICAL and HIGH have exactly two dispositions, FIXED or DISPROVED"
    )]
    PaperPathAtBlockingSeverity {
        id: String,
        disposition: String,
        severity: String,
    },
    #[error(
        "F28R-PAPERA2: finding {id} is {disposition} while contradicting Success Criterion {criterion}; amendment A2 closes the accept and defer paths regardless of recorded severity"
    )]
    PaperPathWithContradictedCriterion {
        id: String,
        disposition: String,
        criterion: String,
    },
    #[error("F28R-PAPEREVID: finding {id} is {disposition} without {missing}")]
    PaperPathMissingEvidence {
        id: String,
        disposition: String,
        missing: &'static str,
    },
    #[error("F28R-REPAIREVID: finding {id} is {disposition} without {missing}")]
    RepairPathMissingEvidence {
        id: String,
        disposition: String,
        missing: &'static str,
    },
    #[error(
        "F28R-OVERCLAIM: the receipt asserts {claim:?}, which is outside the three claims amendment A3 permits"
    )]
    OverClaim { claim: String },
    #[error(
        "F28R-CLAIMMISS: the receipt does not state claim {claim:?}; all three must be stated, true or false"
    )]
    MissingClaim { claim: &'static str },
    #[error(
        "F28R-CLAIMFALSE: claim {claim:?} is asserted true but the ledger contradicts it: {detail}"
    )]
    ClaimContradictsLedger { claim: &'static str, detail: String },
    #[error(
        "F28R-UNSIGNED: an authoritative certification receipt carries no phase-scoped signature"
    )]
    Unsigned,
    #[error("F28R-KEY: signing key {0} is not configured as trusted by this verifier")]
    UntrustedKey(String),
    #[error("F28R-SIGFORM: the phase-scoped signature is malformed")]
    MalformedSignature,
    #[error("F28R-SIG: the phase-scoped signature does not verify against the body digest")]
    InvalidSignature,
    #[error(
        "F28R-FINGERPRINT: the recorded key fingerprint does not match the recorded public key"
    )]
    FingerprintMismatch,
}

impl CertError {
    /// The stable machine-readable code, split off the Display text so a gate can match on it.
    pub fn code(&self) -> String {
        match self {
            Self::UnsupportedSchema { .. } => "F28R-SCHEMA".to_string(),
            Self::InvalidJson(_) => "F28R-JSON".to_string(),
            Self::DigestMismatch => "F28R-DIGEST".to_string(),
            Self::MissingBinding { index, .. } => format!("F28R-B{index:02}"),
            Self::InvalidField(_) => "F28R-FIELD".to_string(),
            Self::IllegalSkipClass { .. } => "F28R-SKIPCLASS".to_string(),
            Self::SkipWithoutEvidence { .. } => "F28R-SKIPEVID".to_string(),
            Self::SkippedCriticalCase { .. } => "F28R-SKIPCRIT".to_string(),
            Self::UndispositionedFinding { .. } => "F28R-NODISP".to_string(),
            Self::UnknownSeverity { .. } => "F28R-SEV".to_string(),
            Self::MissingProvenance { .. } => "F28R-PROV".to_string(),
            Self::PaperPathAtBlockingSeverity { .. } => "F28R-PAPERSEV".to_string(),
            Self::PaperPathWithContradictedCriterion { .. } => "F28R-PAPERA2".to_string(),
            Self::PaperPathMissingEvidence { .. } => "F28R-PAPEREVID".to_string(),
            Self::RepairPathMissingEvidence { .. } => "F28R-REPAIREVID".to_string(),
            Self::OverClaim { .. } => "F28R-OVERCLAIM".to_string(),
            Self::MissingClaim { .. } => "F28R-CLAIMMISS".to_string(),
            Self::ClaimContradictsLedger { .. } => "F28R-CLAIMFALSE".to_string(),
            Self::Unsigned => "F28R-UNSIGNED".to_string(),
            Self::UntrustedKey(_) => "F28R-KEY".to_string(),
            Self::MalformedSignature => "F28R-SIGFORM".to_string(),
            Self::InvalidSignature => "F28R-SIG".to_string(),
            Self::FingerprintMismatch => "F28R-FINGERPRINT".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CertificationReceiptV2 {
    pub schema: String,
    pub schema_version: u32,
    pub body_sha256: String,
    pub body: CertificationBodyV2,
    pub authority: CertAuthorityClaimV2,
}

/// Authority is a CLAIM in the body's sibling, never in the body, and it is only ever
/// upgraded to authoritative by a verifier holding the key out of band.
///
/// `PhaseScoped` is named for what it is. It is NOT a release trust root, it is NOT a seal,
/// and it does not authorise anything. It says: this evidence was assembled by this
/// certification run and has not been altered since.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CertAuthorityClaimV2 {
    Unsigned,
    PhaseScoped {
        key_id: String,
        /// Recorded so a later reader can check the signature without the minting run.
        public_key_base64: String,
        fingerprint_sha256: String,
        signature_base64: String,
        /// Free text, and it is checked by the verdict rather than by this crate. Present so
        /// the artifact itself states its own scope to a reader who never sees the verdict.
        scope: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CertificationBodyV2 {
    pub certification_id: String,
    pub phase: String,
    pub bindings: CertBindingsV2,
    pub findings: Vec<CertFindingV2>,
    /// Exactly the three keys in [`CERT_PERMITTED_CLAIMS`], each with its measured value.
    /// A fourth key is `F28R-OVERCLAIM`; a missing one is `F28R-CLAIMMISS`.
    pub claims: BTreeMap<String, bool>,
}

/// The eight F28-03 bindings, one field each, in the order the requirement states them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CertBindingsV2 {
    pub candidate: Vec<CandidateBindingV2>,
    pub platform: Vec<PlatformBindingV2>,
    pub posture: Vec<PostureBindingV2>,
    pub fixture_corpus: Vec<CorpusBindingV2>,
    pub environment: Vec<EnvironmentBindingV2>,
    pub artifacts: Vec<ArtifactBindingV2>,
    pub logs: Vec<LogBindingV2>,
    pub skip_policy: SkipPolicyV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateBindingV2 {
    /// Which evidence family this candidate is the candidate FOR. A phase that measured two
    /// candidates says so here rather than picking one and calling it "the" candidate.
    pub scope: String,
    pub commit: String,
    pub tree: String,
    pub ledger_ref: String,
    pub binaries: Vec<CandidateBinaryV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateBinaryV2 {
    pub target: String,
    pub sha256: String,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlatformBindingV2 {
    pub os_family: String,
    pub target: String,
    pub cells_total: u64,
    pub cells_pass: u64,
    pub cells_red: u64,
    pub cells_skipped: u64,
    pub critical_cells: u64,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PostureBindingV2 {
    pub name: String,
    pub description: String,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CorpusBindingV2 {
    pub name: String,
    pub sha256: String,
    pub item_count: u64,
    pub source_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentBindingV2 {
    pub host: String,
    pub os_family: String,
    pub os_build: String,
    pub run_context: String,
    pub evidence_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactBindingV2 {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogBindingV2 {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub produced_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkipPolicyV2 {
    /// The four legal classes, restated in the receipt so a reader needs no second document.
    pub classes: Vec<String>,
    pub skipped_cells: Vec<SkippedCellV2>,
    pub skipped_critical_cases: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkippedCellV2 {
    pub cell_id: String,
    pub class: String,
    pub criticality: String,
    pub required_evidence: String,
}

/// One adjudicated finding. `inherited_severity` is PROVENANCE ONLY and is never read by a
/// rule below; `p28_severity` is the operative value. That asymmetry is amendment A1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CertFindingV2 {
    pub id: String,
    pub origin: String,
    pub subject: String,
    pub inherited_severity: String,
    pub p28_severity: String,
    /// `"-"` for none. An EMPTY string is rejected, so an omission cannot read as a none.
    pub contradicted_criterion: String,
    pub disposition: String,
    pub rationale: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub backlog_id: String,
    #[serde(default)]
    pub executable_check: String,
    #[serde(default)]
    pub counter_evidence: String,
}

impl CertFindingV2 {
    fn contradicts(&self) -> bool {
        !self.contradicted_criterion.is_empty() && self.contradicted_criterion != "-"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertAuthority {
    /// Structurally valid, but nobody this verifier trusts vouches for it.
    UnverifiedProvenance,
    /// A key this verifier was configured with signed this exact body digest.
    PhaseScopedSigned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCertification {
    pub authority: CertAuthority,
    /// True only when all three A3 claims are asserted true AND recomputation from the
    /// receipt's own ledger agrees. This is NOT "no defects" — see [`CERT_PERMITTED_CLAIMS`].
    pub acceptance_gate_passed: bool,
}

#[derive(Debug, Default)]
pub struct CertificationVerifier {
    trusted_phase_keys: BTreeMap<String, VerifyingKey>,
}

impl CertificationReceiptV2 {
    /// Build an unsigned receipt. Validation runs here, so a structurally illegal receipt
    /// cannot be constructed and then signed.
    pub fn unsigned(body: CertificationBodyV2) -> Result<Self, CertError> {
        validate_cert_body(&body)?;
        Ok(Self {
            schema: CERT_RECEIPT_SCHEMA.to_string(),
            schema_version: CERT_RECEIPT_SCHEMA_VERSION,
            body_sha256: cert_body_digest(&body)?,
            body,
            authority: CertAuthorityClaimV2::Unsigned,
        })
    }

    /// Attach a detached PHASE-SCOPED signature. Possession of this object never makes it
    /// authoritative: a verifier must be configured with `key_id` out of band. This is the
    /// v1 property carried forward unchanged, and it is the reason the signature binds
    /// authorship rather than truth.
    pub fn sign_phase_scoped(
        mut self,
        key_id: impl Into<String>,
        key: &SigningKey,
        scope: impl Into<String>,
    ) -> Self {
        let signature = key.sign(&cert_signature_message(&self.body_sha256));
        let verifying = key.verifying_key();
        self.authority = CertAuthorityClaimV2::PhaseScoped {
            key_id: key_id.into(),
            public_key_base64: BASE64.encode(verifying.to_bytes()),
            fingerprint_sha256: sha256(verifying.as_bytes()),
            signature_base64: BASE64.encode(signature.to_bytes()),
            scope: scope.into(),
        };
        self
    }

    pub fn to_json(&self) -> Result<String, CertError> {
        serde_json::to_string_pretty(self).map_err(|e| CertError::InvalidJson(e.to_string()))
    }
}

impl CertificationVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trust_phase_key(&mut self, key_id: impl Into<String>, key: VerifyingKey) {
        self.trusted_phase_keys.insert(key_id.into(), key);
    }

    pub fn verify(
        &self,
        receipt: &CertificationReceiptV2,
    ) -> Result<VerifiedCertification, CertError> {
        // Schema first, and fail closed. An older reader never reaches the body.
        if receipt.schema != CERT_RECEIPT_SCHEMA
            || receipt.schema_version != CERT_RECEIPT_SCHEMA_VERSION
        {
            return Err(CertError::UnsupportedSchema {
                schema: receipt.schema.clone(),
                version: receipt.schema_version,
            });
        }
        validate_cert_body(&receipt.body)?;
        if cert_body_digest(&receipt.body)? != receipt.body_sha256 {
            return Err(CertError::DigestMismatch);
        }

        let gate = cert_acceptance_gate(&receipt.body);
        match &receipt.authority {
            CertAuthorityClaimV2::Unsigned => Ok(VerifiedCertification {
                authority: CertAuthority::UnverifiedProvenance,
                acceptance_gate_passed: gate,
            }),
            CertAuthorityClaimV2::PhaseScoped {
                key_id,
                public_key_base64,
                fingerprint_sha256,
                signature_base64,
                ..
            } => {
                let key = self
                    .trusted_phase_keys
                    .get(key_id)
                    .ok_or_else(|| CertError::UntrustedKey(key_id.clone()))?;
                // The recorded public half must be the trusted key AND must match the recorded
                // fingerprint. A receipt that records one key and is signed by another is
                // exactly the confusion a later reader would inherit.
                let recorded = BASE64
                    .decode(public_key_base64)
                    .map_err(|_| CertError::MalformedSignature)?;
                if recorded.as_slice() != key.to_bytes().as_slice() {
                    return Err(CertError::UntrustedKey(key_id.clone()));
                }
                if sha256(&recorded) != *fingerprint_sha256 {
                    return Err(CertError::FingerprintMismatch);
                }
                let signature_bytes = BASE64
                    .decode(signature_base64)
                    .map_err(|_| CertError::MalformedSignature)?;
                let signature = Signature::from_slice(&signature_bytes)
                    .map_err(|_| CertError::MalformedSignature)?;
                key.verify(&cert_signature_message(&receipt.body_sha256), &signature)
                    .map_err(|_| CertError::InvalidSignature)?;
                Ok(VerifiedCertification {
                    authority: CertAuthority::PhaseScopedSigned,
                    acceptance_gate_passed: gate,
                })
            }
        }
    }

    pub fn parse_and_verify(
        &self,
        bytes: &[u8],
    ) -> Result<(CertificationReceiptV2, VerifiedCertification), CertError> {
        let checked: DuplicateCheckedValue =
            serde_json::from_slice(bytes).map_err(|e| CertError::InvalidJson(e.to_string()))?;
        let receipt: CertificationReceiptV2 =
            serde_json::from_value(checked.0).map_err(|e| CertError::InvalidJson(e.to_string()))?;
        let verified = self.verify(&receipt)?;
        Ok((receipt, verified))
    }
}

fn cert_signature_message(body_sha256: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(CERT_SIGNATURE_DOMAIN.len() + body_sha256.len());
    message.extend_from_slice(CERT_SIGNATURE_DOMAIN);
    message.extend_from_slice(body_sha256.as_bytes());
    message
}

fn cert_body_digest(body: &CertificationBodyV2) -> Result<String, CertError> {
    let bytes = serde_json::to_vec(body)
        .map_err(|e| CertError::InvalidField(format!("canonical JSON: {e}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// True only when every one of the three A3 claims is asserted true. Deliberately NOT
/// "no findings" — see [`CERT_PERMITTED_CLAIMS`]. `validate_cert_body` has already proved
/// each asserted-true claim against the ledger, so this cannot be true over a ledger that
/// contradicts it.
fn cert_acceptance_gate(body: &CertificationBodyV2) -> bool {
    CERT_PERMITTED_CLAIMS
        .iter()
        .all(|claim| body.claims.get(*claim).copied().unwrap_or(false))
}

fn cert_require_nonempty(field: &str, value: &str) -> Result<(), CertError> {
    if value.trim().is_empty() {
        return Err(CertError::InvalidField(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn validate_cert_body(body: &CertificationBodyV2) -> Result<(), CertError> {
    cert_require_nonempty("certification_id", &body.certification_id)?;
    cert_require_nonempty("phase", &body.phase)?;
    validate_cert_bindings(&body.bindings)?;
    validate_cert_findings(&body.findings)?;
    validate_cert_claims(body)?;
    Ok(())
}

fn validate_cert_bindings(b: &CertBindingsV2) -> Result<(), CertError> {
    // Each binding gets its OWN failure code, so a gate can prove which one is missing rather
    // than reporting "a binding is missing" and letting the reader guess.
    if b.candidate.is_empty() {
        return Err(CertError::MissingBinding {
            index: 1,
            binding: "candidate",
        });
    }
    for c in &b.candidate {
        cert_require_nonempty("candidate.scope", &c.scope)?;
        require_cert_sha("candidate.commit", &c.commit, 40)?;
        require_cert_sha("candidate.tree", &c.tree, 40)?;
        cert_require_nonempty("candidate.ledger_ref", &c.ledger_ref)?;
        if c.binaries.is_empty() {
            return Err(CertError::MissingBinding {
                index: 1,
                binding: "candidate.binaries",
            });
        }
        for bin in &c.binaries {
            cert_require_nonempty("candidate.binaries.target", &bin.target)?;
            require_cert_sha("candidate.binaries.sha256", &bin.sha256, 64)?;
            cert_require_nonempty("candidate.binaries.provenance", &bin.provenance)?;
        }
    }
    if b.platform.is_empty() {
        return Err(CertError::MissingBinding {
            index: 2,
            binding: "platform",
        });
    }
    for p in &b.platform {
        cert_require_nonempty("platform.os_family", &p.os_family)?;
        cert_require_nonempty("platform.target", &p.target)?;
        cert_require_nonempty("platform.evidence_ref", &p.evidence_ref)?;
        if p.cells_pass + p.cells_red + p.cells_skipped != p.cells_total {
            return Err(CertError::InvalidField(format!(
                "platform.{} cell counts do not sum to cells_total",
                p.os_family
            )));
        }
    }
    if b.posture.is_empty() {
        return Err(CertError::MissingBinding {
            index: 3,
            binding: "posture",
        });
    }
    for p in &b.posture {
        cert_require_nonempty("posture.name", &p.name)?;
        cert_require_nonempty("posture.description", &p.description)?;
        cert_require_nonempty("posture.evidence_ref", &p.evidence_ref)?;
    }
    if b.fixture_corpus.is_empty() {
        return Err(CertError::MissingBinding {
            index: 4,
            binding: "fixture_corpus",
        });
    }
    for c in &b.fixture_corpus {
        cert_require_nonempty("fixture_corpus.name", &c.name)?;
        require_cert_sha("fixture_corpus.sha256", &c.sha256, 64)?;
        cert_require_nonempty("fixture_corpus.source_ref", &c.source_ref)?;
    }
    if b.environment.is_empty() {
        return Err(CertError::MissingBinding {
            index: 5,
            binding: "environment",
        });
    }
    for e in &b.environment {
        cert_require_nonempty("environment.host", &e.host)?;
        cert_require_nonempty("environment.os_family", &e.os_family)?;
        cert_require_nonempty("environment.os_build", &e.os_build)?;
        cert_require_nonempty("environment.run_context", &e.run_context)?;
        cert_require_nonempty("environment.evidence_ref", &e.evidence_ref)?;
    }
    if b.artifacts.is_empty() {
        return Err(CertError::MissingBinding {
            index: 6,
            binding: "artifacts",
        });
    }
    for a in &b.artifacts {
        cert_require_nonempty("artifacts.path", &a.path)?;
        require_cert_sha("artifacts.sha256", &a.sha256, 64)?;
    }
    if b.logs.is_empty() {
        return Err(CertError::MissingBinding {
            index: 7,
            binding: "logs",
        });
    }
    for l in &b.logs {
        cert_require_nonempty("logs.path", &l.path)?;
        require_cert_sha("logs.sha256", &l.sha256, 64)?;
        cert_require_nonempty("logs.produced_by", &l.produced_by)?;
    }
    // Binding 8 is the skip policy. An EMPTY `skipped_cells` list is legal and expected — a
    // matrix with no skips is the good outcome — but the class list itself must be the four,
    // so a receipt cannot express a taxonomy the contract does not have.
    if b.skip_policy.classes.is_empty() {
        return Err(CertError::MissingBinding {
            index: 8,
            binding: "skip_policy.classes",
        });
    }
    for class in &b.skip_policy.classes {
        if !CERT_SKIP_CLASSES.contains(&class.as_str()) {
            return Err(CertError::IllegalSkipClass {
                cell: "<class list>".to_string(),
                class: class.clone(),
            });
        }
    }
    for cell in &b.skip_policy.skipped_cells {
        cert_require_nonempty("skip_policy.skipped_cells.cell_id", &cell.cell_id)?;
        if !CERT_SKIP_CLASSES.contains(&cell.class.as_str()) {
            return Err(CertError::IllegalSkipClass {
                cell: cell.cell_id.clone(),
                class: cell.class.clone(),
            });
        }
        if cell.required_evidence.trim().is_empty() {
            return Err(CertError::SkipWithoutEvidence {
                cell: cell.cell_id.clone(),
            });
        }
        // Criterion 1 says "with no skipped critical case". A receipt that RECORDS one is
        // rejected outright rather than passed with a warning.
        if cell.criticality.eq_ignore_ascii_case("critical") {
            return Err(CertError::SkippedCriticalCase {
                cell: cell.cell_id.clone(),
            });
        }
    }
    if b.skip_policy.skipped_critical_cases != 0 {
        return Err(CertError::SkippedCriticalCase {
            cell: format!("<count {} declared>", b.skip_policy.skipped_critical_cases),
        });
    }
    Ok(())
}

fn validate_cert_findings(findings: &[CertFindingV2]) -> Result<(), CertError> {
    for f in findings {
        cert_require_nonempty("findings.id", &f.id)?;
        cert_require_nonempty("findings.origin", &f.origin)?;
        cert_require_nonempty("findings.subject", &f.subject)?;
        cert_require_nonempty("findings.rationale", &f.rationale)?;

        // A1: provenance must be present, and '-' is how you say "none" explicitly.
        if f.inherited_severity.trim().is_empty() {
            return Err(CertError::MissingProvenance { id: f.id.clone() });
        }
        if !CERT_SEVERITIES.contains(&f.p28_severity.as_str()) {
            return Err(CertError::UnknownSeverity {
                id: f.id.clone(),
                severity: f.p28_severity.clone(),
            });
        }
        if f.contradicted_criterion.trim().is_empty() {
            return Err(CertError::InvalidField(format!(
                "findings.{}.contradicted_criterion is empty; write '-' for none so an \
                 omission cannot read as a none",
                f.id
            )));
        }
        if f.contradicts() && !["1", "2", "3", "4"].contains(&f.contradicted_criterion.as_str()) {
            return Err(CertError::InvalidField(format!(
                "findings.{}.contradicted_criterion {:?} is not one of 1..4 or '-'",
                f.id, f.contradicted_criterion
            )));
        }

        // A disposition outside the recognised vocabulary is rejected. `OPEN` is recognised
        // and is not terminal; the consequence of recording one is enforced on the CLAIMS,
        // not by refusing to write the row. See CERT_DISPOSITIONS.
        if !CERT_DISPOSITIONS.contains(&f.disposition.as_str()) {
            return Err(CertError::UndispositionedFinding {
                id: f.id.clone(),
                disposition: f.disposition.clone(),
            });
        }

        let paper = CERT_PAPER_DISPOSITIONS.contains(&f.disposition.as_str());
        if paper && CERT_BLOCKING_SEVERITIES.contains(&f.p28_severity.as_str()) {
            return Err(CertError::PaperPathAtBlockingSeverity {
                id: f.id.clone(),
                disposition: f.disposition.clone(),
                severity: f.p28_severity.clone(),
            });
        }
        // A2 fires on the contradicted criterion REGARDLESS of recorded severity, so a
        // mis-scored severity cannot reopen the accept path.
        if paper && f.contradicts() {
            return Err(CertError::PaperPathWithContradictedCriterion {
                id: f.id.clone(),
                disposition: f.disposition.clone(),
                criterion: f.contradicted_criterion.clone(),
            });
        }
        if paper {
            if f.owner.trim().is_empty() {
                return Err(CertError::PaperPathMissingEvidence {
                    id: f.id.clone(),
                    disposition: f.disposition.clone(),
                    missing: "a named owner",
                });
            }
            if f.backlog_id.trim().is_empty() {
                return Err(CertError::PaperPathMissingEvidence {
                    id: f.id.clone(),
                    disposition: f.disposition.clone(),
                    missing: "a backlog id",
                });
            }
        }
        if f.disposition == "FIXED" && f.executable_check.trim().is_empty() {
            return Err(CertError::RepairPathMissingEvidence {
                id: f.id.clone(),
                disposition: f.disposition.clone(),
                missing: "an executable-check reference; a repair is proved by a check, not asserted",
            });
        }
        if f.disposition == "DISPROVED" && f.counter_evidence.trim().is_empty() {
            return Err(CertError::RepairPathMissingEvidence {
                id: f.id.clone(),
                disposition: f.disposition.clone(),
                missing: "a counter-evidence reference",
            });
        }
    }
    Ok(())
}

/// Amendment A3, enforced rather than intended.
fn validate_cert_claims(body: &CertificationBodyV2) -> Result<(), CertError> {
    for key in body.claims.keys() {
        if !CERT_PERMITTED_CLAIMS.contains(&key.as_str()) {
            return Err(CertError::OverClaim { claim: key.clone() });
        }
    }
    for claim in CERT_PERMITTED_CLAIMS {
        if !body.claims.contains_key(claim) {
            return Err(CertError::MissingClaim { claim });
        }
    }
    // A claim asserted TRUE is recomputed against the receipt's own ledger. This is the
    // in-receipt half; `f28-verify-bindings.py` independently recomputes the same three from
    // the RAW evidence, and both must agree.
    if body.claims["zero_undispositioned_findings"] {
        let undispositioned: Vec<&str> = body
            .findings
            .iter()
            .filter(|f| !CERT_TERMINAL_DISPOSITIONS.contains(&f.disposition.as_str()))
            .map(|f| f.id.as_str())
            .collect();
        if !undispositioned.is_empty() {
            return Err(CertError::ClaimContradictsLedger {
                claim: "zero_undispositioned_findings",
                detail: format!("no terminal disposition: {}", undispositioned.join(", ")),
            });
        }
    }
    if body.claims["zero_skipped_critical_cases"]
        && (body.bindings.skip_policy.skipped_critical_cases != 0
            || body
                .bindings
                .skip_policy
                .skipped_cells
                .iter()
                .any(|c| c.criticality.eq_ignore_ascii_case("critical")))
    {
        return Err(CertError::ClaimContradictsLedger {
            claim: "zero_skipped_critical_cases",
            detail: "the skip policy records a skipped critical case".to_string(),
        });
    }
    if body.claims["zero_unresolved_critical_or_high"] {
        let unresolved: Vec<&str> = body
            .findings
            .iter()
            .filter(|f| {
                CERT_BLOCKING_SEVERITIES.contains(&f.p28_severity.as_str())
                    && !["FIXED", "DISPROVED"].contains(&f.disposition.as_str())
            })
            .map(|f| f.id.as_str())
            .collect();
        if !unresolved.is_empty() {
            return Err(CertError::ClaimContradictsLedger {
                claim: "zero_unresolved_critical_or_high",
                detail: format!("unresolved at CRITICAL/HIGH: {}", unresolved.join(", ")),
            });
        }
    }
    Ok(())
}

fn require_cert_sha(field: &str, value: &str, length: usize) -> Result<(), CertError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(CertError::InvalidField(format!(
            "{field} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(())
}
