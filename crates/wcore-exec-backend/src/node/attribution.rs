//! F25-03 — attested node attribution.
//!
//! ## Why this is not a `node_name` field
//!
//! The cheap implementation is to add a `node_name: String` to the receipt
//! body. It would satisfy every surface: `node list` shows a name, receipts
//! carry a name, an operator can read a name. And it would be worthless,
//! because any party that can produce a receipt can produce any name. **An
//! attribution field a caller can set is not attribution.**
//!
//! So node identity goes INSIDE [`crate::receipt::ReceiptBody`], which is the
//! structure the backend's Ed25519 attestation signs. Altering it changes
//! `body_sha256`, which breaks the signature, which fails
//! [`crate::receipt::ExecutionReceipt::verify`] exactly as altering the backend
//! identity does. There is no second, weaker path.
//!
//! ## What "survives a disruption" means concretely
//!
//! [`verify_node_attribution`] takes a receipt AND the node record the
//! controller pinned at pairing time, and re-derives the whole chain:
//! signature → body digest → backend identity → node identity → the key pinned
//! when the operator paired that machine. Nothing in that chain reads anything
//! the far end sent after pairing, so a node going away, coming back, being
//! revoked, or being re-paired cannot change the answer for work that already
//! happened. That is the property Success Criterion 2 is really asking for, and
//! it is why the re-verification is a function rather than a comment.

use serde::{Deserialize, Serialize};

use super::pairing::NodeIdentity;
use super::registry::NodeRecord;
use crate::error::{ExecError, Result};
use crate::receipt::{BackendIdentity, ExecutionReceipt};

/// The node half of a receipt's attested identity.
///
/// Lives inside the signed body. Kept minimal on purpose: everything here has
/// to be worth signing, and a field that is merely informative invites a future
/// change to make it settable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAttribution {
    pub node_id: String,
    pub machine_id: String,
    /// SHA-256 of the node's Ed25519 verifying key — the identity proper.
    pub key_id: String,
    pub contract_version: super::version::NodeContractVersion,
}

impl NodeAttribution {
    pub fn from_identity(identity: &NodeIdentity) -> Self {
        Self {
            node_id: identity.node_id.clone(),
            machine_id: identity.machine_id.clone(),
            key_id: identity.key_id.clone(),
            contract_version: identity.contract_version,
        }
    }

    pub fn validate(&self) -> Result<()> {
        crate::contract::validate_identifier("node.node_id", &self.node_id)?;
        crate::contract::validate_identifier("node.machine_id", &self.machine_id)?;
        crate::receipt::validate_sha256_public("node.key_id", &self.key_id)?;
        Ok(())
    }

    /// Does this attribution name the node in `record`?
    pub fn matches(&self, record: &NodeRecord) -> bool {
        self.key_id == record.identity.key_id
            && self.node_id == record.identity.node_id
            && self.machine_id == record.identity.machine_id
    }
}

/// The outcome of re-verifying an attribution chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionVerdict {
    /// The chain verifies end to end against the pinned node record.
    Holds {
        node_id: String,
        key_id: String,
        backend_id: String,
    },
    /// The chain is broken. This is a HIGH finding, never a warning.
    Broken { reason: String },
    /// The receipt carries no node attribution at all — work that predates the
    /// node layer, or work run directly on the controller. Distinct from
    /// `Broken` because "not attributed to a node" and "attributed to the
    /// wrong node" are different facts.
    Unattributed,
}

impl AttributionVerdict {
    pub fn holds(&self) -> bool {
        matches!(self, AttributionVerdict::Holds { .. })
    }
    pub fn label(&self) -> String {
        match self {
            AttributionVerdict::Holds {
                node_id, key_id, ..
            } => format!(
                "HOLDS — work attributed to node '{node_id}' (key {})",
                super::pairing::short(key_id)
            ),
            AttributionVerdict::Broken { reason } => format!("BROKEN — {reason}"),
            AttributionVerdict::Unattributed => {
                "UNATTRIBUTED — this receipt carries no node identity".to_string()
            }
        }
    }
}

/// Re-derive the whole chain from a receipt to the node the operator paired.
///
/// `expected_backend` and `backend_key` are the caller-pinned backend identity
/// and key that [`ExecutionReceipt::verify`] already requires — this function
/// deliberately does not weaken that requirement, it adds the node link on top
/// of it.
pub fn verify_node_attribution(
    receipt: &ExecutionReceipt,
    expected_backend: &BackendIdentity,
    backend_key: &ed25519_dalek::VerifyingKey,
    node: &NodeRecord,
) -> AttributionVerdict {
    // 1. The receipt itself must verify. If the signature does not check out,
    //    every field in it — including the node identity — is unattested.
    if let Err(e) = receipt.verify(expected_backend, backend_key) {
        return AttributionVerdict::Broken {
            reason: format!("receipt does not verify: {e}"),
        };
    }

    // 2. The node identity has to be present, well-formed, and the one the
    //    operator pinned when they paired the machine.
    let Some(attribution) = receipt.body.node.as_ref() else {
        return AttributionVerdict::Unattributed;
    };
    if let Err(e) = attribution.validate() {
        return AttributionVerdict::Broken {
            reason: format!("node attribution is malformed: {e}"),
        };
    }
    if !attribution.matches(node) {
        return AttributionVerdict::Broken {
            reason: format!(
                "receipt attributes work to node '{}' (key {}) but the pinned record for \
                 '{}' carries key {}",
                attribution.node_id,
                super::pairing::short(&attribution.key_id),
                node.identity.node_id,
                super::pairing::short(&node.identity.key_id)
            ),
        };
    }

    // 3. The pinned key must actually hash to the pinned key_id. This catches a
    //    registry that was edited by hand to point a familiar node name at a
    //    different machine's key.
    match pinned_key(node) {
        Ok(key) => {
            if crate::receipt::sha256_public(key.as_bytes()) != node.identity.key_id {
                return AttributionVerdict::Broken {
                    reason: format!(
                        "the pinned verifying key for node '{}' does not hash to its \
                         recorded key_id — the registry record is inconsistent",
                        node.identity.node_id
                    ),
                };
            }
        }
        Err(e) => {
            return AttributionVerdict::Broken {
                reason: format!("pinned verifying key is unusable: {e}"),
            };
        }
    }

    AttributionVerdict::Holds {
        node_id: attribution.node_id.clone(),
        key_id: attribution.key_id.clone(),
        backend_id: receipt.body.backend.backend_id.clone(),
    }
}

fn pinned_key(node: &NodeRecord) -> Result<ed25519_dalek::VerifyingKey> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&node.verifying_key_base64)
        .map_err(|_| ExecError::Receipt("pinned node key is not base64".into()))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| ExecError::Receipt("pinned node key is not 32 bytes".into()))?;
    ed25519_dalek::VerifyingKey::from_bytes(&arr)
        .map_err(|_| ExecError::Receipt("pinned node key is malformed".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::HibernationObservation;
    use crate::contract::{BackendKind, ResourceBudget};
    use crate::node::capability::NodeAdvertisement;
    use crate::node::registry::NodeRegistry;
    use crate::receipt::{
        EventKind, PROTOCOL_VERSION, ReceiptBody, ReceiptEvent, ReceiptSigner, TaskEvidence,
        TerminalStatus, Timing, Transport, sha256_public,
    };
    use ed25519_dalek::SigningKey;
    use tempfile::TempDir;

    fn node_key() -> SigningKey {
        SigningKey::from_bytes(&[21u8; 32])
    }

    fn node_identity() -> NodeIdentity {
        NodeIdentity {
            node_id: "alpha".into(),
            machine_id: "test-host".into(),
            os: "linux".into(),
            contract_version: crate::node::version::NODE_CONTRACT_VERSION,
            key_id: sha256_public(node_key().verifying_key().as_bytes()),
        }
    }

    fn paired(tmp: &TempDir) -> (NodeRegistry, NodeRecord) {
        let reg = NodeRegistry::new(tmp.path());
        let rec = reg
            .record_paired(
                node_identity(),
                node_key().verifying_key(),
                "ssh",
                "host.example",
                "wayland-core",
                NodeAdvertisement::empty("alpha"),
            )
            .unwrap();
        (reg, rec)
    }

    fn receipt_with(
        node: Option<NodeAttribution>,
    ) -> (
        ExecutionReceipt,
        BackendIdentity,
        ed25519_dalek::VerifyingKey,
    ) {
        let signer = ReceiptSigner::from_seed([9u8; 32]);
        let backend = BackendIdentity {
            backend_id: "local".into(),
            instance_id: "inst-1".into(),
            version: "0.12.25".into(),
            key_id: signer.key_id().to_string(),
        };
        let limits = ResourceBudget::new(1_000, 1 << 20, 5_000, 1 << 16).unwrap();
        let artifact_sha = sha256_public(b"artifact");
        let events = vec![
            ReceiptEvent {
                sequence: 1,
                event: EventKind::TaskAccepted {
                    backend_id: "local".into(),
                    task_id: "t-1".into(),
                    workspace_sha256: sha256_public(b"ws"),
                    input_sha256: sha256_public(b"in"),
                },
            },
            ReceiptEvent {
                sequence: 2,
                event: EventKind::ArtifactPublished {
                    name: "out.txt".into(),
                    sha256: artifact_sha.clone(),
                    bytes: 8,
                },
            },
            ReceiptEvent {
                sequence: 3,
                event: EventKind::Succeeded {
                    artifact_sha256: artifact_sha.clone(),
                },
            },
        ];
        let body = ReceiptBody {
            protocol_version: PROTOCOL_VERSION,
            backend: backend.clone(),
            node,
            transport: Transport {
                kind: BackendKind::Local,
                endpoint: "localhost".into(),
            },
            task: TaskEvidence {
                task_id: "t-1".into(),
                workspace_sha256: sha256_public(b"ws"),
                input_sha256: sha256_public(b"in"),
                resources: limits,
            },
            limits,
            events_sha256: crate::receipt::events_digest(&events),
            events,
            artifact: Some(crate::receipt::ArtifactEvidence {
                name: "out.txt".into(),
                sha256: sha256_public(b"artifact"),
                bytes: 8,
            }),
            terminal: TerminalStatus::Success,
            timing: Timing {
                started_unix_ms: 1,
                finished_unix_ms: 2,
                wall_ms: 1,
            },
            hibernation: HibernationObservation::NotApplicable,
            secrets_exposed: vec![],
            egress_decision: "deny".into(),
        };
        let key = signer.verifying_key();
        (signer.seal(body).unwrap(), backend, key)
    }

    #[test]
    fn a_genuine_chain_holds() {
        let tmp = TempDir::new().unwrap();
        let (_reg, record) = paired(&tmp);
        let (receipt, backend, key) =
            receipt_with(Some(NodeAttribution::from_identity(&node_identity())));
        let verdict = verify_node_attribution(&receipt, &backend, &key, &record);
        assert!(verdict.holds(), "{}", verdict.label());
    }

    /// The whole reason attribution lives inside the signed body.
    #[test]
    fn altering_the_node_identity_breaks_verification_like_a_tampered_backend_does() {
        let tmp = TempDir::new().unwrap();
        let (_reg, record) = paired(&tmp);
        let (mut receipt, backend, key) =
            receipt_with(Some(NodeAttribution::from_identity(&node_identity())));

        // Rewrite the node id, as a caller wanting to blame another machine would.
        receipt.body.node.as_mut().unwrap().node_id = "beta".into();
        let verdict = verify_node_attribution(&receipt, &backend, &key, &record);
        assert!(!verdict.holds());
        assert!(
            verdict.label().contains("receipt does not verify"),
            "the signature must break, not merely the comparison: {}",
            verdict.label()
        );
    }

    /// A receipt for a DIFFERENT node must not verify against this record, even
    /// though it is perfectly well-signed on its own terms.
    #[test]
    fn a_well_signed_receipt_from_another_node_does_not_attribute_to_this_one() {
        let tmp = TempDir::new().unwrap();
        let (_reg, record) = paired(&tmp);
        let other = NodeIdentity {
            node_id: "beta".into(),
            machine_id: "other-host".into(),
            os: "windows".into(),
            contract_version: crate::node::version::NODE_CONTRACT_VERSION,
            key_id: sha256_public(
                SigningKey::from_bytes(&[22u8; 32])
                    .verifying_key()
                    .as_bytes(),
            ),
        };
        let (receipt, backend, key) = receipt_with(Some(NodeAttribution::from_identity(&other)));
        let verdict = verify_node_attribution(&receipt, &backend, &key, &record);
        assert!(!verdict.holds());
        assert!(
            verdict.label().contains("but the pinned record"),
            "{}",
            verdict.label()
        );
    }

    #[test]
    fn a_receipt_with_no_node_identity_is_unattributed_not_broken() {
        let tmp = TempDir::new().unwrap();
        let (_reg, record) = paired(&tmp);
        let (receipt, backend, key) = receipt_with(None);
        let verdict = verify_node_attribution(&receipt, &backend, &key, &record);
        assert_eq!(verdict, AttributionVerdict::Unattributed);
        assert!(!verdict.holds());
    }

    /// The point of the whole plan: the chain does not consult anything the far
    /// end sends after pairing, so disruptions cannot change it.
    #[test]
    fn attribution_survives_offline_return_revoke_and_repair() {
        let tmp = TempDir::new().unwrap();
        let (reg, record) = paired(&tmp);
        let (receipt, backend, key) =
            receipt_with(Some(NodeAttribution::from_identity(&node_identity())));
        assert!(verify_node_attribution(&receipt, &backend, &key, &record).holds());

        // DISCONNECT
        reg.set_liveness(
            "alpha",
            crate::node::registry::Liveness::Offline {
                observed_unix_ms: 10,
                detail: "far end unreachable".into(),
            },
        )
        .unwrap();
        let after_disconnect = reg.get("alpha").unwrap().unwrap();
        assert!(
            verify_node_attribution(&receipt, &backend, &key, &after_disconnect).holds(),
            "attribution must survive the node going away"
        );

        // RETURN
        reg.set_liveness(
            "alpha",
            crate::node::registry::Liveness::Live {
                observed_unix_ms: 20,
            },
        )
        .unwrap();
        let after_return = reg.get("alpha").unwrap().unwrap();
        assert!(verify_node_attribution(&receipt, &backend, &key, &after_return).holds());

        // REVOKE — the node loses authority for FUTURE work; work already done
        // stays attributable, which is exactly what an audit needs.
        reg.revoke("alpha", "operator withdrew authority").unwrap();
        let after_revoke = reg.get("alpha").unwrap().unwrap();
        assert!(
            verify_node_attribution(&receipt, &backend, &key, &after_revoke).holds(),
            "revoking a node must not erase the attribution of work it already did"
        );

        // RE-PAIR
        reg.clear_revocation("alpha").unwrap();
        let repaired = reg
            .record_paired(
                node_identity(),
                node_key().verifying_key(),
                "ssh",
                "host.example",
                "wayland-core",
                NodeAdvertisement::empty("alpha"),
            )
            .unwrap();
        assert!(verify_node_attribution(&receipt, &backend, &key, &repaired).holds());
    }

    /// A registry record edited by hand to point a familiar name at another
    /// machine's key must not silently validate.
    #[test]
    fn an_inconsistent_registry_record_is_reported_broken() {
        use base64::Engine as _;
        let tmp = TempDir::new().unwrap();
        let (_reg, mut record) = paired(&tmp);
        record.verifying_key_base64 = base64::engine::general_purpose::STANDARD.encode(
            SigningKey::from_bytes(&[99u8; 32])
                .verifying_key()
                .as_bytes(),
        );
        let (receipt, backend, key) =
            receipt_with(Some(NodeAttribution::from_identity(&node_identity())));
        let verdict = verify_node_attribution(&receipt, &backend, &key, &record);
        assert!(!verdict.holds());
        assert!(
            verdict.label().contains("inconsistent"),
            "{}",
            verdict.label()
        );
    }
}
