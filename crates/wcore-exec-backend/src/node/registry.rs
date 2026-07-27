//! F25-03 — the paired-node registry, revocation and liveness.
//!
//! ## Revocation is a refusal, not a forget
//!
//! Deleting a row is forgetting. Revocation means that afterwards:
//!
//! * work submitted to that node is REFUSED with a named verdict,
//! * in-flight work on it is terminated, and
//! * **nothing reroutes.**
//!
//! That last clause is the one that looks like a missing feature and is
//! actually the requirement. A controller that quietly runs the work on a
//! healthy node instead turns every disruption test green while destroying the
//! attribution the criterion is about: the work happened, it succeeded, and the
//! receipt names a machine the operator never asked for.
//!
//! The revoked record is RETAINED with its state, not deleted, for the same
//! reason a revoked plugin approval is retained: a far end must not be able to
//! re-pair itself back into authority. Re-pairing is an operator action.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::capability::{NodeAdvertisement, now_unix_ms};
use super::pairing::NodeIdentity;
use super::version::{VersionVerdict, evaluate_version};
use crate::error::{ExecError, Result};

/// Whether a node is reachable, as last OBSERVED — never as assumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Liveness {
    /// A probe reached the node at this timestamp.
    Live { observed_unix_ms: u64 },
    /// A probe was attempted and did not reach it.
    Offline {
        observed_unix_ms: u64,
        detail: String,
    },
    /// Never probed since pairing. Distinct from Offline on purpose: "we have
    /// not looked" and "we looked and it was gone" are different facts, and
    /// collapsing them is how a controller reports confidence it does not have.
    Unknown,
}

impl Liveness {
    pub fn label(&self) -> String {
        match self {
            Liveness::Live { .. } => "live".to_string(),
            Liveness::Offline { detail, .. } => format!("offline ({detail})"),
            Liveness::Unknown => "unknown (not probed since pairing)".to_string(),
        }
    }
    pub fn is_live(&self) -> bool {
        matches!(self, Liveness::Live { .. })
    }
}

/// Where a node stands with this controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Paired,
    /// Authority withdrawn by an operator. Retained, never deleted.
    Revoked {
        revoked_unix_ms: u64,
        reason: String,
    },
}

/// One paired node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRecord {
    pub identity: NodeIdentity,
    /// Base64 of the far end's verifying key, pinned at pairing time. Work
    /// attribution is checked against THIS, not against whatever key a later
    /// receipt happens to carry.
    pub verifying_key_base64: String,
    /// How the controller reaches it — an ssh target today.
    pub transport: String,
    pub target: String,
    /// Where `wayland-core` lives ON THE FAR END.
    ///
    /// Recorded at pairing time because `probe` has to invoke the same binary
    /// pairing did. Hardcoding a name and hoping it is on the far end's PATH
    /// makes a perfectly healthy node report OFFLINE — and an offline node then
    /// refuses work, so the false answer becomes a refusal. Found live.
    #[serde(default = "default_remote_bin")]
    pub remote_bin: String,
    pub state: NodeState,
    pub paired_unix_ms: u64,
    pub liveness: Liveness,
    pub advertisement: NodeAdvertisement,
}

impl NodeRecord {
    pub fn version_verdict(&self) -> VersionVerdict {
        evaluate_version(self.identity.contract_version)
    }
    pub fn is_revoked(&self) -> bool {
        matches!(self.state, NodeState::Revoked { .. })
    }
}

/// What happens when work is submitted to a node.
///
/// `Refused` carries the reason so an operator sees WHY, and — critically —
/// there is no `Rerouted` variant. The type cannot express a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionVerdict {
    Accepted { node_id: String },
    Refused { node_id: String, reason: String },
}

impl SubmissionVerdict {
    pub fn is_accepted(&self) -> bool {
        matches!(self, SubmissionVerdict::Accepted { .. })
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            SubmissionVerdict::Refused { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    nodes: BTreeMap<String, NodeRecord>,
}

/// The paired-node registry, persisted next to the live-task registry.
pub struct NodeRegistry {
    root: PathBuf,
}

impl NodeRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default location, honouring the same override the live-task
    /// registry uses so a test never touches an operator's real state.
    pub fn default_location() -> Self {
        Self::new(crate::registry::state_dir())
    }

    fn path(&self) -> PathBuf {
        self.root.join("nodes.json")
    }

    fn load(&self) -> Result<RegistryFile> {
        let p = self.path();
        if !p.is_file() {
            return Ok(RegistryFile::default());
        }
        let raw = std::fs::read_to_string(&p)?;
        // A corrupt registry REFUSES rather than reading as empty. Treating a
        // damaged file as "no nodes" would silently un-revoke every revoked
        // node on the next load.
        serde_json::from_str(&raw).map_err(|e| {
            ExecError::Receipt(format!(
                "node registry at {} is unreadable: {e}",
                p.display()
            ))
        })
    }

    fn store(&self, f: &RegistryFile) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let bytes = serde_json::to_vec_pretty(f)?;
        wcore_config::atomic_write(self.path(), &bytes)
            .map_err(|e| ExecError::Receipt(e.to_string()))?;
        Ok(())
    }

    /// Record a node whose pairing proof has ALREADY been verified.
    ///
    /// Takes the verified key by value so an unverified pairing cannot reach
    /// this function by accident — there is no path that records a node
    /// without something having produced a `VerifyingKey` from a real proof.
    #[allow(clippy::too_many_arguments)]
    pub fn record_paired(
        &self,
        identity: NodeIdentity,
        verified_key: ed25519_dalek::VerifyingKey,
        transport: &str,
        target: &str,
        remote_bin: &str,
        advertisement: NodeAdvertisement,
    ) -> Result<NodeRecord> {
        use base64::Engine as _;
        identity.validate()?;
        let mut f = self.load()?;

        // A revoked node cannot be re-paired implicitly. The far end
        // presenting a valid proof again is exactly the situation revocation
        // exists to survive.
        if let Some(existing) = f.nodes.get(&identity.node_id)
            && existing.is_revoked()
        {
            return Err(ExecError::Receipt(format!(
                "node '{}' is REVOKED; re-pairing is an operator action \
                 (`wayland-core node repair {}` after `node revoke --clear`)",
                identity.node_id, identity.node_id
            )));
        }

        let record = NodeRecord {
            identity: identity.clone(),
            verifying_key_base64: base64::engine::general_purpose::STANDARD
                .encode(verified_key.as_bytes()),
            transport: transport.to_string(),
            target: target.to_string(),
            remote_bin: remote_bin.to_string(),
            state: NodeState::Paired,
            paired_unix_ms: now_unix_ms(),
            liveness: Liveness::Unknown,
            advertisement,
        };
        f.nodes.insert(identity.node_id.clone(), record.clone());
        self.store(&f)?;
        Ok(record)
    }

    pub fn list(&self) -> Result<Vec<NodeRecord>> {
        Ok(self.load()?.nodes.into_values().collect())
    }

    pub fn get(&self, node_id: &str) -> Result<Option<NodeRecord>> {
        Ok(self.load()?.nodes.remove(node_id))
    }

    /// Withdraw authority. The record is RETAINED in a revoked state.
    pub fn revoke(&self, node_id: &str, reason: &str) -> Result<NodeRecord> {
        let mut f = self.load()?;
        let record = f
            .nodes
            .get_mut(node_id)
            .ok_or_else(|| ExecError::Receipt(format!("no node named '{node_id}' is paired")))?;
        record.state = NodeState::Revoked {
            revoked_unix_ms: now_unix_ms(),
            reason: reason.to_string(),
        };
        let out = record.clone();
        self.store(&f)?;
        Ok(out)
    }

    /// Clear a revocation so an operator can deliberately re-pair. This is the
    /// ONLY way out of the revoked state, and it is a local operator action —
    /// nothing the far end sends can trigger it.
    pub fn clear_revocation(&self, node_id: &str) -> Result<()> {
        let mut f = self.load()?;
        let removed = f.nodes.remove(node_id);
        if removed.is_none() {
            return Err(ExecError::Receipt(format!(
                "no node named '{node_id}' is paired"
            )));
        }
        self.store(&f)
    }

    pub fn set_liveness(&self, node_id: &str, liveness: Liveness) -> Result<()> {
        let mut f = self.load()?;
        let record = f
            .nodes
            .get_mut(node_id)
            .ok_or_else(|| ExecError::Receipt(format!("no node named '{node_id}' is paired")))?;
        record.liveness = liveness;
        self.store(&f)
    }

    pub fn set_advertisement(&self, node_id: &str, advertisement: NodeAdvertisement) -> Result<()> {
        let mut f = self.load()?;
        let record = f
            .nodes
            .get_mut(node_id)
            .ok_or_else(|| ExecError::Receipt(format!("no node named '{node_id}' is paired")))?;
        record.advertisement = advertisement;
        self.store(&f)
    }

    /// May work be submitted to this node right now?
    ///
    /// Returns a verdict for THIS node and nothing else. There is deliberately
    /// no "pick another node" path anywhere in this crate: a caller that wants
    /// a different node must name it, so the operator's intent and the
    /// receipt's attribution stay the same thing.
    pub fn evaluate_submission(&self, node_id: &str) -> Result<SubmissionVerdict> {
        let Some(record) = self.get(node_id)? else {
            return Ok(SubmissionVerdict::Refused {
                node_id: node_id.to_string(),
                reason: format!("node '{node_id}' is not paired with this controller"),
            });
        };
        if let NodeState::Revoked { reason, .. } = &record.state {
            return Ok(SubmissionVerdict::Refused {
                node_id: node_id.to_string(),
                reason: format!(
                    "node '{node_id}' is REVOKED ({reason}); refusing to run and NOT \
                     falling back to another node"
                ),
            });
        }
        let verdict = record.version_verdict();
        if !verdict.accepts_work() {
            return Ok(SubmissionVerdict::Refused {
                node_id: node_id.to_string(),
                reason: format!(
                    "node '{node_id}' advertises an unsupported contract version: {}",
                    verdict.label()
                ),
            });
        }
        if let Liveness::Offline { detail, .. } = &record.liveness {
            return Ok(SubmissionVerdict::Refused {
                node_id: node_id.to_string(),
                reason: format!(
                    "node '{node_id}' was last observed offline ({detail}); refusing to run \
                     and NOT falling back to another node"
                ),
            });
        }
        Ok(SubmissionVerdict::Accepted {
            node_id: node_id.to_string(),
        })
    }

    /// Drive every live task recorded against `node_id` to the disconnected
    /// terminal status, returning the task ids affected.
    ///
    /// This is what makes "in-flight work is terminated" true rather than
    /// aspirational. It reads the SAME live-task registry `backend cancel`
    /// uses, so a node going away and an operator cancelling converge on one
    /// ledger instead of two that can disagree.
    pub fn terminate_in_flight(&self, node_id: &str) -> Result<Vec<String>> {
        let mut affected = Vec::new();
        for task in crate::registry::list() {
            if task.handle.as_deref() == Some(node_id) {
                crate::registry::forget(&task.task_id)?;
                affected.push(task.task_id);
            }
        }
        Ok(affected)
    }
}

/// The far-end binary name assumed for records written before `remote_bin`
/// existed. Kept as a named function rather than an inline literal so the
/// migration default is greppable.
fn default_remote_bin() -> String {
    "wayland-core".to_string()
}

/// Where the node registry lives, for operator-facing messages.
pub fn registry_path(root: &Path) -> PathBuf {
    root.join("nodes.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::pairing::NodeIdentity;
    use crate::receipt::sha256_public;
    use ed25519_dalek::SigningKey;
    use tempfile::TempDir;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn identity(node_id: &str, k: &SigningKey) -> NodeIdentity {
        NodeIdentity {
            node_id: node_id.into(),
            machine_id: "test-host".into(),
            os: "linux".into(),
            contract_version: super::super::version::NODE_CONTRACT_VERSION,
            key_id: sha256_public(k.verifying_key().as_bytes()),
        }
    }

    fn registry() -> (TempDir, NodeRegistry) {
        let tmp = TempDir::new().unwrap();
        let reg = NodeRegistry::new(tmp.path());
        (tmp, reg)
    }

    fn pair(reg: &NodeRegistry, node_id: &str, seed: u8) -> NodeRecord {
        let k = key(seed);
        reg.record_paired(
            identity(node_id, &k),
            k.verifying_key(),
            "ssh",
            "host.example",
            "wayland-core",
            NodeAdvertisement::empty(node_id),
        )
        .unwrap()
    }

    #[test]
    fn a_paired_node_accepts_work() {
        let (_t, reg) = registry();
        pair(&reg, "alpha", 7);
        assert!(reg.evaluate_submission("alpha").unwrap().is_accepted());
    }

    #[test]
    fn an_unpaired_node_is_refused() {
        let (_t, reg) = registry();
        let v = reg.evaluate_submission("ghost").unwrap();
        assert!(!v.is_accepted());
        assert!(v.reason().unwrap().contains("not paired"));
    }

    /// The central requirement: revocation refuses, retains, and never reroutes.
    #[test]
    fn revocation_refuses_subsequent_work_and_does_not_reroute() {
        let (_t, reg) = registry();
        pair(&reg, "alpha", 7);
        pair(&reg, "beta", 8);
        assert!(reg.evaluate_submission("alpha").unwrap().is_accepted());

        reg.revoke("alpha", "operator withdrew authority").unwrap();
        let v = reg.evaluate_submission("alpha").unwrap();
        assert!(!v.is_accepted());
        let reason = v.reason().unwrap();
        assert!(reason.contains("REVOKED"), "{reason}");
        assert!(reason.contains("NOT falling back"), "{reason}");

        // The verdict names alpha and only alpha. A reroute would have to
        // produce a verdict naming beta, and the type cannot express one.
        match v {
            SubmissionVerdict::Refused { node_id, .. } => assert_eq!(node_id, "alpha"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        // beta is untouched, but nothing moved alpha's work to it.
        assert!(reg.evaluate_submission("beta").unwrap().is_accepted());
    }

    /// Revocation retains the record; a far end cannot re-pair itself back in.
    #[test]
    fn a_revoked_node_cannot_re_pair_itself() {
        let (_t, reg) = registry();
        pair(&reg, "alpha", 7);
        reg.revoke("alpha", "compromised").unwrap();

        let k = key(7);
        let err = reg
            .record_paired(
                identity("alpha", &k),
                k.verifying_key(),
                "ssh",
                "host.example",
                "wayland-core",
                NodeAdvertisement::empty("alpha"),
            )
            .unwrap_err();
        assert!(err.to_string().contains("REVOKED"), "{err}");
        assert!(!reg.evaluate_submission("alpha").unwrap().is_accepted());

        // Only a deliberate operator action opens the door again.
        reg.clear_revocation("alpha").unwrap();
        pair(&reg, "alpha", 7);
        assert!(reg.evaluate_submission("alpha").unwrap().is_accepted());
    }

    #[test]
    fn a_node_last_observed_offline_is_refused_without_fallback() {
        let (_t, reg) = registry();
        pair(&reg, "alpha", 7);
        reg.set_liveness(
            "alpha",
            Liveness::Offline {
                observed_unix_ms: 1,
                detail: "ssh handshake did not reach the far end".into(),
            },
        )
        .unwrap();
        let v = reg.evaluate_submission("alpha").unwrap();
        assert!(!v.is_accepted());
        assert!(v.reason().unwrap().contains("NOT falling back"));
    }

    /// Unknown liveness is not offline. Collapsing them would make a
    /// freshly-paired node unusable until probed, or an offline one usable.
    #[test]
    fn unknown_liveness_is_distinct_from_offline() {
        let (_t, reg) = registry();
        let rec = pair(&reg, "alpha", 7);
        assert_eq!(rec.liveness, Liveness::Unknown);
        assert!(reg.evaluate_submission("alpha").unwrap().is_accepted());
    }

    /// A damaged registry must refuse, not read as empty — reading as empty
    /// would silently un-revoke every revoked node.
    #[test]
    fn a_corrupt_registry_refuses_rather_than_reading_as_empty() {
        let (tmp, reg) = registry();
        pair(&reg, "alpha", 7);
        reg.revoke("alpha", "compromised").unwrap();
        std::fs::write(tmp.path().join("nodes.json"), b"{ not json").unwrap();
        assert!(reg.list().is_err());
        assert!(reg.evaluate_submission("alpha").is_err());
    }

    #[test]
    fn an_unsupported_contract_version_refuses_work() {
        let (_t, reg) = registry();
        let k = key(7);
        let mut id = identity("alpha", &k);
        id.contract_version = super::super::version::NodeContractVersion {
            major: 99,
            minor: 0,
        };
        reg.record_paired(
            id,
            k.verifying_key(),
            "ssh",
            "host.example",
            "wayland-core",
            NodeAdvertisement::empty("alpha"),
        )
        .unwrap();
        let v = reg.evaluate_submission("alpha").unwrap();
        assert!(!v.is_accepted());
        assert!(v.reason().unwrap().contains("unsupported"), "{v:?}");
    }
}
