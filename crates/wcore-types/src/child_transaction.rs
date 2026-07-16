//! Path-free, replay-safe evidence for transactional delegated mutation.
//!
//! These types do not perform Git operations. A runtime may only emit merge or
//! rollback dispositions after resolving the named objects, checking ancestry,
//! and applying a compare-and-swap against the expected parent revision.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::spawner::{
    ChildId, ChildWorkspaceMode, DurableChildRecord, DurableChildStatus, MAX_DURABLE_CHILD_ID_BYTES,
};

pub const CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION: u16 = 1;
pub const MAX_CHILD_TRANSACTION_GATES: usize = 64;
const MAX_TRANSACTION_STRING_BYTES: usize = 512;
const RECEIPT_DIGEST_DOMAIN: &[u8] = b"wayland-core:child-transaction-receipt:v1\0";
const GATE_PLAN_DIGEST_DOMAIN: &[u8] = b"wayland-core:child-transaction-gate-plan:v1\0";

/// One orchestrator-authorized executable gate and its pinned execution
/// closure. The runtime owns this plan; child output cannot define it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildGateRequirement {
    pub gate_id: String,
    pub gate_closure_digest: String,
}

/// Ordered gate closure authorized before delegated mutation begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildGatePlan {
    pub required_gates: Vec<ChildGateRequirement>,
}

impl ChildGatePlan {
    pub fn validate(&self) -> Result<(), ChildTransactionValidationError> {
        if self.required_gates.len() > MAX_CHILD_TRANSACTION_GATES {
            return Err(ChildTransactionValidationError::TooManyGates);
        }
        let mut gate_ids = BTreeSet::new();
        for gate in &self.required_gates {
            validate_identifier("gate_plan.gate_id", &gate.gate_id)?;
            validate_digest("gate_plan.gate_closure_digest", &gate.gate_closure_digest)?;
            if !gate_ids.insert(&gate.gate_id) {
                return Err(ChildTransactionValidationError::InvalidField(
                    "gate_plan.gate_id",
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_digest(&self) -> Result<String, ChildTransactionValidationError> {
        self.validate()?;
        canonical_digest(GATE_PLAN_DIGEST_DOMAIN, self)
    }
}

/// Immutable subject that one executable gate actually evaluated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildGateSubject {
    pub base_revision: String,
    pub candidate_revision: String,
    pub diff_digest: String,
    pub request_digest: String,
    pub policy_digest: String,
    /// Digest of the orchestrator-owned `ChildGatePlan`.
    pub gate_plan_digest: String,
    /// Digest of argv, environment names, cwd semantics, and transitive inputs.
    pub gate_closure_digest: String,
}

/// Result of one executable acceptance gate over an isolated child workspace.
/// Command text, host paths, captured output, and environment values are never
/// serialized into this receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildGateReceipt {
    pub gate_id: String,
    pub subject: ChildGateSubject,
    pub evidence_digest: String,
    pub outcome: ChildGateOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildGateOutcome {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    InfrastructureError,
}

/// Orchestrator-owned handling of one isolated child workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildTransactionDisposition {
    Active,
    Retained {
        reason_digest: String,
    },
    MergeReady {
        expected_parent_revision: String,
    },
    Merged {
        expected_parent_revision: String,
        parent_revision: String,
        parent_tree_digest: String,
        ancestry_evidence_digest: String,
    },
    Conflict {
        expected_parent_revision: String,
        observed_parent_revision: String,
        evidence_digest: String,
    },
    RolledBack {
        expected_parent_revision: String,
        parent_revision: String,
        parent_tree_digest: String,
        evidence_digest: String,
    },
}

/// Versioned, predecessor-linked evidence for one delegated mutation.
///
/// `validate` checks structural truth only and must not authorize a merge.
/// `validate_for_child` additionally binds the receipt to one authoritative
/// durable-child snapshot and orchestrator-owned gate plan. Neither method
/// substitutes for the runtime's Git object, ancestry, tree, and parent-head
/// compare-and-swap checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildTransactionReceipt {
    pub schema_version: u16,
    pub transaction_id: String,
    pub receipt_id: String,
    pub receipt_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_receipt_digest: Option<String>,
    pub child_id: ChildId,
    pub child_declaration_id: String,
    /// Durable-child revision against which this receipt was authorized.
    pub child_revision: u64,
    pub workspace_id: String,
    /// Immutable Git object selected before the child workspace was created.
    pub base_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_revision: Option<String>,
    pub request_digest: String,
    pub policy_digest: String,
    /// Canonical digest of the orchestrator-owned gate plan.
    pub gate_plan_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<ChildGateReceipt>,
    pub disposition: ChildTransactionDisposition,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

impl ChildTransactionReceipt {
    pub fn validate(&self) -> Result<(), ChildTransactionValidationError> {
        if self.schema_version != CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION {
            return Err(ChildTransactionValidationError::UnsupportedReceiptSchema(
                self.schema_version,
            ));
        }
        validate_identifier("transaction_id", &self.transaction_id)?;
        validate_identifier("receipt_id", &self.receipt_id)?;
        validate_identifier("child_id", self.child_id.as_str())?;
        validate_identifier("child_declaration_id", &self.child_declaration_id)?;
        validate_identifier("workspace_id", &self.workspace_id)?;
        validate_git_revision("base_revision", &self.base_revision)?;
        validate_digest("request_digest", &self.request_digest)?;
        validate_digest("policy_digest", &self.policy_digest)?;
        validate_digest("gate_plan_digest", &self.gate_plan_digest)?;
        if let Some(candidate) = &self.candidate_revision {
            validate_git_revision("candidate_revision", candidate)?;
        }
        if let Some(diff) = &self.diff_digest {
            validate_digest("diff_digest", diff)?;
        }
        match (&self.previous_receipt_digest, self.receipt_revision) {
            (None, 0) => {}
            (Some(previous), revision) if revision > 0 => {
                validate_digest("previous_receipt_digest", previous)?;
            }
            _ => return Err(ChildTransactionValidationError::InvalidSequence),
        }
        if self.updated_at_unix_ms < self.created_at_unix_ms {
            return Err(ChildTransactionValidationError::InvalidTimestamp);
        }
        if self.gates.len() > MAX_CHILD_TRANSACTION_GATES {
            return Err(ChildTransactionValidationError::TooManyGates);
        }

        let mut gate_ids = BTreeSet::new();
        for gate in &self.gates {
            self.validate_gate(gate)?;
            if !gate_ids.insert(&gate.gate_id) {
                return Err(ChildTransactionValidationError::InvalidField("gate_id"));
            }
        }
        self.validate_disposition()
    }

    /// Bind this receipt to the exact durable-child snapshot that authorized
    /// it. Callers validating historical receipts must load that historical
    /// child revision rather than comparing against a later live snapshot.
    pub fn validate_for_child(
        &self,
        child: &DurableChildRecord,
        gate_plan: &ChildGatePlan,
    ) -> Result<(), ChildTransactionValidationError> {
        self.validate()?;
        gate_plan.validate()?;
        if self.child_id != child.child_id
            || self.child_declaration_id != child.declaration_id
            || self.child_revision != child.revision
            || self.workspace_id != child.workspace.workspace_id
            || child.workspace.mode != ChildWorkspaceMode::Isolated
            || self.request_digest != child.request.exact_digest
            || self.policy_digest != child.policy_snapshot.exact_digest
            || self.gate_plan_digest != gate_plan.canonical_digest()?
        {
            return Err(ChildTransactionValidationError::ChildBindingMismatch);
        }
        self.validate_gate_plan(gate_plan)?;
        if matches!(
            self.disposition,
            ChildTransactionDisposition::MergeReady { .. }
                | ChildTransactionDisposition::Merged { .. }
        ) && child.status != DurableChildStatus::Succeeded
        {
            return Err(ChildTransactionValidationError::ChildBindingMismatch);
        }
        Ok(())
    }

    /// Canonical, domain-separated digest used by predecessor links and
    /// content-addressed storage.
    pub fn canonical_digest(&self) -> Result<String, ChildTransactionValidationError> {
        canonical_digest(RECEIPT_DIGEST_DOMAIN, self)
    }

    fn validate_gate_plan(
        &self,
        gate_plan: &ChildGatePlan,
    ) -> Result<(), ChildTransactionValidationError> {
        for gate in &self.gates {
            let Some(requirement) = gate_plan
                .required_gates
                .iter()
                .find(|requirement| requirement.gate_id == gate.gate_id)
            else {
                return Err(ChildTransactionValidationError::GatePlanMismatch);
            };
            if gate.subject.gate_closure_digest != requirement.gate_closure_digest {
                return Err(ChildTransactionValidationError::GatePlanMismatch);
            }
        }
        if matches!(
            self.disposition,
            ChildTransactionDisposition::MergeReady { .. }
                | ChildTransactionDisposition::Merged { .. }
        ) && (self.gates.len() != gate_plan.required_gates.len()
            || self
                .gates
                .iter()
                .zip(&gate_plan.required_gates)
                .any(|(receipt, requirement)| {
                    receipt.gate_id != requirement.gate_id
                        || receipt.subject.gate_closure_digest != requirement.gate_closure_digest
                }))
        {
            return Err(ChildTransactionValidationError::GatePlanMismatch);
        }
        Ok(())
    }

    fn validate_gate(
        &self,
        gate: &ChildGateReceipt,
    ) -> Result<(), ChildTransactionValidationError> {
        validate_identifier("gate_id", &gate.gate_id)?;
        validate_digest("gate.evidence_digest", &gate.evidence_digest)?;
        validate_git_revision("gate.subject.base_revision", &gate.subject.base_revision)?;
        validate_git_revision(
            "gate.subject.candidate_revision",
            &gate.subject.candidate_revision,
        )?;
        for (field, digest) in [
            ("gate.subject.diff_digest", &gate.subject.diff_digest),
            ("gate.subject.request_digest", &gate.subject.request_digest),
            ("gate.subject.policy_digest", &gate.subject.policy_digest),
            (
                "gate.subject.gate_plan_digest",
                &gate.subject.gate_plan_digest,
            ),
            (
                "gate.subject.gate_closure_digest",
                &gate.subject.gate_closure_digest,
            ),
        ] {
            validate_digest(field, digest)?;
        }
        if gate.subject.base_revision != self.base_revision
            || Some(&gate.subject.candidate_revision) != self.candidate_revision.as_ref()
            || Some(&gate.subject.diff_digest) != self.diff_digest.as_ref()
            || gate.subject.request_digest != self.request_digest
            || gate.subject.policy_digest != self.policy_digest
            || gate.subject.gate_plan_digest != self.gate_plan_digest
        {
            return Err(ChildTransactionValidationError::GateSubjectMismatch);
        }
        match gate.outcome {
            ChildGateOutcome::Passed if gate.exit_code == Some(0) => {}
            ChildGateOutcome::Failed if gate.exit_code.is_some_and(|code| code != 0) => {}
            ChildGateOutcome::TimedOut
            | ChildGateOutcome::Cancelled
            | ChildGateOutcome::InfrastructureError
                if gate.exit_code.is_none() => {}
            _ => {
                return Err(ChildTransactionValidationError::InvalidField(
                    "gate.exit_code",
                ));
            }
        }
        Ok(())
    }

    fn validate_disposition(&self) -> Result<(), ChildTransactionValidationError> {
        match &self.disposition {
            ChildTransactionDisposition::Active => {}
            ChildTransactionDisposition::Retained { reason_digest } => {
                validate_digest("retained.reason_digest", reason_digest)?;
            }
            ChildTransactionDisposition::MergeReady {
                expected_parent_revision,
            } => {
                validate_git_revision(
                    "merge_ready.expected_parent_revision",
                    expected_parent_revision,
                )?;
                self.require_merge_evidence()?;
            }
            ChildTransactionDisposition::Merged {
                expected_parent_revision,
                parent_revision,
                parent_tree_digest,
                ancestry_evidence_digest,
            } => {
                validate_git_revision("merged.expected_parent_revision", expected_parent_revision)?;
                validate_git_revision("merged.parent_revision", parent_revision)?;
                validate_digest("merged.parent_tree_digest", parent_tree_digest)?;
                validate_digest("merged.ancestry_evidence_digest", ancestry_evidence_digest)?;
                self.require_merge_evidence()?;
            }
            ChildTransactionDisposition::Conflict {
                expected_parent_revision,
                observed_parent_revision,
                evidence_digest,
            } => {
                validate_git_revision(
                    "conflict.expected_parent_revision",
                    expected_parent_revision,
                )?;
                validate_git_revision(
                    "conflict.observed_parent_revision",
                    observed_parent_revision,
                )?;
                validate_digest("conflict.evidence_digest", evidence_digest)?;
            }
            ChildTransactionDisposition::RolledBack {
                expected_parent_revision,
                parent_revision,
                parent_tree_digest,
                evidence_digest,
            } => {
                validate_git_revision(
                    "rollback.expected_parent_revision",
                    expected_parent_revision,
                )?;
                validate_git_revision("rollback.parent_revision", parent_revision)?;
                validate_digest("rollback.parent_tree_digest", parent_tree_digest)?;
                validate_digest("rollback.evidence_digest", evidence_digest)?;
            }
        }
        Ok(())
    }

    fn require_merge_evidence(&self) -> Result<(), ChildTransactionValidationError> {
        if self.candidate_revision.is_none()
            || self.diff_digest.is_none()
            || self.gates.is_empty()
            || self
                .gates
                .iter()
                .any(|gate| gate.outcome != ChildGateOutcome::Passed)
        {
            return Err(ChildTransactionValidationError::InvalidDisposition);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildTransactionReplay {
    Applied,
    Duplicate,
}

/// Deterministic reducer for one transaction's content-addressed receipts.
/// Every apply recomputes the receipt digest and requires the historical child
/// snapshot plus gate plan that authorized that receipt revision.
#[derive(Debug, Clone, Default)]
pub struct ChildTransactionReducer {
    latest: Option<(String, ChildTransactionReceipt)>,
}

impl ChildTransactionReducer {
    pub fn apply(
        &mut self,
        receipt_digest: &str,
        receipt: ChildTransactionReceipt,
        child: &DurableChildRecord,
        gate_plan: &ChildGatePlan,
    ) -> Result<ChildTransactionReplay, ChildTransactionValidationError> {
        validate_digest("receipt_digest", receipt_digest)?;
        receipt.validate_for_child(child, gate_plan)?;
        if receipt.canonical_digest()? != receipt_digest {
            return Err(ChildTransactionValidationError::ReceiptDigestMismatch);
        }
        let Some((latest_digest, latest)) = self.latest.as_ref() else {
            if receipt.receipt_revision != 0 || receipt.previous_receipt_digest.is_some() {
                return Err(ChildTransactionValidationError::InvalidSequence);
            }
            self.latest = Some((receipt_digest.to_owned(), receipt));
            return Ok(ChildTransactionReplay::Applied);
        };
        if receipt_digest == latest_digest {
            return if &receipt == latest {
                Ok(ChildTransactionReplay::Duplicate)
            } else {
                Err(ChildTransactionValidationError::ReceiptConflict)
            };
        }
        let Some(expected_revision) = latest.receipt_revision.checked_add(1) else {
            return Err(ChildTransactionValidationError::InvalidSequence);
        };
        if receipt.receipt_revision != expected_revision
            || receipt.previous_receipt_digest.as_deref() != Some(latest_digest.as_str())
            || receipt.transaction_id != latest.transaction_id
            || receipt.child_id != latest.child_id
            || receipt.child_declaration_id != latest.child_declaration_id
            || receipt.workspace_id != latest.workspace_id
            || receipt.base_revision != latest.base_revision
            || receipt.request_digest != latest.request_digest
            || receipt.policy_digest != latest.policy_digest
            || receipt.created_at_unix_ms != latest.created_at_unix_ms
            || receipt.updated_at_unix_ms < latest.updated_at_unix_ms
            || !valid_disposition_successor(&latest.disposition, &receipt.disposition)
            || !parent_authority_continues(&latest.disposition, &receipt.disposition)
        {
            return Err(ChildTransactionValidationError::InvalidSequence);
        }
        if subject_is_frozen(&latest.disposition)
            && (receipt.candidate_revision != latest.candidate_revision
                || receipt.diff_digest != latest.diff_digest
                || receipt.gates != latest.gates)
        {
            return Err(ChildTransactionValidationError::ReceiptConflict);
        }
        self.latest = Some((receipt_digest.to_owned(), receipt));
        Ok(ChildTransactionReplay::Applied)
    }

    #[must_use]
    pub fn latest(&self) -> Option<(&str, &ChildTransactionReceipt)> {
        self.latest
            .as_ref()
            .map(|(digest, receipt)| (digest.as_str(), receipt))
    }
}

fn subject_is_frozen(disposition: &ChildTransactionDisposition) -> bool {
    matches!(
        disposition,
        ChildTransactionDisposition::MergeReady { .. }
            | ChildTransactionDisposition::Merged { .. }
            | ChildTransactionDisposition::Conflict { .. }
            | ChildTransactionDisposition::RolledBack { .. }
    )
}

fn valid_disposition_successor(
    current: &ChildTransactionDisposition,
    next: &ChildTransactionDisposition,
) -> bool {
    use ChildTransactionDisposition::{Active, Conflict, MergeReady, Merged, Retained, RolledBack};
    matches!(
        (current, next),
        (
            Active,
            Active | Retained { .. } | MergeReady { .. } | Conflict { .. }
        ) | (Retained { .. }, Active | Retained { .. })
            | (
                MergeReady { .. },
                Merged { .. } | Conflict { .. } | Retained { .. } | RolledBack { .. }
            )
            | (Conflict { .. }, Retained { .. } | RolledBack { .. })
    )
}

fn parent_authority_continues(
    current: &ChildTransactionDisposition,
    next: &ChildTransactionDisposition,
) -> bool {
    use ChildTransactionDisposition::{Conflict, MergeReady, Merged, RolledBack};
    match (current, next) {
        (
            MergeReady {
                expected_parent_revision: current,
            },
            Merged {
                expected_parent_revision: next,
                ..
            }
            | Conflict {
                expected_parent_revision: next,
                ..
            }
            | RolledBack {
                expected_parent_revision: next,
                ..
            },
        )
        | (
            Conflict {
                expected_parent_revision: current,
                ..
            },
            RolledBack {
                expected_parent_revision: next,
                ..
            },
        ) => current == next,
        _ => true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChildTransactionValidationError {
    #[error("unsupported child transaction receipt schema {0}")]
    UnsupportedReceiptSchema(u16),
    #[error("invalid child transaction field {0}")]
    InvalidField(&'static str),
    #[error("invalid SHA-256 digest in child transaction field {0}")]
    InvalidDigest(&'static str),
    #[error("child transaction receipt sequence is invalid")]
    InvalidSequence,
    #[error("child transaction receipt timestamps are non-monotonic")]
    InvalidTimestamp,
    #[error("child transaction receipt has too many gates")]
    TooManyGates,
    #[error("child transaction gate is not bound to the receipt subject")]
    GateSubjectMismatch,
    #[error("child transaction disposition is not supported by its evidence")]
    InvalidDisposition,
    #[error("child transaction receipt does not match durable-child authority")]
    ChildBindingMismatch,
    #[error("child transaction receipt conflicts with committed receipt history")]
    ReceiptConflict,
    #[error("child transaction receipt digest does not match its canonical body")]
    ReceiptDigestMismatch,
    #[error("child transaction gate evidence does not match the authorized gate plan")]
    GatePlanMismatch,
    #[error("child transaction receipt could not be canonically encoded")]
    ReceiptEncoding,
}

fn canonical_digest<T: Serialize>(
    domain: &[u8],
    value: &T,
) -> Result<String, ChildTransactionValidationError> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| ChildTransactionValidationError::ReceiptEncoding)?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_identifier(
    field: &'static str,
    value: &str,
) -> Result<(), ChildTransactionValidationError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.len() > MAX_DURABLE_CHILD_ID_BYTES
    {
        return Err(ChildTransactionValidationError::InvalidField(field));
    }
    Ok(())
}

fn validate_digest(
    field: &'static str,
    value: &str,
) -> Result<(), ChildTransactionValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ChildTransactionValidationError::InvalidDigest(field));
    }
    Ok(())
}

fn validate_git_revision(
    field: &'static str,
    value: &str,
) -> Result<(), ChildTransactionValidationError> {
    if !matches!(value.len(), 40 | 64)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || value.len() > MAX_TRANSACTION_STRING_BYTES
    {
        return Err(ChildTransactionValidationError::InvalidField(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::spawner::{
        ChildDeliveryState, ChildDesiredState, ChildOrigin, ChildParent, ChildPolicySnapshot,
        ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace,
        DURABLE_CHILD_SCHEMA_VERSION,
    };

    use super::*;

    fn digest(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    fn revision(byte: char) -> String {
        std::iter::repeat_n(byte, 40).collect()
    }

    fn gate_plan() -> ChildGatePlan {
        ChildGatePlan {
            required_gates: vec![ChildGateRequirement {
                gate_id: "cargo-test".to_owned(),
                gate_closure_digest: digest('6'),
            }],
        }
    }

    fn subject() -> ChildGateSubject {
        ChildGateSubject {
            base_revision: revision('1'),
            candidate_revision: revision('2'),
            diff_digest: digest('3'),
            request_digest: digest('4'),
            policy_digest: digest('5'),
            gate_plan_digest: gate_plan().canonical_digest().unwrap(),
            gate_closure_digest: digest('6'),
        }
    }

    fn passed_gate() -> ChildGateReceipt {
        ChildGateReceipt {
            gate_id: "cargo-test".to_owned(),
            subject: subject(),
            evidence_digest: digest('7'),
            outcome: ChildGateOutcome::Passed,
            exit_code: Some(0),
        }
    }

    fn receipt() -> ChildTransactionReceipt {
        ChildTransactionReceipt {
            schema_version: CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION,
            transaction_id: "transaction-1".to_owned(),
            receipt_id: "transaction-1-receipt-0".to_owned(),
            receipt_revision: 0,
            previous_receipt_digest: None,
            child_id: ChildId::new("child-1").unwrap(),
            child_declaration_id: "declaration-1".to_owned(),
            child_revision: 4,
            workspace_id: "isolated-child-1".to_owned(),
            base_revision: revision('1'),
            candidate_revision: Some(revision('2')),
            request_digest: digest('4'),
            policy_digest: digest('5'),
            gate_plan_digest: gate_plan().canonical_digest().unwrap(),
            diff_digest: Some(digest('3')),
            gates: vec![passed_gate()],
            disposition: ChildTransactionDisposition::MergeReady {
                expected_parent_revision: revision('1'),
            },
            created_at_unix_ms: 100,
            updated_at_unix_ms: 200,
        }
    }

    fn durable_child() -> DurableChildRecord {
        DurableChildRecord {
            schema_version: DURABLE_CHILD_SCHEMA_VERSION,
            declaration_id: "declaration-1".to_owned(),
            child_id: ChildId::new("child-1").unwrap(),
            parent: ChildParent {
                session_id: "session-1".to_owned(),
                turn_id: None,
                parent_child_id: None,
                workflow_run_id: None,
                graph_node_id: None,
                parent_call_id: None,
            },
            origin: ChildOrigin::Delegate,
            request: ChildRequestEvidence::redacted(digest('4')),
            policy_snapshot: ChildPolicySnapshot {
                contract_version: "execution-policy/v1".to_owned(),
                exact_digest: digest('5'),
                posture: "smart".to_owned(),
                approvals: "on_request".to_owned(),
                sandbox: "required".to_owned(),
                source: "local".to_owned(),
                managed_floor_active: false,
                dangerous_activation_id_digest: None,
            },
            provider: Some("test".to_owned()),
            model: Some("test-model".to_owned()),
            workspace: ChildWorkspace {
                mode: ChildWorkspaceMode::Isolated,
                workspace_id: "isolated-child-1".to_owned(),
            },
            status: DurableChildStatus::Succeeded,
            desired_state: ChildDesiredState::Run,
            recovery: ChildRecoveryState::Clean,
            revision: 4,
            timestamps: ChildTimestamps {
                created_at_unix_ms: 10,
                updated_at_unix_ms: 20,
                queued_at_unix_ms: Some(11),
                started_at_unix_ms: Some(12),
                terminal_at_unix_ms: Some(20),
            },
            result: None,
            delivery_target: None,
            delivery_state: ChildDeliveryState::NotRequired,
            attempt: 1,
            retry_of: None,
            applied_events: BTreeMap::new(),
        }
    }

    #[test]
    fn gate_subject_is_bound_to_exact_candidate_and_authority() {
        receipt().validate().unwrap();
        let mut stale = receipt();
        stale.gates[0].subject.candidate_revision = revision('8');
        assert_eq!(
            stale.validate(),
            Err(ChildTransactionValidationError::GateSubjectMismatch)
        );
    }

    #[test]
    fn merge_dispositions_require_executable_passing_evidence() {
        let mut missing = receipt();
        missing.gates.clear();
        assert_eq!(
            missing.validate(),
            Err(ChildTransactionValidationError::InvalidDisposition)
        );
        let mut failed = receipt();
        failed.gates[0].outcome = ChildGateOutcome::Failed;
        failed.gates[0].exit_code = Some(1);
        assert_eq!(
            failed.validate(),
            Err(ChildTransactionValidationError::InvalidDisposition)
        );
    }

    #[test]
    fn child_binding_requires_isolation_identity_policy_request_and_revision() {
        receipt()
            .validate_for_child(&durable_child(), &gate_plan())
            .unwrap();
        let mut shared = durable_child();
        shared.workspace.mode = ChildWorkspaceMode::SharedReadOnly;
        assert_eq!(
            receipt().validate_for_child(&shared, &gate_plan()),
            Err(ChildTransactionValidationError::ChildBindingMismatch)
        );
        let mut unrelated = durable_child();
        unrelated.request = ChildRequestEvidence::redacted(digest('9'));
        assert_eq!(
            receipt().validate_for_child(&unrelated, &gate_plan()),
            Err(ChildTransactionValidationError::ChildBindingMismatch)
        );
    }

    #[test]
    fn reducer_rejects_sibling_receipts_and_terminal_supersession() {
        let mut reducer = ChildTransactionReducer::default();
        let genesis = receipt();
        let genesis_digest = genesis.canonical_digest().unwrap();
        assert_eq!(
            reducer.apply(
                &genesis_digest,
                genesis.clone(),
                &durable_child(),
                &gate_plan(),
            ),
            Ok(ChildTransactionReplay::Applied)
        );
        assert_eq!(
            reducer.apply(
                &genesis_digest,
                genesis.clone(),
                &durable_child(),
                &gate_plan(),
            ),
            Ok(ChildTransactionReplay::Duplicate)
        );

        let mut merged = genesis.clone();
        merged.receipt_id = "transaction-1-receipt-1".to_owned();
        merged.receipt_revision = 1;
        merged.previous_receipt_digest = Some(genesis_digest.clone());
        merged.updated_at_unix_ms = 300;
        merged.disposition = ChildTransactionDisposition::Merged {
            expected_parent_revision: revision('1'),
            parent_revision: revision('8'),
            parent_tree_digest: digest('b'),
            ancestry_evidence_digest: digest('c'),
        };
        let merged_digest = merged.canonical_digest().unwrap();
        assert_eq!(
            reducer.apply(&merged_digest, merged, &durable_child(), &gate_plan(),),
            Ok(ChildTransactionReplay::Applied)
        );

        let mut sibling = genesis;
        sibling.receipt_id = "transaction-1-conflict-1".to_owned();
        sibling.receipt_revision = 1;
        sibling.previous_receipt_digest = Some(genesis_digest);
        sibling.updated_at_unix_ms = 300;
        sibling.disposition = ChildTransactionDisposition::Conflict {
            expected_parent_revision: revision('1'),
            observed_parent_revision: revision('9'),
            evidence_digest: digest('e'),
        };
        let sibling_digest = sibling.canonical_digest().unwrap();
        assert_eq!(
            reducer.apply(&sibling_digest, sibling, &durable_child(), &gate_plan(),),
            Err(ChildTransactionValidationError::InvalidSequence)
        );
    }

    #[test]
    fn reducer_rejects_revision_overflow() {
        let mut latest = receipt();
        latest.receipt_revision = u64::MAX;
        let latest_digest = latest.canonical_digest().unwrap();
        let mut reducer = ChildTransactionReducer {
            latest: Some((latest_digest.clone(), latest.clone())),
        };

        let mut overflow = latest;
        overflow.receipt_id = "transaction-1-overflow".to_owned();
        overflow.previous_receipt_digest = Some(latest_digest);
        overflow.updated_at_unix_ms = 300;
        overflow.disposition = ChildTransactionDisposition::Retained {
            reason_digest: digest('c'),
        };
        let overflow_digest = overflow.canonical_digest().unwrap();
        assert_eq!(
            reducer.apply(&overflow_digest, overflow, &durable_child(), &gate_plan(),),
            Err(ChildTransactionValidationError::InvalidSequence)
        );
    }

    #[test]
    fn reducer_recomputes_content_addressed_receipt_digest() {
        let mut reducer = ChildTransactionReducer::default();
        assert_eq!(
            reducer.apply(&digest('a'), receipt(), &durable_child(), &gate_plan(),),
            Err(ChildTransactionValidationError::ReceiptDigestMismatch)
        );
    }

    #[test]
    fn merge_ready_requires_the_exact_authorized_gate_plan() {
        let mut incomplete_plan = gate_plan();
        incomplete_plan.required_gates.push(ChildGateRequirement {
            gate_id: "cargo-clippy".to_owned(),
            gate_closure_digest: digest('8'),
        });
        let mut candidate = receipt();
        candidate.gate_plan_digest = incomplete_plan.canonical_digest().unwrap();
        candidate.gates[0].subject.gate_plan_digest = candidate.gate_plan_digest.clone();
        assert_eq!(
            candidate.validate_for_child(&durable_child(), &incomplete_plan),
            Err(ChildTransactionValidationError::GatePlanMismatch)
        );

        let mut invented = receipt();
        invented.gates[0].gate_id = "invented-pass".to_owned();
        assert_eq!(
            invented.validate_for_child(&durable_child(), &gate_plan()),
            Err(ChildTransactionValidationError::GatePlanMismatch)
        );
    }

    #[test]
    fn merged_receipt_cannot_change_authorized_parent_revision() {
        let mut reducer = ChildTransactionReducer::default();
        let genesis = receipt();
        let genesis_digest = genesis.canonical_digest().unwrap();
        reducer
            .apply(
                &genesis_digest,
                genesis.clone(),
                &durable_child(),
                &gate_plan(),
            )
            .unwrap();

        let mut drifted = genesis;
        drifted.receipt_id = "transaction-1-receipt-1".to_owned();
        drifted.receipt_revision = 1;
        drifted.previous_receipt_digest = Some(genesis_digest);
        drifted.updated_at_unix_ms = 300;
        drifted.disposition = ChildTransactionDisposition::Merged {
            expected_parent_revision: revision('9'),
            parent_revision: revision('8'),
            parent_tree_digest: digest('b'),
            ancestry_evidence_digest: digest('c'),
        };
        let drifted_digest = drifted.canonical_digest().unwrap();
        assert_eq!(
            reducer.apply(&drifted_digest, drifted, &durable_child(), &gate_plan(),),
            Err(ChildTransactionValidationError::InvalidSequence)
        );
    }

    #[test]
    fn merge_and_rollback_require_parent_cas_result_evidence() {
        let mut merged = receipt();
        merged.disposition = ChildTransactionDisposition::Merged {
            expected_parent_revision: revision('1'),
            parent_revision: "moving-main".to_owned(),
            parent_tree_digest: digest('b'),
            ancestry_evidence_digest: digest('c'),
        };
        assert!(matches!(
            merged.validate(),
            Err(ChildTransactionValidationError::InvalidField(
                "merged.parent_revision"
            ))
        ));

        let mut rolled_back = receipt();
        rolled_back.disposition = ChildTransactionDisposition::RolledBack {
            expected_parent_revision: revision('8'),
            parent_revision: revision('9'),
            parent_tree_digest: "not-a-digest".to_owned(),
            evidence_digest: digest('e'),
        };
        assert!(matches!(
            rolled_back.validate(),
            Err(ChildTransactionValidationError::InvalidDigest(
                "rollback.parent_tree_digest"
            ))
        ));
    }

    #[test]
    fn receipt_rejects_unknown_host_path_and_invalid_exit_semantics() {
        let mut value = serde_json::to_value(receipt()).unwrap();
        value.as_object_mut().unwrap().insert(
            "host_path".to_owned(),
            serde_json::json!("/Users/alice/repo"),
        );
        assert!(serde_json::from_value::<ChildTransactionReceipt>(value).is_err());

        let mut failed_without_status = receipt();
        failed_without_status.gates[0].outcome = ChildGateOutcome::Failed;
        failed_without_status.gates[0].exit_code = None;
        assert_eq!(
            failed_without_status.validate(),
            Err(ChildTransactionValidationError::InvalidField(
                "gate.exit_code"
            ))
        );
    }
}
