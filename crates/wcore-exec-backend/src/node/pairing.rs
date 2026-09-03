//! F25-03 — node identity and pairing.
//!
//! Pairing is a **proof of possession**, not a trust-on-first-use string. The
//! controller mints a random challenge, the far end signs it with its node
//! signing key, and the controller verifies that signature against the
//! verifying key the far end presented. Only then is a record written.
//!
//! A pairing whose far-end identity cannot be proven is REFUSED. Recording an
//! unverified node is precisely how attribution dies quietly three steps later:
//! the receipts still carry a node id, the node id still looks plausible, and
//! nothing ever established that the machine holding that id was the machine
//! that did the work.
//!
//! Identity derives from the same Ed25519 material as the receipt attestation
//! — `key_id` is the SHA-256 of the verifying key, exactly as
//! [`crate::receipt::BackendIdentity`] defines it — so the crate has ONE
//! identity notion rather than two that can disagree.

use ed25519_dalek::ed25519::signature::Verifier;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey, ed25519::signature::Signer};
use serde::{Deserialize, Serialize};

use crate::error::{ExecError, Result};
use crate::receipt::sha256_public as sha256;

/// The domain separator the challenge signature covers. Without one, a
/// signature minted for a node challenge could be replayed as a receipt
/// attestation (or vice versa) since both are Ed25519 over bytes this crate
/// controls.
const CHALLENGE_DOMAIN: &[u8] = b"wayland.node-pairing.v1:";

/// Who a node is. Every field is either operator-chosen and validated, or
/// derived from key material — nothing here is a free-form claim the far end
/// can set to whatever it likes AND have believed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    /// Operator-facing name, unique within this controller's registry.
    pub node_id: String,
    /// Stable per-host discriminator. Distinguishes two nodes an operator
    /// happened to give confusingly similar names.
    pub machine_id: String,
    /// `linux` / `windows` / `macos` — advertised, and checked against reality
    /// only insofar as the far end reports its own `std::env::consts::OS`.
    pub os: String,
    /// The node-contract version this node speaks. See [`super::version`].
    pub contract_version: super::version::NodeContractVersion,
    /// SHA-256 of the node's Ed25519 verifying key. THIS is the identity;
    /// `node_id` is a label for humans.
    pub key_id: String,
}

impl NodeIdentity {
    /// Build an identity for THIS host from its signing key.
    pub fn local(node_id: &str, signing_key: &SigningKey) -> Result<Self> {
        crate::contract::validate_identifier("node_id", node_id)?;
        Ok(Self {
            node_id: node_id.to_string(),
            machine_id: local_machine_id(),
            os: std::env::consts::OS.to_string(),
            contract_version: super::version::NODE_CONTRACT_VERSION,
            key_id: sha256(signing_key.verifying_key().as_bytes()),
        })
    }

    /// Reject an identity that could not have come from real key material.
    pub fn validate(&self) -> Result<()> {
        crate::contract::validate_identifier("node_id", &self.node_id)?;
        crate::contract::validate_identifier("machine_id", &self.machine_id)?;
        crate::contract::validate_identifier("os", &self.os)?;
        crate::receipt::validate_sha256_public("node.key_id", &self.key_id)?;
        Ok(())
    }

    /// Does this identity match the key that is supposed to own it?
    pub fn matches_key(&self, key: &VerifyingKey) -> bool {
        self.key_id == sha256(key.as_bytes())
    }
}

/// A stable-enough discriminator for one physical host.
///
/// Hostname is used because it is the thing an operator actually recognises in
/// `node list`, and because a value that is stable but meaningless (a random
/// uuid) makes a mis-paired node impossible to spot by eye. It is a LABEL, not
/// a security boundary — `key_id` carries the security.
fn local_machine_id() -> String {
    let raw = std::env::var("WAYLAND_NODE_MACHINE_ID")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("COMPUTERNAME").ok().filter(|v| !v.is_empty()))
        // `HOSTNAME` is a SHELL variable, not an exported environment one, so a
        // non-login ssh invocation — which is exactly how a controller reaches
        // a node — sees none of the above and every host would report
        // `unknown-host`. Found by running the real binary over ssh; the env
        // lookup alone looked fine locally and was useless where it mattered.
        .or_else(read_hostname_file)
        .unwrap_or_else(|| "unknown-host".to_string());
    sanitize_identifier(&raw)
}

/// Unix hosts publish the hostname on disk regardless of shell environment.
fn read_hostname_file() -> Option<String> {
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
        if let Ok(contents) = std::fs::read_to_string(path) {
            let trimmed = contents.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Force an arbitrary host string into the crate's identifier shape, so a
/// hostname containing a dot or a space cannot fail validation later.
fn sanitize_identifier(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "unknown-host".to_string()
    } else {
        trimmed
    }
}

/// What the controller sends. The nonce is fresh per pairing attempt, so a
/// captured proof cannot be replayed into a later pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingChallenge {
    pub nonce: String,
    pub controller_key_id: String,
}

impl PairingChallenge {
    pub fn new(controller_key_id: &str) -> Self {
        let mut bytes = [0u8; 32];
        {
            use rand::RngCore as _;
            rand::rngs::OsRng.fill_bytes(&mut bytes);
        }
        Self {
            nonce: hex(&bytes),
            controller_key_id: controller_key_id.to_string(),
        }
    }
}

/// What the far end answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingProof {
    pub identity: NodeIdentity,
    /// Base64 of the far end's raw 32-byte Ed25519 verifying key.
    pub verifying_key_base64: String,
    /// Base64 of the signature over the domain-separated challenge.
    pub signature_base64: String,
    /// Echoed so a proof cannot be presented against a different challenge.
    pub nonce: String,
    /// What the far end says it can do. Advertisement is data, not authority —
    /// it is believed only about capability, never about identity.
    pub advertisement: super::capability::NodeAdvertisement,
}

fn challenge_message(challenge: &PairingChallenge) -> Vec<u8> {
    let mut msg = CHALLENGE_DOMAIN.to_vec();
    msg.extend_from_slice(challenge.nonce.as_bytes());
    msg.push(b'|');
    msg.extend_from_slice(challenge.controller_key_id.as_bytes());
    msg
}

/// Far-end half: sign the controller's challenge.
pub fn prove_challenge(
    signing_key: &SigningKey,
    identity: &NodeIdentity,
    challenge: &PairingChallenge,
    advertisement: super::capability::NodeAdvertisement,
) -> Result<PairingProof> {
    use base64::Engine as _;
    let base64 = base64::engine::general_purpose::STANDARD;
    identity.validate()?;
    if !identity.matches_key(&signing_key.verifying_key()) {
        return Err(ExecError::Receipt(
            "node identity key_id does not match the signing key presenting it".into(),
        ));
    }
    let signature: Signature = signing_key.sign(&challenge_message(challenge));
    Ok(PairingProof {
        identity: identity.clone(),
        verifying_key_base64: base64.encode(signing_key.verifying_key().as_bytes()),
        signature_base64: base64.encode(signature.to_bytes()),
        nonce: challenge.nonce.clone(),
        advertisement,
    })
}

/// Controller half: verify a proof, or refuse.
///
/// Returns the far end's verifying key on success. Four independent ways to
/// fail, each a real attack rather than a formality:
/// wrong nonce (replay), key that does not parse, identity whose `key_id` does
/// not match the presented key (a stolen label), and a signature that does not
/// verify (no possession of the private half).
pub fn verify_proof(challenge: &PairingChallenge, proof: &PairingProof) -> Result<VerifyingKey> {
    use base64::Engine as _;
    let base64 = base64::engine::general_purpose::STANDARD;

    if proof.nonce != challenge.nonce {
        return Err(ExecError::Receipt(format!(
            "pairing refused: proof answers nonce {} but the challenge was {}",
            short(&proof.nonce),
            short(&challenge.nonce)
        )));
    }
    proof.identity.validate()?;

    let key_bytes = base64
        .decode(&proof.verifying_key_base64)
        .map_err(|_| ExecError::Receipt("pairing refused: verifying key is not base64".into()))?;
    let key_arr: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| ExecError::Receipt("pairing refused: verifying key is not 32 bytes".into()))?;
    let key = VerifyingKey::from_bytes(&key_arr)
        .map_err(|_| ExecError::Receipt("pairing refused: verifying key is malformed".into()))?;

    if !proof.identity.matches_key(&key) {
        return Err(ExecError::Receipt(format!(
            "pairing refused: node '{}' presents key_id {} but its key hashes to {} \
             — the identity was not minted by the key presenting it",
            proof.identity.node_id,
            short(&proof.identity.key_id),
            short(&sha256(key.as_bytes()))
        )));
    }

    let sig_bytes = base64
        .decode(&proof.signature_base64)
        .map_err(|_| ExecError::Receipt("pairing refused: signature is not base64".into()))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|_| ExecError::Receipt("pairing refused: signature is malformed".into()))?;
    key.verify(&challenge_message(challenge), &signature)
        .map_err(|_| {
            ExecError::Receipt(format!(
                "pairing refused: node '{}' did not prove possession of its signing key",
                proof.identity.node_id
            ))
        })?;

    Ok(key)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub(crate) fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

/// Load or create this host's node signing seed, alongside the backend seeds.
pub fn load_or_create_node_seed() -> Result<[u8; 32]> {
    // Same atomic publish as the backend seeds, through the same helper --
    // this file carried a byte-identical copy of the torn-write hazard.
    let path = crate::registry::state_dir().join("keys").join("node.key");
    crate::backends::load_or_create_seed_at(&path, "node signing seed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::capability::NodeAdvertisement;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn identity(node_id: &str, k: &SigningKey) -> NodeIdentity {
        NodeIdentity {
            node_id: node_id.into(),
            machine_id: "test-host".into(),
            os: "linux".into(),
            contract_version: super::super::version::NODE_CONTRACT_VERSION,
            key_id: sha256(k.verifying_key().as_bytes()),
        }
    }

    #[test]
    fn a_genuine_proof_verifies() {
        let k = key(7);
        let id = identity("alpha", &k);
        let challenge = PairingChallenge::new("controller-key");
        let proof =
            prove_challenge(&k, &id, &challenge, NodeAdvertisement::empty("alpha")).unwrap();
        let verified = verify_proof(&challenge, &proof).unwrap();
        assert_eq!(verified.as_bytes(), k.verifying_key().as_bytes());
    }

    /// The core refusal: presenting someone else's label.
    #[test]
    fn an_identity_not_minted_by_the_presenting_key_is_refused() {
        let real = key(7);
        let impostor = key(8);
        // The impostor claims the real node's key_id.
        let stolen = identity("alpha", &real);
        let challenge = PairingChallenge::new("controller-key");
        // `prove_challenge` refuses to even build this...
        assert!(
            prove_challenge(
                &impostor,
                &stolen,
                &challenge,
                NodeAdvertisement::empty("alpha")
            )
            .is_err()
        );
        // ...and if one is assembled by hand, the controller refuses it.
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let sig = impostor.sign(&challenge_message(&challenge));
        let forged = PairingProof {
            identity: stolen,
            verifying_key_base64: b64.encode(impostor.verifying_key().as_bytes()),
            signature_base64: b64.encode(sig.to_bytes()),
            nonce: challenge.nonce.clone(),
            advertisement: NodeAdvertisement::empty("alpha"),
        };
        let err = verify_proof(&challenge, &forged).unwrap_err();
        assert!(
            err.to_string()
                .contains("not minted by the key presenting it"),
            "{err}"
        );
    }

    /// A captured proof cannot be replayed into a later pairing.
    #[test]
    fn a_proof_for_another_challenge_is_refused() {
        let k = key(7);
        let id = identity("alpha", &k);
        let first = PairingChallenge::new("controller-key");
        let proof = prove_challenge(&k, &id, &first, NodeAdvertisement::empty("alpha")).unwrap();
        let second = PairingChallenge::new("controller-key");
        assert_ne!(first.nonce, second.nonce, "nonces must be fresh");
        let err = verify_proof(&second, &proof).unwrap_err();
        assert!(err.to_string().contains("answers nonce"), "{err}");
    }

    /// Possession, not presentation: the right label with the wrong signature.
    #[test]
    fn a_signature_that_does_not_verify_is_refused() {
        let k = key(7);
        let id = identity("alpha", &k);
        let challenge = PairingChallenge::new("controller-key");
        let mut proof =
            prove_challenge(&k, &id, &challenge, NodeAdvertisement::empty("alpha")).unwrap();
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut sig = b64.decode(&proof.signature_base64).unwrap();
        sig[0] ^= 0x01;
        proof.signature_base64 = b64.encode(&sig);
        let err = verify_proof(&challenge, &proof).unwrap_err();
        assert!(
            err.to_string().contains("did not prove possession"),
            "{err}"
        );
    }

    /// Domain separation: a signature over the bare nonce is not a proof.
    #[test]
    fn a_signature_without_the_domain_separator_is_refused() {
        let k = key(7);
        let id = identity("alpha", &k);
        let challenge = PairingChallenge::new("controller-key");
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let sig = k.sign(challenge.nonce.as_bytes());
        let proof = PairingProof {
            identity: id,
            verifying_key_base64: b64.encode(k.verifying_key().as_bytes()),
            signature_base64: b64.encode(sig.to_bytes()),
            nonce: challenge.nonce.clone(),
            advertisement: NodeAdvertisement::empty("alpha"),
        };
        assert!(verify_proof(&challenge, &proof).is_err());
    }

    #[test]
    fn a_hostname_with_dots_still_produces_a_valid_machine_id() {
        assert_eq!(
            sanitize_identifier("Sean.Desktop.local"),
            "sean-desktop-local"
        );
        assert_eq!(sanitize_identifier("---"), "unknown-host");
    }
}
