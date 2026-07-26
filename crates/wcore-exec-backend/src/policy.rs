//! Effective policy for a task, and the secret-exposure declaration a backend
//! must make BEFORE the task is accepted.
//!
//! The egress decision is READ FROM the `wcore-egress` shared policy rather
//! than re-derived here. Re-deriving it would create a second authority for
//! the same question, and the whole point of the egress chokepoint is that
//! there is exactly one.

use serde::{Deserialize, Serialize};

use crate::contract::{BackendKind, ExecutionTask, SecretChannel, validate_identifier};
use crate::error::Result;

/// Where the egress verdict came from. Recorded so a reader can tell an
/// inherited decision from an invented one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressDecisionSource {
    /// Read from the process-global `wcore-egress` shared policy.
    SharedEgressPolicy,
    /// The backend performs no outbound HTTP at all, so there is nothing to
    /// decide. This is NOT the same as "allowed".
    NoEgressSurface,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectivePolicy {
    pub backend_id: String,
    pub kind: BackendKind,
    pub egress_decision: String,
    pub egress_source: EgressDecisionSource,
    pub secret_channel: SecretChannel,
    /// The exact set of secret NAMES that would be exposed to the task. Names
    /// only; there is deliberately nowhere in this type to put a value.
    pub secrets_exposed: Vec<String>,
    /// Containment the backend composes rather than replaces, named so a
    /// reader can see it was not bypassed.
    pub containment: String,
}

impl EffectivePolicy {
    pub fn validate(&self) -> Result<()> {
        for name in &self.secrets_exposed {
            validate_identifier("secrets_exposed", name)?;
        }
        Ok(())
    }
}

/// Read the current egress disposition from the shared policy.
///
/// `wcore-egress` defaults to an allow-all policy when nothing has installed
/// one, and that default is a KNOWN fail-open (recorded in the phase handoff
/// §7). This function therefore reports the disposition it actually observed
/// including the word `default` when nothing was installed, so a receipt that
/// says `allow-all-default` is legible as a fail-open rather than as a
/// deliberate allow.
pub fn observed_egress_decision() -> (String, EgressDecisionSource) {
    if wcore_egress::global_policy_installed() {
        (
            "shared-egress-policy-installed".to_string(),
            EgressDecisionSource::SharedEgressPolicy,
        )
    } else {
        // Say the fail-open out loud. Rendering this as "allow" would launder
        // an uninstalled boundary into a deliberate decision, and that is
        // exactly the class of laundering this field exists to prevent.
        (
            "allow-all-default-no-policy-installed".to_string(),
            EgressDecisionSource::SharedEgressPolicy,
        )
    }
}

/// Declare, before acceptance, which secrets a backend would expose for this
/// task. The reference backends expose NONE — the deterministic reference task
/// needs no secret, and a backend that needs a credential to reach its own
/// control plane (cloud) keeps that credential on the CONTROL side and never
/// provisions it into the task.
pub fn declared_secret_exposure(kind: BackendKind, _task: &ExecutionTask) -> Vec<String> {
    match kind {
        BackendKind::Local | BackendKind::Container | BackendKind::Ssh | BackendKind::Cloud => {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{ResourceBudget, WorkspaceFile};

    fn task() -> ExecutionTask {
        ExecutionTask {
            task_id: "t-1".into(),
            nonce: "n-1".into(),
            workspace: vec![WorkspaceFile {
                path: "a.txt".into(),
                bytes: b"a".to_vec(),
            }],
            input: b"in".to_vec(),
            argv: vec!["cat".into()],
            artifact_name: "out.bin".into(),
            resources: ResourceBudget::new(1, 1, 1, 1).unwrap(),
        }
    }

    #[test]
    fn no_reference_backend_provisions_a_secret_into_the_task() {
        for kind in [
            BackendKind::Local,
            BackendKind::Container,
            BackendKind::Ssh,
            BackendKind::Cloud,
        ] {
            assert!(
                declared_secret_exposure(kind, &task()).is_empty(),
                "{kind:?} must not provision a secret into the deterministic reference task"
            );
        }
    }

    #[test]
    fn a_policy_carrying_a_secret_value_shaped_name_is_refused() {
        let policy = EffectivePolicy {
            backend_id: "local".into(),
            kind: BackendKind::Local,
            egress_decision: "deny".into(),
            egress_source: EgressDecisionSource::NoEgressSurface,
            secret_channel: SecretChannel::None,
            // A real token would contain characters an identifier refuses.
            secrets_exposed: vec!["fo1_abc/def+ghi=".into()],
            containment: "none".into(),
        };
        assert!(policy.validate().is_err());
    }
}
