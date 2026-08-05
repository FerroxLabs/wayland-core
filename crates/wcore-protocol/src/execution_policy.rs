//! Versioned, output-only execution-policy snapshot sequencing.
//!
//! The authority remains in `wcore-types` and the live approval manager. This
//! module only makes the effective result deterministic for hosts: revision
//! zero is the launch/resume snapshot and every accepted, value-changing
//! transition advances by exactly one.

use std::error::Error;
use std::fmt;

use serde::Serialize;
use wcore_types::execution_policy::EffectiveExecutionPolicy;

pub const EXECUTION_POLICY_CONTRACT_VERSION: &str = "1.0";
pub const EXECUTION_POLICY_CONTRACT_MAJOR: u64 = 1;

/// Why a complete effective-policy snapshot was emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPolicyChangeReason {
    Launch,
    ModeChange,
    Resume,
    Expiry,
}

/// Complete output-only policy state at one session-monotonic revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecutionPolicySnapshot {
    /// Policy is authority-critical. Contract-aware hosts fail closed when
    /// they do not understand this event/version.
    pub critical: bool,
    pub contract_version: String,
    pub revision: u64,
    pub reason: ExecutionPolicyChangeReason,
    /// Audit/display evidence only. Monotonic runtime deadlines remain the
    /// authority for dangerous-session expiry.
    pub effective_at_unix_ms: u64,
    pub policy: EffectiveExecutionPolicy,
}

impl ExecutionPolicySnapshot {
    fn current(
        revision: u64,
        reason: ExecutionPolicyChangeReason,
        effective_at_unix_ms: u64,
        policy: EffectiveExecutionPolicy,
    ) -> Self {
        Self {
            critical: true,
            contract_version: EXECUTION_POLICY_CONTRACT_VERSION.to_owned(),
            revision,
            reason,
            effective_at_unix_ms,
            policy,
        }
    }
}

/// Consumer disposition for a valid next snapshot or exact replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionPolicyAcceptance {
    Advanced,
    Duplicate,
}

/// Fail-closed sequencing errors for authority-bearing snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPolicySequenceError {
    UnsupportedContractVersion { actual: String },
    NonCriticalSnapshot,
    ConflictingDuplicate { revision: u64 },
    OutOfOrder { expected: u64, actual: u64 },
    RevisionOverflow,
}

impl fmt::Display for ExecutionPolicySequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedContractVersion { actual } => write!(
                formatter,
                "unsupported execution-policy contract version: {actual}"
            ),
            Self::NonCriticalSnapshot => {
                formatter.write_str("execution-policy snapshot must be critical")
            }
            Self::ConflictingDuplicate { revision } => write!(
                formatter,
                "execution-policy revision {revision} conflicts with accepted bytes"
            ),
            Self::OutOfOrder { expected, actual } => write!(
                formatter,
                "execution-policy revision out of order: expected {expected}, got {actual}"
            ),
            Self::RevisionOverflow => formatter.write_str("execution-policy revision overflowed"),
        }
    }
}

impl Error for ExecutionPolicySequenceError {}

/// Validate the policy sub-contract major without deserializing authority.
pub fn validate_execution_policy_contract_version(
    version: &str,
) -> Result<(), ExecutionPolicySequenceError> {
    let major = version
        .split_once('.')
        .map_or(version, |(major, _)| major)
        .parse::<u64>()
        .ok();
    if major == Some(EXECUTION_POLICY_CONTRACT_MAJOR) {
        Ok(())
    } else {
        Err(ExecutionPolicySequenceError::UnsupportedContractVersion {
            actual: version.to_owned(),
        })
    }
}

/// Session-local producer sequence and reference reducer semantics.
#[derive(Debug, Clone)]
pub struct ExecutionPolicySequence {
    current: ExecutionPolicySnapshot,
}

impl ExecutionPolicySequence {
    pub fn launch(policy: EffectiveExecutionPolicy, effective_at_unix_ms: u64) -> Self {
        Self {
            current: ExecutionPolicySnapshot::current(
                0,
                ExecutionPolicyChangeReason::Launch,
                effective_at_unix_ms,
                policy,
            ),
        }
    }

    pub fn resume(policy: EffectiveExecutionPolicy, effective_at_unix_ms: u64) -> Self {
        Self {
            current: ExecutionPolicySnapshot::current(
                0,
                ExecutionPolicyChangeReason::Resume,
                effective_at_unix_ms,
                policy,
            ),
        }
    }

    pub const fn current(&self) -> &ExecutionPolicySnapshot {
        &self.current
    }

    /// Advance only when the effective policy bytes changed. Accepted no-op
    /// mode requests therefore do not consume a revision.
    pub fn advance_if_changed(
        &mut self,
        policy: EffectiveExecutionPolicy,
        reason: ExecutionPolicyChangeReason,
        effective_at_unix_ms: u64,
    ) -> Result<Option<&ExecutionPolicySnapshot>, ExecutionPolicySequenceError> {
        if policy == self.current.policy {
            return Ok(None);
        }
        let revision = self
            .current
            .revision
            .checked_add(1)
            .ok_or(ExecutionPolicySequenceError::RevisionOverflow)?;
        self.current =
            ExecutionPolicySnapshot::current(revision, reason, effective_at_unix_ms, policy);
        Ok(Some(&self.current))
    }

    /// Apply a serialized snapshot according to the pinned Desktop reducer
    /// contract. A byte-identical current revision is idempotent; conflicting,
    /// stale or gapped revisions fail closed.
    pub fn accept(
        &mut self,
        snapshot: ExecutionPolicySnapshot,
    ) -> Result<ExecutionPolicyAcceptance, ExecutionPolicySequenceError> {
        validate_execution_policy_contract_version(&snapshot.contract_version)?;
        if !snapshot.critical {
            return Err(ExecutionPolicySequenceError::NonCriticalSnapshot);
        }
        if snapshot.revision == self.current.revision {
            return if snapshot == self.current {
                Ok(ExecutionPolicyAcceptance::Duplicate)
            } else {
                Err(ExecutionPolicySequenceError::ConflictingDuplicate {
                    revision: snapshot.revision,
                })
            };
        }
        let expected = self
            .current
            .revision
            .checked_add(1)
            .ok_or(ExecutionPolicySequenceError::RevisionOverflow)?;
        if snapshot.revision != expected {
            return Err(ExecutionPolicySequenceError::OutOfOrder {
                expected,
                actual: snapshot.revision,
            });
        }
        self.current = snapshot;
        Ok(ExecutionPolicyAcceptance::Advanced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::execution_policy::{ApprovalPolicy, BaselineExecutionPolicy, PolicySource};

    fn policy(approvals: ApprovalPolicy) -> EffectiveExecutionPolicy {
        EffectiveExecutionPolicy::baseline(&BaselineExecutionPolicy::smart(
            approvals,
            PolicySource::DesktopLocalLaunch,
        ))
    }

    #[test]
    fn revisions_advance_only_for_effective_changes() {
        let mut sequence = ExecutionPolicySequence::launch(policy(ApprovalPolicy::Prompt), 10);
        assert!(
            sequence
                .advance_if_changed(
                    policy(ApprovalPolicy::Prompt),
                    ExecutionPolicyChangeReason::ModeChange,
                    11,
                )
                .unwrap()
                .is_none()
        );
        let changed = sequence
            .advance_if_changed(
                policy(ApprovalPolicy::AutoEdit),
                ExecutionPolicyChangeReason::ModeChange,
                12,
            )
            .unwrap()
            .unwrap();
        assert_eq!(changed.revision, 1);
        assert_eq!(changed.effective_at_unix_ms, 12);
        assert_eq!(changed.policy.approvals(), ApprovalPolicy::AutoEdit);
    }

    #[test]
    fn duplicate_is_idempotent_but_conflict_fails_closed() {
        let initial = ExecutionPolicySequence::launch(policy(ApprovalPolicy::Prompt), 10);
        let mut reducer = initial.clone();
        assert_eq!(
            reducer.accept(initial.current().clone()).unwrap(),
            ExecutionPolicyAcceptance::Duplicate
        );

        let conflicting = ExecutionPolicySnapshot::current(
            0,
            ExecutionPolicyChangeReason::Launch,
            10,
            policy(ApprovalPolicy::Bypass),
        );
        assert_eq!(
            reducer.accept(conflicting),
            Err(ExecutionPolicySequenceError::ConflictingDuplicate { revision: 0 })
        );
    }

    #[test]
    fn out_of_order_and_version_mismatch_fail_closed() {
        let mut reducer = ExecutionPolicySequence::launch(policy(ApprovalPolicy::Prompt), 10);
        let gap = ExecutionPolicySnapshot::current(
            2,
            ExecutionPolicyChangeReason::ModeChange,
            12,
            policy(ApprovalPolicy::Bypass),
        );
        assert_eq!(
            reducer.accept(gap),
            Err(ExecutionPolicySequenceError::OutOfOrder {
                expected: 1,
                actual: 2,
            })
        );
        assert!(matches!(
            validate_execution_policy_contract_version("2.0"),
            Err(ExecutionPolicySequenceError::UnsupportedContractVersion { .. })
        ));
        assert!(validate_execution_policy_contract_version("1.7").is_ok());
    }

    #[test]
    fn resume_snapshot_is_explicit_and_critical() {
        let sequence = ExecutionPolicySequence::resume(policy(ApprovalPolicy::Prompt), 99);
        assert_eq!(sequence.current().revision, 0);
        assert_eq!(
            sequence.current().reason,
            ExecutionPolicyChangeReason::Resume
        );
        assert!(sequence.current().critical);
        assert_eq!(
            sequence.current().contract_version,
            EXECUTION_POLICY_CONTRACT_VERSION
        );
    }
}
