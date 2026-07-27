//! The production execution receipt.
//!
//! This receipt SATISFIES the F04 remote-execution oracle
//! (`wcore-eval-scenarios/src/fixtures/remote_execution.rs`) rather than
//! competing with it. Re-deriving a second receipt shape would be a parallel
//! authority, so every rule below is the oracle's rule:
//!
//! * events are strictly ordered from sequence 1 with no gaps;
//! * sequence 1 is `task_accepted` — UNLESS the task was denied before
//!   acceptance, in which case sequence 1 is `resource_denied` and there is no
//!   accepted event at all;
//! * workspace, input and artifact are content-addressed, never inlined;
//! * exactly ONE terminal event, and it is the last one;
//! * streamed output bytes and artifact bytes are charged against ONE shared
//!   output budget;
//! * the attestation is Ed25519 and its `key_id` is the SHA-256 of the pinned
//!   verifying key;
//! * a receipt whose body was altered, or whose backend identity is not the
//!   pinned one, fails verification.
//!
//! What this receipt adds over the fixture, because it describes a REAL run:
//! a transport, a timing block and a hibernation observation. All three are
//! DIVERGENT by construction and are excluded from the normalized body.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contract::{
    BackendKind, HibernationObservation, ResourceBudget, ResourceKind, hex, validate_identifier,
};
use crate::error::{ExecError, Result};

pub const RECEIPT_SCHEMA: &str = "wayland.execution-backend-receipt";
pub const RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const PROTOCOL_VERSION: u32 = 1;

pub const MAX_EVENTS: usize = 256;

/// Placeholder substituted for every divergent identity field when the body is
/// normalized. It is deliberately NOT the empty string: an empty string would
/// also be produced by a backend that simply failed to fill the field in, and
/// the diff must be able to tell those two cases apart.
pub const NORMALIZED_PLACEHOLDER: &str = "<normalized>";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendIdentity {
    pub backend_id: String,
    pub instance_id: String,
    pub version: String,
    /// SHA-256 of the pinned Ed25519 verifying key.
    pub key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transport {
    pub kind: BackendKind,
    /// Where the work actually ran, as the backend understands it — a host
    /// name, a container id, a machine id. NEVER a credential.
    pub endpoint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    pub started_unix_ms: u64,
    pub finished_unix_ms: u64,
    pub wall_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputChannel {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    TaskAccepted {
        task_id: String,
        backend_id: String,
        workspace_sha256: String,
        input_sha256: String,
    },
    Output {
        channel: OutputChannel,
        text_sha256: String,
        bytes: u64,
    },
    ArtifactPublished {
        name: String,
        sha256: String,
        bytes: u64,
    },
    Succeeded {
        artifact_sha256: String,
    },
    Failed {
        code: String,
    },
    TimedOut {
        limit_ms: u64,
    },
    Cancelled {
        reason: String,
    },
    Disconnected {
        reason: String,
    },
    ResourceDenied {
        resource: ResourceKind,
        requested: u64,
        limit: u64,
    },
}

impl EventKind {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            EventKind::Succeeded { .. }
                | EventKind::Failed { .. }
                | EventKind::TimedOut { .. }
                | EventKind::Cancelled { .. }
                | EventKind::Disconnected { .. }
                | EventKind::ResourceDenied { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptEvent {
    pub sequence: u64,
    pub event: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvidence {
    pub task_id: String,
    pub workspace_sha256: String,
    pub input_sha256: String,
    pub resources: ResourceBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEvidence {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalStatus {
    Success,
    Failure {
        code: String,
    },
    Timeout {
        limit_ms: u64,
    },
    Cancelled {
        reason: String,
    },
    Disconnected {
        reason: String,
    },
    ResourceDenied {
        resource: ResourceKind,
        requested: u64,
        limit: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptBody {
    pub protocol_version: u32,
    pub backend: BackendIdentity,
    /// F25-03: WHICH MACHINE, attested. This lives inside the signed body on
    /// purpose — altering it changes `body_sha256` and therefore breaks the
    /// attestation, exactly as altering `backend` does. A caller-settable
    /// `node_name` string beside the body would have been trivial and
    /// worthless: an attribution field a caller can set is not attribution.
    ///
    /// `Option` with `skip_serializing_if` so a receipt produced without a node
    /// serializes to the SAME bytes it did before this field existed — every
    /// receipt sealed by plan 25-01 still verifies unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<crate::node::attribution::NodeAttribution>,
    pub transport: Transport,
    pub task: TaskEvidence,
    pub limits: ResourceBudget,
    pub events_sha256: String,
    pub events: Vec<ReceiptEvent>,
    pub artifact: Option<ArtifactEvidence>,
    pub terminal: TerminalStatus,
    pub timing: Timing,
    pub hibernation: HibernationObservation,
    /// Names only — never values. Enforced by construction: the policy layer
    /// hands over names and the backend never sees a place to put a value.
    pub secrets_exposed: Vec<String>,
    pub egress_decision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    pub algorithm: String,
    pub key_id: String,
    pub signature_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionReceipt {
    pub schema: String,
    pub schema_version: u32,
    pub body_sha256: String,
    pub body: ReceiptBody,
    pub attestation: Attestation,
}

/// The comparable body: what MUST match across every backend running the same
/// task.
///
/// What is excluded, and why each exclusion is legitimate rather than
/// convenient — over-normalizing is the forbidden move here, because an
/// equivalence check that strips every field the backends actually differ on
/// proves nothing:
///   * `backend` identity — the four references are four different programs
///     with four different keys. This is what "which backend" MEANS.
///   * `transport` — the criterion asks four transports to agree, so the
///     transport itself cannot be part of the agreement.
///   * `timing` — a cold cloud boot is seconds and a local fork is
///     microseconds; requiring these to match would require lying.
///   * `hibernation` — only one backend kind has the surface at all.
///   * the `backend_id` INSIDE `task_accepted` — same reason as `backend`.
///
/// Everything else stays: task digests, resource budget, backend ceiling,
/// every event's ordering and content digests, the artifact digest, the
/// terminal status, the exposed-secret NAME set, and the egress decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedBody {
    pub protocol_version: u32,
    pub task: TaskEvidence,
    pub limits: ResourceBudget,
    pub events: Vec<ReceiptEvent>,
    pub artifact: Option<ArtifactEvidence>,
    pub terminal: TerminalStatus,
    pub secrets_exposed: Vec<String>,
    pub egress_decision: String,
}

impl ReceiptBody {
    pub fn normalize(&self) -> NormalizedBody {
        let events = self
            .events
            .iter()
            .map(|event| {
                let kind = match &event.event {
                    EventKind::TaskAccepted {
                        task_id,
                        backend_id: _,
                        workspace_sha256,
                        input_sha256,
                    } => EventKind::TaskAccepted {
                        task_id: task_id.clone(),
                        backend_id: NORMALIZED_PLACEHOLDER.to_string(),
                        workspace_sha256: workspace_sha256.clone(),
                        input_sha256: input_sha256.clone(),
                    },
                    other => other.clone(),
                };
                ReceiptEvent {
                    sequence: event.sequence,
                    event: kind,
                }
            })
            .collect();
        NormalizedBody {
            protocol_version: self.protocol_version,
            task: self.task.clone(),
            limits: self.limits,
            events,
            artifact: self.artifact.clone(),
            terminal: self.terminal.clone(),
            secrets_exposed: self.secrets_exposed.clone(),
            egress_decision: self.egress_decision.clone(),
        }
    }

    /// The fields deliberately left OUT of the normalized body, reported
    /// alongside it so a diff can classify each difference as
    /// expected-divergent rather than silently dropping it.
    pub fn divergent_fields(&self) -> Vec<(String, String)> {
        vec![
            ("backend.backend_id".into(), self.backend.backend_id.clone()),
            (
                "backend.instance_id".into(),
                self.backend.instance_id.clone(),
            ),
            ("backend.version".into(), self.backend.version.clone()),
            ("backend.key_id".into(), self.backend.key_id.clone()),
            (
                "transport.kind".into(),
                self.transport.kind.as_str().to_string(),
            ),
            ("transport.endpoint".into(), self.transport.endpoint.clone()),
            ("timing.wall_ms".into(), self.timing.wall_ms.to_string()),
            (
                "hibernation".into(),
                serde_json::to_string(&self.hibernation).unwrap_or_default(),
            ),
        ]
    }
}

/// Deterministic signing identity for one backend on one host.
pub struct ReceiptSigner {
    signing_key: SigningKey,
    key_id: String,
}

impl ReceiptSigner {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(&seed);
        let key_id = sha256(signing_key.verifying_key().as_bytes());
        Self {
            signing_key,
            key_id,
        }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn seal(&self, body: ReceiptBody) -> Result<ExecutionReceipt> {
        validate_receipt_semantics(&body)?;
        let body_bytes = serde_json::to_vec(&body)?;
        let body_sha256 = sha256(&body_bytes);
        let signature = self.signing_key.sign(&signature_message(&body_sha256));
        Ok(ExecutionReceipt {
            schema: RECEIPT_SCHEMA.to_string(),
            schema_version: RECEIPT_SCHEMA_VERSION,
            body_sha256,
            body,
            attestation: Attestation {
                algorithm: "ed25519".to_string(),
                key_id: self.key_id.clone(),
                signature_base64: BASE64.encode(signature.to_bytes()),
            },
        })
    }
}

impl ExecutionReceipt {
    /// Verify integrity and attestation against a CALLER-PINNED identity and
    /// key. Self-verification against a key carried only inside the receipt
    /// would not establish identity, so both are required — this is the
    /// oracle's rule and it is the reason a swapped backend is detectable.
    pub fn verify(&self, expected: &BackendIdentity, verifying_key: &VerifyingKey) -> Result<()> {
        if self.schema != RECEIPT_SCHEMA || self.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(ExecError::Receipt("unsupported receipt schema".into()));
        }
        if self.body.protocol_version != PROTOCOL_VERSION {
            return Err(ExecError::Receipt("unsupported protocol version".into()));
        }
        if &self.body.backend != expected {
            return Err(ExecError::Receipt("backend identity mismatch".into()));
        }
        let expected_key_id = sha256(verifying_key.as_bytes());
        if expected.key_id != expected_key_id
            || self.attestation.key_id != expected_key_id
            || self.attestation.algorithm != "ed25519"
        {
            return Err(ExecError::Receipt(
                "attestation key does not match the pinned backend identity".into(),
            ));
        }
        validate_receipt_semantics(&self.body)?;
        let body_bytes = serde_json::to_vec(&self.body)?;
        if sha256(&body_bytes) != self.body_sha256 {
            return Err(ExecError::Receipt("receipt body digest mismatch".into()));
        }
        let signature_bytes = BASE64
            .decode(&self.attestation.signature_base64)
            .map_err(|_| ExecError::Attestation)?;
        let signature =
            Signature::from_slice(&signature_bytes).map_err(|_| ExecError::Attestation)?;
        verifying_key
            .verify(&signature_message(&self.body_sha256), &signature)
            .map_err(|_| ExecError::Attestation)
    }

    /// Verification that does not require the caller to already hold the key —
    /// used by `wayland-core backend receipt verify` against a receipt file.
    /// It proves INTEGRITY and internal consistency; it deliberately cannot
    /// prove identity, and says so.
    pub fn verify_integrity_only(&self) -> Result<()> {
        validate_receipt_semantics(&self.body)?;
        let body_bytes = serde_json::to_vec(&self.body)?;
        if sha256(&body_bytes) != self.body_sha256 {
            return Err(ExecError::Receipt("receipt body digest mismatch".into()));
        }
        if self.body.backend.key_id != self.attestation.key_id {
            return Err(ExecError::Receipt(
                "attestation key_id does not match the backend identity in the body".into(),
            ));
        }
        Ok(())
    }
}

/// Every ordering, addressing and single-terminal rule the oracle enforces.
pub fn validate_receipt_semantics(body: &ReceiptBody) -> Result<()> {
    validate_identifier("backend_id", &body.backend.backend_id)?;
    validate_identifier("instance_id", &body.backend.instance_id)?;
    validate_identifier("version", &body.backend.version)?;
    validate_sha256("backend.key_id", &body.backend.key_id)?;
    if let Some(node) = &body.node {
        node.validate()?;
    }
    validate_identifier("task_id", &body.task.task_id)?;
    validate_sha256("workspace_sha256", &body.task.workspace_sha256)?;
    validate_sha256("input_sha256", &body.task.input_sha256)?;
    body.task.resources.validate()?;
    body.limits.validate()?;

    if body.events.is_empty() || body.events.len() > MAX_EVENTS + 3 {
        return Err(ExecError::Receipt("event count out of range".into()));
    }
    for (index, event) in body.events.iter().enumerate() {
        let expected = index as u64 + 1;
        if event.sequence != expected {
            return Err(ExecError::Receipt(format!(
                "event sequence gap: expected {expected}, observed {}",
                event.sequence
            )));
        }
    }

    let event_bytes = serde_json::to_vec(&body.events)?;
    if sha256(&event_bytes) != body.events_sha256 {
        return Err(ExecError::Receipt("event digest mismatch".into()));
    }

    // Exactly one terminal event, and it is last.
    let terminal_count = body
        .events
        .iter()
        .filter(|event| event.event.is_terminal())
        .count();
    if terminal_count != 1 {
        return Err(ExecError::Receipt(format!(
            "expected exactly one terminal event, found {terminal_count}"
        )));
    }
    if !body
        .events
        .last()
        .map(|event| event.event.is_terminal())
        .unwrap_or(false)
    {
        return Err(ExecError::Receipt(
            "the terminal event is not the last event".into(),
        ));
    }

    // Sequence 1 is task_accepted, UNLESS the run was denied before
    // acceptance — in which case there is no accepted event at all.
    match &body.events[0].event {
        EventKind::TaskAccepted {
            task_id,
            backend_id,
            workspace_sha256,
            input_sha256,
        } => {
            if task_id != &body.task.task_id
                || workspace_sha256 != &body.task.workspace_sha256
                || input_sha256 != &body.task.input_sha256
            {
                return Err(ExecError::Receipt(
                    "task_accepted does not agree with the receipt's own task evidence".into(),
                ));
            }
            // Only the normalized form may carry the placeholder.
            if backend_id != &body.backend.backend_id && backend_id != NORMALIZED_PLACEHOLDER {
                return Err(ExecError::Receipt(
                    "task_accepted names a different backend than the receipt".into(),
                ));
            }
        }
        EventKind::ResourceDenied { .. } => {
            if !matches!(body.terminal, TerminalStatus::ResourceDenied { .. }) {
                return Err(ExecError::Receipt(
                    "a pre-acceptance denial must terminate as resource_denied".into(),
                ));
            }
            if body.events.len() != 1 {
                return Err(ExecError::Receipt(
                    "a pre-acceptance denial emits exactly one event".into(),
                ));
            }
        }
        _ => {
            return Err(ExecError::Receipt(
                "the first event must be task_accepted or a pre-acceptance resource_denied".into(),
            ));
        }
    }

    // Terminal event and terminal status must agree.
    let last = &body.events[body.events.len() - 1].event;
    let agrees = matches!(
        (last, &body.terminal),
        (EventKind::Succeeded { .. }, TerminalStatus::Success)
            | (EventKind::Failed { .. }, TerminalStatus::Failure { .. })
            | (EventKind::TimedOut { .. }, TerminalStatus::Timeout { .. })
            | (
                EventKind::Cancelled { .. },
                TerminalStatus::Cancelled { .. }
            )
            | (
                EventKind::Disconnected { .. },
                TerminalStatus::Disconnected { .. }
            )
            | (
                EventKind::ResourceDenied { .. },
                TerminalStatus::ResourceDenied { .. }
            )
    );
    if !agrees {
        return Err(ExecError::Receipt(
            "the terminal event and the terminal status disagree".into(),
        ));
    }

    if matches!(body.terminal, TerminalStatus::Success) && body.artifact.is_none() {
        return Err(ExecError::Receipt(
            "a successful run must publish artifact evidence".into(),
        ));
    }
    if let Some(artifact) = &body.artifact {
        validate_sha256("artifact.sha256", &artifact.sha256)?;
    }

    // A secret VALUE must never reach a receipt. Names only, and a name that
    // looks like a value is refused.
    for name in &body.secrets_exposed {
        validate_identifier("secrets_exposed", name)?;
    }
    Ok(())
}

/// F25-03: the node layer needs the SAME digest the receipt identity uses, so
/// there is one definition of `key_id` rather than two that can disagree.
pub fn sha256_public(bytes: &[u8]) -> String {
    sha256(bytes)
}

/// F25-03: node identities are validated against the same rule as receipt
/// digests, for the same reason.
pub fn validate_sha256_public(field: &str, value: &str) -> Result<()> {
    validate_sha256(field, value)
}

/// F25-03: the events digest, exposed so a caller assembling a body outside
/// this module cannot compute it a second, subtly different way.
pub fn events_digest(events: &[ReceiptEvent]) -> String {
    sha256(&serde_json::to_vec(events).unwrap_or_default())
}

pub(crate) fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn signature_message(body_sha256: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(RECEIPT_SCHEMA.len() + 1 + body_sha256.len());
    message.extend_from_slice(RECEIPT_SCHEMA.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(body_sha256.as_bytes());
    message
}

fn validate_sha256(field: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
    {
        return Err(ExecError::Receipt(format!(
            "{field} is not a lowercase hex sha-256"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> ReceiptSigner {
        ReceiptSigner::from_seed([7u8; 32])
    }

    fn budget() -> ResourceBudget {
        ResourceBudget::new(1_000, 1 << 20, 5_000, 1 << 16).unwrap()
    }

    fn body_with(events: Vec<ReceiptEvent>, terminal: TerminalStatus) -> ReceiptBody {
        let signer = signer();
        let mut body = ReceiptBody {
            protocol_version: PROTOCOL_VERSION,
            node: None,
            backend: BackendIdentity {
                backend_id: "local".into(),
                instance_id: "inst-1".into(),
                version: "0.12.25".into(),
                key_id: signer.key_id().to_string(),
            },
            transport: Transport {
                kind: BackendKind::Local,
                endpoint: "localhost".into(),
            },
            task: TaskEvidence {
                task_id: "t-1".into(),
                workspace_sha256: sha256(b"ws"),
                input_sha256: sha256(b"in"),
                resources: budget(),
            },
            limits: budget(),
            events_sha256: String::new(),
            events,
            artifact: None,
            terminal,
            timing: Timing {
                started_unix_ms: 1,
                finished_unix_ms: 2,
                wall_ms: 1,
            },
            hibernation: HibernationObservation::NotApplicable,
            secrets_exposed: vec![],
            egress_decision: "deny-all".into(),
        };
        body.events_sha256 = sha256(&serde_json::to_vec(&body.events).unwrap());
        body
    }

    fn accepted_event() -> ReceiptEvent {
        ReceiptEvent {
            sequence: 1,
            event: EventKind::TaskAccepted {
                task_id: "t-1".into(),
                backend_id: "local".into(),
                workspace_sha256: sha256(b"ws"),
                input_sha256: sha256(b"in"),
            },
        }
    }

    fn success_body() -> ReceiptBody {
        let artifact_sha = sha256(b"artifact");
        let mut body = body_with(
            vec![
                accepted_event(),
                ReceiptEvent {
                    sequence: 2,
                    event: EventKind::ArtifactPublished {
                        name: "out.bin".into(),
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
            ],
            TerminalStatus::Success,
        );
        body.artifact = Some(ArtifactEvidence {
            name: "out.bin".into(),
            sha256: artifact_sha,
            bytes: 8,
        });
        body.events_sha256 = sha256(&serde_json::to_vec(&body.events).unwrap());
        body
    }

    #[test]
    fn a_sealed_receipt_verifies_against_its_pinned_identity() {
        let signer = signer();
        let receipt = signer.seal(success_body()).unwrap();
        receipt
            .verify(&receipt.body.backend.clone(), &signer.verifying_key())
            .unwrap();
    }

    #[test]
    fn an_altered_body_fails_verification() {
        let signer = signer();
        let mut receipt = signer.seal(success_body()).unwrap();
        receipt.body.task.task_id = "t-2".into();
        let identity = receipt.body.backend.clone();
        let err = receipt.verify(&identity, &signer.verifying_key());
        assert!(err.is_err(), "a tampered body must not verify");
    }

    #[test]
    fn an_unpinned_backend_identity_is_rejected() {
        let signer = signer();
        let receipt = signer.seal(success_body()).unwrap();
        let mut wrong = receipt.body.backend.clone();
        wrong.backend_id = "container".into();
        assert!(receipt.verify(&wrong, &signer.verifying_key()).is_err());
    }

    #[test]
    fn a_gap_in_the_event_sequence_is_rejected() {
        let mut body = success_body();
        body.events[2].sequence = 4;
        body.events_sha256 = sha256(&serde_json::to_vec(&body.events).unwrap());
        assert!(validate_receipt_semantics(&body).is_err());
    }

    #[test]
    fn two_terminal_events_are_rejected() {
        let mut body = success_body();
        let artifact_sha = sha256(b"artifact");
        body.events.push(ReceiptEvent {
            sequence: 4,
            event: EventKind::Succeeded {
                artifact_sha256: artifact_sha,
            },
        });
        body.events_sha256 = sha256(&serde_json::to_vec(&body.events).unwrap());
        assert!(validate_receipt_semantics(&body).is_err());
    }

    #[test]
    fn a_pre_acceptance_denial_carries_no_accepted_event() {
        let body = body_with(
            vec![ReceiptEvent {
                sequence: 1,
                event: EventKind::ResourceDenied {
                    resource: ResourceKind::MemoryBytes,
                    requested: 1 << 40,
                    limit: 1 << 20,
                },
            }],
            TerminalStatus::ResourceDenied {
                resource: ResourceKind::MemoryBytes,
                requested: 1 << 40,
                limit: 1 << 20,
            },
        );
        validate_receipt_semantics(&body).unwrap();
        assert!(!matches!(
            body.events[0].event,
            EventKind::TaskAccepted { .. }
        ));
    }

    #[test]
    fn normalization_keeps_every_field_two_backends_must_agree_on() {
        let body = success_body();
        let normalized = body.normalize();
        // Digests, budget, artifact and terminal survive normalization —
        // stripping any of them would make the equivalence claim vacuous.
        assert_eq!(normalized.task.workspace_sha256, body.task.workspace_sha256);
        assert_eq!(normalized.task.input_sha256, body.task.input_sha256);
        assert_eq!(normalized.limits, body.limits);
        assert_eq!(normalized.artifact, body.artifact);
        assert_eq!(normalized.terminal, body.terminal);
        assert_eq!(normalized.events.len(), body.events.len());
    }

    #[test]
    fn normalization_elides_only_identity_and_reports_it_as_divergent() {
        let mut local = success_body();
        let mut remote = success_body();
        remote.backend.backend_id = "ssh".into();
        remote.backend.instance_id = "inst-2".into();
        remote.transport = Transport {
            kind: BackendKind::Ssh,
            endpoint: "hetzner-dsm".into(),
        };
        remote.timing.wall_ms = 9_999;
        if let EventKind::TaskAccepted { backend_id, .. } = &mut remote.events[0].event {
            *backend_id = "ssh".into();
        }
        remote.events_sha256 = sha256(&serde_json::to_vec(&remote.events).unwrap());
        local.events_sha256 = sha256(&serde_json::to_vec(&local.events).unwrap());

        assert_eq!(
            local.normalize(),
            remote.normalize(),
            "two backends running the same task must normalize equal"
        );
        let divergent: Vec<String> = remote
            .divergent_fields()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(divergent.contains(&"transport.kind".to_string()));
        assert!(divergent.contains(&"backend.backend_id".to_string()));
        assert!(divergent.contains(&"timing.wall_ms".to_string()));
    }

    #[test]
    fn normalization_does_not_hide_a_real_disagreement() {
        let local = success_body();
        let mut tampered = success_body();
        tampered.task.input_sha256 = sha256(b"different input");
        if let EventKind::TaskAccepted { input_sha256, .. } = &mut tampered.events[0].event {
            *input_sha256 = sha256(b"different input");
        }
        tampered.events_sha256 = sha256(&serde_json::to_vec(&tampered.events).unwrap());
        assert_ne!(
            local.normalize(),
            tampered.normalize(),
            "normalization that hides a different input digest would prove nothing"
        );
    }
}
