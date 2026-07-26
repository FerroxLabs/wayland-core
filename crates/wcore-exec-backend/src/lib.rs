//! # wcore-exec-backend — the provider-neutral execution-backend contract
//!
//! F25-01 in one crate: capabilities, policy, secrets, artifact transfer,
//! resource limits, cancellation, attestation, receipts and lifecycle health,
//! with four reference backends — local, container, ssh and one hibernating
//! cloud machine — that all pass the SAME conformance harness.
//!
//! ## Why this is a new crate rather than an extension of `wcore-sandbox`
//!
//! Recorded here so it is not re-litigated during review. `wcore-sandbox` is
//! the CONTAINMENT crate whose posture Phase 20 and 20A spent months proving.
//! SSH and a credentialed cloud REST client are network REACH, not
//! containment; folding a credentialed network transport into that crate would
//! put new attack surface inside the one crate that currently carries the
//! anti-swap and hard-containment guarantees. The contract is also strictly
//! broader than `SandboxBackend` — it adds attestation, artifact transfer,
//! receipts and lifecycle health.
//!
//! This crate therefore COMPOSES `wcore-sandbox`; it does not replace,
//! wrap-around or bypass any containment predicate. Dependencies flow downward
//! only: `wcore-types` / `wcore-config` / `wcore-sandbox` / `wcore-egress`,
//! never `wcore-agent` and never `wcore-cli`.
//!
//! ## The receipt is not this crate's to invent
//!
//! `wcore-eval-scenarios/src/fixtures/remote_execution.rs` is a complete,
//! tested, deterministic remote-execution receipt contract, and its own module
//! doc says it does not implement an F25 production backend. It is therefore
//! the ORACLE this crate satisfies. [`receipt`] replicates its ordering,
//! content-addressing, single-terminal-event, shared-output-budget,
//! attestation and tamper rules, and
//! `wcore-eval-scenarios/tests/f25_production_receipt_oracle_conformance.rs`
//! checks a real production receipt against them.

pub mod backends;
pub mod conformance;
pub mod contract;
pub mod error;
pub mod policy;
pub mod receipt;
pub mod registry;

pub use contract::{
    Availability, BackendCapabilities, BackendKind, CleanupObservation, ExecutionBackend,
    ExecutionTask, Health, HibernationObservation, OrphanScan, ProbeBasis, ResourceBudget,
    ResourceKind, SecretChannel, WorkspaceFile,
};
pub use error::{ExecError, Result};
pub use policy::EffectivePolicy;
pub use receipt::{BackendIdentity, ExecutionReceipt, NormalizedBody, ReceiptBody};

use ed25519_dalek::VerifyingKey;

/// Base64 serde helper for the byte fields a task carries. JSON has no byte
/// type, and hex would double a workspace's size on the wire.
pub(crate) mod b64 {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        BASE64
            .decode(encoded.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

/// One constructed reference backend plus the material needed to verify what
/// it emits. Verification requires a CALLER-PINNED identity and key, so a
/// caller that only had the receipt could not establish identity from it.
pub struct ReferenceBackend {
    pub backend: Box<dyn ExecutionBackend>,
    pub identity: BackendIdentity,
    pub verifying_key: VerifyingKey,
}

/// Every reference backend this build carries, constructed but NOT probed.
/// Probing is the caller's decision because it costs real network time.
pub fn reference_backends(limits: ResourceBudget) -> Result<Vec<ReferenceBackend>> {
    let local = backends::local::LocalBackend::new(limits)?;
    let container = backends::container::ContainerBackend::new(limits)?;
    let ssh = backends::ssh::SshBackend::new(limits)?;
    let cloud = backends::cloud::CloudBackend::new(limits)?;
    Ok(vec![
        ReferenceBackend {
            identity: local.identity().clone(),
            verifying_key: local.verifying_key(),
            backend: Box::new(local),
        },
        ReferenceBackend {
            identity: container.identity().clone(),
            verifying_key: container.verifying_key(),
            backend: Box::new(container),
        },
        ReferenceBackend {
            identity: ssh.identity().clone(),
            verifying_key: ssh.verifying_key(),
            backend: Box::new(ssh),
        },
        ReferenceBackend {
            identity: cloud.identity().clone(),
            verifying_key: cloud.verifying_key(),
            backend: Box::new(cloud),
        },
    ])
}

/// Build one reference backend by name. Returns `None` for an unknown name so
/// the CLI can report the valid set rather than guessing.
pub fn reference_backend_named(
    name: &str,
    limits: ResourceBudget,
) -> Result<Option<ReferenceBackend>> {
    let found = reference_backends(limits)?
        .into_iter()
        .find(|b| b.backend.capabilities().backend_id == name);
    Ok(found)
}

/// Compare normalized receipt bodies across a set of runs of the SAME task.
///
/// Returns `(equivalent, differing_field_names)`. The differing names are the
/// point: a diff that reported only a boolean could not distinguish an
/// expected divergence from an unexpected one.
pub fn normalized_equivalence(receipts: &[ExecutionReceipt]) -> (bool, Vec<String>) {
    if receipts.len() < 2 {
        return (true, Vec::new());
    }
    let first = receipts[0].body.normalize();
    let mut differing = Vec::new();
    for receipt in &receipts[1..] {
        let other = receipt.body.normalize();
        if other == first {
            continue;
        }
        if other.task != first.task {
            differing.push("task".to_string());
        }
        if other.limits != first.limits {
            differing.push("limits".to_string());
        }
        if other.events != first.events {
            differing.push("events".to_string());
        }
        if other.artifact != first.artifact {
            differing.push("artifact".to_string());
        }
        if other.terminal != first.terminal {
            differing.push("terminal".to_string());
        }
        if other.secrets_exposed != first.secrets_exposed {
            differing.push("secrets_exposed".to_string());
        }
        if other.egress_decision != first.egress_decision {
            differing.push("egress_decision".to_string());
        }
        if other.protocol_version != first.protocol_version {
            differing.push("protocol_version".to_string());
        }
    }
    differing.sort();
    differing.dedup();
    (differing.is_empty(), differing)
}
