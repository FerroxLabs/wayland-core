//! Deterministic, fixture-only remote execution contract.
//!
//! This module is an evaluation oracle for F04. It does not implement or
//! advertise an F25 production backend, transport, secret channel, container,
//! SSH connection, or cloud executor. Every execution happens in memory and
//! every attestation is explicitly fixture-only.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const FIXTURE_PROTOCOL_VERSION: u32 = 1;
const RECEIPT_SCHEMA: &str = "wayland.remote-execution-fixture-receipt";
const RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_REASON_BYTES: usize = 1024;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;
const MAX_EVENTS: usize = 256;
const MAX_EVENT_TEXT_BYTES: usize = 64 * 1024;
const MAX_TOTAL_EVENT_TEXT_BYTES: usize = 1024 * 1024;

/// Identity asserted by the fixture receipt. `key_id` is the SHA-256 of the
/// pinned Ed25519 verifying key, not a production backend credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FixtureBackendIdentity {
    pub backend_id: String,
    pub instance_id: String,
    pub version: String,
    pub key_id: String,
}

/// Resource request and fixture limit. Values are declarative; the fixture
/// performs no operating-system resource allocation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceBudget {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub wall_time_ms: u64,
    pub output_bytes: u64,
}

impl ResourceBudget {
    pub fn new(
        cpu_millis: u64,
        memory_bytes: u64,
        wall_time_ms: u64,
        output_bytes: u64,
    ) -> Result<Self, RemoteFixtureError> {
        let budget = Self {
            cpu_millis,
            memory_bytes,
            wall_time_ms,
            output_bytes,
        };
        budget.validate()?;
        Ok(budget)
    }

    fn validate(self) -> Result<(), RemoteFixtureError> {
        if self.cpu_millis == 0
            || self.memory_bytes == 0
            || self.wall_time_ms == 0
            || self.output_bytes == 0
        {
            return Err(RemoteFixtureError::InvalidResourceBudget);
        }
        Ok(())
    }
}

/// An accepted fixture task. Only digests enter the receipt; raw input bytes
/// are never copied into events or attestation material.
#[derive(Debug, Clone)]
pub struct RemoteTask {
    task_id: String,
    workspace_sha256: String,
    input: Vec<u8>,
    resources: ResourceBudget,
}

impl RemoteTask {
    pub fn new(
        task_id: impl Into<String>,
        workspace_sha256: impl Into<String>,
        input: impl Into<Vec<u8>>,
        resources: ResourceBudget,
    ) -> Result<Self, RemoteFixtureError> {
        let task_id = task_id.into();
        validate_identifier("task_id", &task_id)?;
        let workspace_sha256 = workspace_sha256.into();
        validate_sha256("workspace_sha256", &workspace_sha256)?;
        let input = input.into();
        if input.len() > MAX_INPUT_BYTES {
            return Err(RemoteFixtureError::InputTooLarge {
                limit: MAX_INPUT_BYTES,
            });
        }
        resources.validate()?;
        Ok(Self {
            task_id,
            workspace_sha256,
            input,
            resources,
        })
    }

    pub fn input_sha256(&self) -> String {
        sha256(&self.input)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputChannel {
    Stdout,
    Stderr,
}

/// A non-terminal event supplied by a fixture script. Sequence 1 is reserved
/// for `task_accepted`; scripted output therefore begins at sequence 2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScriptedOutputEvent {
    pub sequence: u64,
    pub channel: OutputChannel,
    pub text: String,
}

impl ScriptedOutputEvent {
    pub fn new(sequence: u64, channel: OutputChannel, text: impl Into<String>) -> Self {
        Self {
            sequence,
            channel,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FixtureArtifact {
    name: String,
    bytes: Vec<u8>,
}

impl FixtureArtifact {
    pub fn new(
        name: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, RemoteFixtureError> {
        let name = name.into();
        validate_artifact_name(&name)?;
        let bytes = bytes.into();
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(RemoteFixtureError::ArtifactTooLarge {
                limit: MAX_ARTIFACT_BYTES,
            });
        }
        Ok(Self { name, bytes })
    }
}

#[derive(Debug, Clone)]
pub enum ScriptedOutcome {
    Success { artifact: FixtureArtifact },
    Failure { code: String },
    Timeout,
    Cancelled { reason: String },
    Disconnected { reason: String },
}

impl ScriptedOutcome {
    pub fn success(artifact: FixtureArtifact) -> Self {
        Self::Success { artifact }
    }

    pub fn failure(code: impl Into<String>) -> Self {
        Self::Failure { code: code.into() }
    }

    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self::Cancelled {
            reason: reason.into(),
        }
    }

    pub fn disconnected(reason: impl Into<String>) -> Self {
        Self::Disconnected {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteExecutionScript {
    pub events: Vec<ScriptedOutputEvent>,
    pub outcome: ScriptedOutcome,
}

impl RemoteExecutionScript {
    pub fn new(
        events: impl IntoIterator<Item = ScriptedOutputEvent>,
        outcome: ScriptedOutcome,
    ) -> Self {
        Self {
            events: events.into_iter().collect(),
            outcome,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    CpuMillis,
    MemoryBytes,
    WallTimeMs,
    OutputBytes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteExecutionEventKind {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecutionEvent {
    pub sequence: u64,
    pub event: RemoteExecutionEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskEvidence {
    pub task_id: String,
    pub workspace_sha256: String,
    pub input_sha256: String,
    pub resources: ResourceBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactEvidence {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecutionReceiptBody {
    pub fixture_protocol_version: u32,
    pub backend: FixtureBackendIdentity,
    pub task: TaskEvidence,
    pub limits: ResourceBudget,
    pub events_sha256: String,
    pub events: Vec<RemoteExecutionEvent>,
    pub artifact: Option<ArtifactEvidence>,
    pub terminal: TerminalStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureAuthority {
    FixtureOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FixtureAttestation {
    pub authority: FixtureAuthority,
    pub algorithm: String,
    pub key_id: String,
    pub signature_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecutionReceipt {
    pub schema: String,
    pub schema_version: u32,
    pub body_sha256: String,
    pub body: RemoteExecutionReceiptBody,
    pub attestation: FixtureAttestation,
}

impl RemoteExecutionReceipt {
    /// Verify integrity and fixture attestation against a caller-pinned backend
    /// identity and key. Self-verification against a key carried only in the
    /// receipt would not establish identity, so this API requires both.
    pub fn verify(
        &self,
        expected_backend: &FixtureBackendIdentity,
        verifying_key: &VerifyingKey,
    ) -> Result<(), RemoteFixtureError> {
        if self.schema != RECEIPT_SCHEMA || self.schema_version != RECEIPT_SCHEMA_VERSION {
            return Err(RemoteFixtureError::UnsupportedReceiptSchema);
        }
        if self.body.fixture_protocol_version != FIXTURE_PROTOCOL_VERSION {
            return Err(RemoteFixtureError::UnsupportedFixtureProtocol);
        }
        if &self.body.backend != expected_backend {
            return Err(RemoteFixtureError::BackendIdentityMismatch);
        }
        let expected_key_id = sha256(verifying_key.as_bytes());
        if expected_backend.key_id != expected_key_id
            || self.attestation.key_id != expected_key_id
            || self.attestation.authority != FixtureAuthority::FixtureOnly
            || self.attestation.algorithm != "ed25519"
        {
            return Err(RemoteFixtureError::BackendIdentityMismatch);
        }
        validate_receipt_semantics(&self.body)?;
        let body = serde_json::to_vec(&self.body)
            .map_err(|error| RemoteFixtureError::Serialize(error.to_string()))?;
        if sha256(&body) != self.body_sha256 {
            return Err(RemoteFixtureError::ReceiptDigestMismatch);
        }
        let signature_bytes = BASE64
            .decode(&self.attestation.signature_base64)
            .map_err(|_| RemoteFixtureError::MalformedSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| RemoteFixtureError::MalformedSignature)?;
        verifying_key
            .verify(&signature_message(&self.body_sha256), &signature)
            .map_err(|_| RemoteFixtureError::InvalidSignature)
    }
}

/// In-memory F04 oracle. It deliberately owns no transport or process.
pub struct RemoteExecutionFixture {
    identity: FixtureBackendIdentity,
    limits: ResourceBudget,
    signing_key: SigningKey,
}

impl RemoteExecutionFixture {
    pub fn new(
        backend_id: impl Into<String>,
        instance_id: impl Into<String>,
        version: impl Into<String>,
        limits: ResourceBudget,
        signing_seed: [u8; 32],
    ) -> Result<Self, RemoteFixtureError> {
        let backend_id = backend_id.into();
        let instance_id = instance_id.into();
        let version = version.into();
        validate_identifier("backend_id", &backend_id)?;
        validate_identifier("instance_id", &instance_id)?;
        validate_identifier("version", &version)?;
        limits.validate()?;
        let signing_key = SigningKey::from_bytes(&signing_seed);
        let key_id = sha256(signing_key.verifying_key().as_bytes());
        Ok(Self {
            identity: FixtureBackendIdentity {
                backend_id,
                instance_id,
                version,
                key_id,
            },
            limits,
            signing_key,
        })
    }

    pub fn identity(&self) -> &FixtureBackendIdentity {
        &self.identity
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn execute(
        &self,
        task: &RemoteTask,
        script: &RemoteExecutionScript,
    ) -> Result<RemoteExecutionReceipt, RemoteFixtureError> {
        if let Some((resource, requested, limit)) = denied_resource(task.resources, self.limits) {
            let terminal = TerminalStatus::ResourceDenied {
                resource,
                requested,
                limit,
            };
            return self.attest(
                task,
                vec![RemoteExecutionEvent {
                    sequence: 1,
                    event: RemoteExecutionEventKind::ResourceDenied {
                        resource,
                        requested,
                        limit,
                    },
                }],
                None,
                terminal,
            );
        }

        validate_script(script)?;
        let input_sha256 = task.input_sha256();
        let mut events = Vec::with_capacity(script.events.len().saturating_add(3));
        events.push(RemoteExecutionEvent {
            sequence: 1,
            event: RemoteExecutionEventKind::TaskAccepted {
                task_id: task.task_id.clone(),
                backend_id: self.identity.backend_id.clone(),
                workspace_sha256: task.workspace_sha256.clone(),
                input_sha256,
            },
        });
        for scripted in &script.events {
            events.push(RemoteExecutionEvent {
                sequence: scripted.sequence,
                event: RemoteExecutionEventKind::Output {
                    channel: scripted.channel,
                    text_sha256: sha256(scripted.text.as_bytes()),
                    bytes: scripted.text.len() as u64,
                },
            });
        }

        let next_sequence = events.len() as u64 + 1;
        let (artifact, terminal) = match &script.outcome {
            ScriptedOutcome::Success { artifact } => {
                let artifact_bytes = artifact.bytes.len() as u64;
                if artifact_bytes > task.resources.output_bytes {
                    let terminal = TerminalStatus::ResourceDenied {
                        resource: ResourceKind::OutputBytes,
                        requested: artifact_bytes,
                        limit: task.resources.output_bytes,
                    };
                    events.push(RemoteExecutionEvent {
                        sequence: next_sequence,
                        event: RemoteExecutionEventKind::ResourceDenied {
                            resource: ResourceKind::OutputBytes,
                            requested: artifact_bytes,
                            limit: task.resources.output_bytes,
                        },
                    });
                    return self.attest(task, events, None, terminal);
                }
                let evidence = ArtifactEvidence {
                    name: artifact.name.clone(),
                    sha256: sha256(&artifact.bytes),
                    bytes: artifact_bytes,
                };
                events.push(RemoteExecutionEvent {
                    sequence: next_sequence,
                    event: RemoteExecutionEventKind::ArtifactPublished {
                        name: evidence.name.clone(),
                        sha256: evidence.sha256.clone(),
                        bytes: evidence.bytes,
                    },
                });
                events.push(RemoteExecutionEvent {
                    sequence: next_sequence + 1,
                    event: RemoteExecutionEventKind::Succeeded {
                        artifact_sha256: evidence.sha256.clone(),
                    },
                });
                (Some(evidence), TerminalStatus::Success)
            }
            ScriptedOutcome::Failure { code } => {
                events.push(RemoteExecutionEvent {
                    sequence: next_sequence,
                    event: RemoteExecutionEventKind::Failed { code: code.clone() },
                });
                (None, TerminalStatus::Failure { code: code.clone() })
            }
            ScriptedOutcome::Timeout => {
                events.push(RemoteExecutionEvent {
                    sequence: next_sequence,
                    event: RemoteExecutionEventKind::TimedOut {
                        limit_ms: task.resources.wall_time_ms,
                    },
                });
                (
                    None,
                    TerminalStatus::Timeout {
                        limit_ms: task.resources.wall_time_ms,
                    },
                )
            }
            ScriptedOutcome::Cancelled { reason } => {
                events.push(RemoteExecutionEvent {
                    sequence: next_sequence,
                    event: RemoteExecutionEventKind::Cancelled {
                        reason: reason.clone(),
                    },
                });
                (
                    None,
                    TerminalStatus::Cancelled {
                        reason: reason.clone(),
                    },
                )
            }
            ScriptedOutcome::Disconnected { reason } => {
                events.push(RemoteExecutionEvent {
                    sequence: next_sequence,
                    event: RemoteExecutionEventKind::Disconnected {
                        reason: reason.clone(),
                    },
                });
                (
                    None,
                    TerminalStatus::Disconnected {
                        reason: reason.clone(),
                    },
                )
            }
        };
        self.attest(task, events, artifact, terminal)
    }

    fn attest(
        &self,
        task: &RemoteTask,
        events: Vec<RemoteExecutionEvent>,
        artifact: Option<ArtifactEvidence>,
        terminal: TerminalStatus,
    ) -> Result<RemoteExecutionReceipt, RemoteFixtureError> {
        let events_bytes = serde_json::to_vec(&events)
            .map_err(|error| RemoteFixtureError::Serialize(error.to_string()))?;
        let body = RemoteExecutionReceiptBody {
            fixture_protocol_version: FIXTURE_PROTOCOL_VERSION,
            backend: self.identity.clone(),
            task: TaskEvidence {
                task_id: task.task_id.clone(),
                workspace_sha256: task.workspace_sha256.clone(),
                input_sha256: task.input_sha256(),
                resources: task.resources,
            },
            limits: self.limits,
            events_sha256: sha256(&events_bytes),
            events,
            artifact,
            terminal,
        };
        validate_receipt_semantics(&body)?;
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|error| RemoteFixtureError::Serialize(error.to_string()))?;
        let body_sha256 = sha256(&body_bytes);
        let signature = self.signing_key.sign(&signature_message(&body_sha256));
        Ok(RemoteExecutionReceipt {
            schema: RECEIPT_SCHEMA.to_string(),
            schema_version: RECEIPT_SCHEMA_VERSION,
            body_sha256,
            body,
            attestation: FixtureAttestation {
                authority: FixtureAuthority::FixtureOnly,
                algorithm: "ed25519".to_string(),
                key_id: self.identity.key_id.clone(),
                signature_base64: BASE64.encode(signature.to_bytes()),
            },
        })
    }
}

fn validate_script(script: &RemoteExecutionScript) -> Result<(), RemoteFixtureError> {
    if script.events.len() > MAX_EVENTS {
        return Err(RemoteFixtureError::TooManyEvents { limit: MAX_EVENTS });
    }
    let mut expected = 2_u64;
    let mut total_text = 0_usize;
    for event in &script.events {
        if event.sequence != expected {
            return Err(RemoteFixtureError::InvalidEventSequence {
                expected,
                observed: event.sequence,
            });
        }
        expected += 1;
        if event.text.len() > MAX_EVENT_TEXT_BYTES {
            return Err(RemoteFixtureError::EventTextTooLarge {
                limit: MAX_EVENT_TEXT_BYTES,
            });
        }
        total_text = total_text.saturating_add(event.text.len());
        if total_text > MAX_TOTAL_EVENT_TEXT_BYTES {
            return Err(RemoteFixtureError::TotalEventTextTooLarge {
                limit: MAX_TOTAL_EVENT_TEXT_BYTES,
            });
        }
    }
    match &script.outcome {
        ScriptedOutcome::Success { artifact } => {
            validate_artifact_name(&artifact.name)?;
            if artifact.bytes.len() > MAX_ARTIFACT_BYTES {
                return Err(RemoteFixtureError::ArtifactTooLarge {
                    limit: MAX_ARTIFACT_BYTES,
                });
            }
        }
        ScriptedOutcome::Failure { code } => validate_identifier("failure_code", code)?,
        ScriptedOutcome::Cancelled { reason } | ScriptedOutcome::Disconnected { reason } => {
            validate_reason(reason)?;
        }
        ScriptedOutcome::Timeout => {}
    }
    Ok(())
}

fn validate_receipt_semantics(body: &RemoteExecutionReceiptBody) -> Result<(), RemoteFixtureError> {
    validate_identifier("backend_id", &body.backend.backend_id)?;
    validate_identifier("instance_id", &body.backend.instance_id)?;
    validate_identifier("version", &body.backend.version)?;
    validate_sha256("backend.key_id", &body.backend.key_id)?;
    validate_identifier("task_id", &body.task.task_id)?;
    validate_sha256("workspace_sha256", &body.task.workspace_sha256)?;
    validate_sha256("input_sha256", &body.task.input_sha256)?;
    body.task.resources.validate()?;
    body.limits.validate()?;
    if body.events.is_empty() || body.events.len() > MAX_EVENTS + 3 {
        return Err(RemoteFixtureError::InvalidReceiptSemantics);
    }
    for (index, event) in body.events.iter().enumerate() {
        let expected = index as u64 + 1;
        if event.sequence != expected {
            return Err(RemoteFixtureError::InvalidEventSequence {
                expected,
                observed: event.sequence,
            });
        }
    }
    let event_bytes = serde_json::to_vec(&body.events)
        .map_err(|error| RemoteFixtureError::Serialize(error.to_string()))?;
    if sha256(&event_bytes) != body.events_sha256 {
        return Err(RemoteFixtureError::EventDigestMismatch);
    }

    let first = &body.events[0].event;
    match (&body.terminal, first) {
        (
            TerminalStatus::ResourceDenied {
                resource,
                requested,
                limit,
            },
            RemoteExecutionEventKind::ResourceDenied {
                resource: event_resource,
                requested: event_requested,
                limit: event_limit,
            },
        ) if body.events.len() == 1
            && resource == event_resource
            && requested == event_requested
            && limit == event_limit
            && denied_resource(body.task.resources, body.limits)
                == Some((*resource, *requested, *limit)) => {}
        (
            _,
            RemoteExecutionEventKind::TaskAccepted {
                task_id,
                backend_id,
                workspace_sha256,
                input_sha256,
            },
        ) if task_id == &body.task.task_id
            && backend_id == &body.backend.backend_id
            && workspace_sha256 == &body.task.workspace_sha256
            && input_sha256 == &body.task.input_sha256
            && denied_resource(body.task.resources, body.limits).is_none() => {}
        _ => return Err(RemoteFixtureError::InvalidReceiptSemantics),
    }
    if let (
        TerminalStatus::ResourceDenied {
            resource,
            requested,
            limit,
        },
        RemoteExecutionEventKind::TaskAccepted { .. },
    ) = (&body.terminal, first)
    {
        if resource != &ResourceKind::OutputBytes
            || requested <= limit
            || limit != &body.task.resources.output_bytes
        {
            return Err(RemoteFixtureError::InvalidReceiptSemantics);
        }
    }

    let last = &body.events[body.events.len() - 1].event;
    if !terminal_matches_event(&body.terminal, last) {
        return Err(RemoteFixtureError::InvalidReceiptSemantics);
    }
    match (&body.terminal, &body.artifact) {
        (TerminalStatus::Success, Some(artifact)) => {
            validate_artifact_name(&artifact.name)?;
            validate_sha256("artifact.sha256", &artifact.sha256)?;
            if artifact.bytes > MAX_ARTIFACT_BYTES as u64
                || artifact.bytes > body.task.resources.output_bytes
            {
                return Err(RemoteFixtureError::InvalidReceiptSemantics);
            }
            let published = body.events.iter().rev().nth(1).map(|event| &event.event);
            if !matches!(
                published,
                Some(RemoteExecutionEventKind::ArtifactPublished { name, sha256, bytes })
                    if name == &artifact.name
                        && sha256 == &artifact.sha256
                        && bytes == &artifact.bytes
            ) {
                return Err(RemoteFixtureError::InvalidReceiptSemantics);
            }
            if !matches!(
                last,
                RemoteExecutionEventKind::Succeeded { artifact_sha256 }
                    if artifact_sha256 == &artifact.sha256
            ) {
                return Err(RemoteFixtureError::InvalidReceiptSemantics);
            }
        }
        (TerminalStatus::Success, None) | (_, Some(_)) => {
            return Err(RemoteFixtureError::InvalidReceiptSemantics);
        }
        (_, None) => {}
    }
    let intermediate_start = usize::from(matches!(
        first,
        RemoteExecutionEventKind::TaskAccepted { .. }
    ));
    let intermediate_end = match &body.terminal {
        TerminalStatus::Success => body.events.len().saturating_sub(2),
        _ => body.events.len().saturating_sub(1),
    };
    if intermediate_start > intermediate_end
        || body.events[intermediate_start..intermediate_end]
            .iter()
            .any(|event| !matches!(&event.event, RemoteExecutionEventKind::Output { .. }))
    {
        return Err(RemoteFixtureError::InvalidReceiptSemantics);
    }
    let mut total_text = 0_u64;
    for event in &body.events {
        if let RemoteExecutionEventKind::Output {
            text_sha256, bytes, ..
        } = &event.event
        {
            validate_sha256("event.text_sha256", text_sha256)?;
            if *bytes > MAX_EVENT_TEXT_BYTES as u64 {
                return Err(RemoteFixtureError::EventTextTooLarge {
                    limit: MAX_EVENT_TEXT_BYTES,
                });
            }
            total_text = total_text.saturating_add(*bytes);
        }
    }
    if total_text > MAX_TOTAL_EVENT_TEXT_BYTES as u64 {
        return Err(RemoteFixtureError::TotalEventTextTooLarge {
            limit: MAX_TOTAL_EVENT_TEXT_BYTES,
        });
    }
    match &body.terminal {
        TerminalStatus::Failure { code } => validate_identifier("failure_code", code)?,
        TerminalStatus::Timeout { limit_ms } if *limit_ms != body.task.resources.wall_time_ms => {
            return Err(RemoteFixtureError::InvalidReceiptSemantics);
        }
        TerminalStatus::Cancelled { reason } | TerminalStatus::Disconnected { reason } => {
            validate_reason(reason)?;
        }
        TerminalStatus::Success
        | TerminalStatus::Timeout { .. }
        | TerminalStatus::ResourceDenied { .. } => {}
    }
    Ok(())
}

fn terminal_matches_event(terminal: &TerminalStatus, event: &RemoteExecutionEventKind) -> bool {
    match (terminal, event) {
        (TerminalStatus::Success, RemoteExecutionEventKind::Succeeded { artifact_sha256 }) => {
            !artifact_sha256.is_empty()
        }
        (
            TerminalStatus::Failure { code },
            RemoteExecutionEventKind::Failed { code: event_code },
        ) => code == event_code,
        (
            TerminalStatus::Timeout { limit_ms },
            RemoteExecutionEventKind::TimedOut {
                limit_ms: event_limit,
            },
        ) => limit_ms == event_limit,
        (
            TerminalStatus::Cancelled { reason },
            RemoteExecutionEventKind::Cancelled {
                reason: event_reason,
            },
        ) => reason == event_reason,
        (
            TerminalStatus::Disconnected { reason },
            RemoteExecutionEventKind::Disconnected {
                reason: event_reason,
            },
        ) => reason == event_reason,
        (
            TerminalStatus::ResourceDenied {
                resource,
                requested,
                limit,
            },
            RemoteExecutionEventKind::ResourceDenied {
                resource: event_resource,
                requested: event_requested,
                limit: event_limit,
            },
        ) => resource == event_resource && requested == event_requested && limit == event_limit,
        _ => false,
    }
}

fn denied_resource(
    requested: ResourceBudget,
    limit: ResourceBudget,
) -> Option<(ResourceKind, u64, u64)> {
    [
        (
            ResourceKind::CpuMillis,
            requested.cpu_millis,
            limit.cpu_millis,
        ),
        (
            ResourceKind::MemoryBytes,
            requested.memory_bytes,
            limit.memory_bytes,
        ),
        (
            ResourceKind::WallTimeMs,
            requested.wall_time_ms,
            limit.wall_time_ms,
        ),
        (
            ResourceKind::OutputBytes,
            requested.output_bytes,
            limit.output_bytes,
        ),
    ]
    .into_iter()
    .find(|(_, requested, limit)| requested > limit)
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), RemoteFixtureError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(RemoteFixtureError::InvalidIdentifier { field });
    }
    Ok(())
}

fn validate_reason(reason: &str) -> Result<(), RemoteFixtureError> {
    if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
        return Err(RemoteFixtureError::InvalidReason);
    }
    Ok(())
}

fn validate_artifact_name(name: &str) -> Result<(), RemoteFixtureError> {
    if name.is_empty()
        || name.len() > MAX_IDENTIFIER_BYTES
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
        || name.contains(':')
        || name
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(RemoteFixtureError::InvalidArtifactName);
    }
    Ok(())
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), RemoteFixtureError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RemoteFixtureError::InvalidDigest { field });
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn signature_message(body_sha256: &str) -> Vec<u8> {
    format!("{RECEIPT_SCHEMA}:{RECEIPT_SCHEMA_VERSION}:{body_sha256}").into_bytes()
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RemoteFixtureError {
    #[error("invalid fixture identifier: {field}")]
    InvalidIdentifier { field: &'static str },
    #[error("invalid SHA-256 digest: {field}")]
    InvalidDigest { field: &'static str },
    #[error("fixture resource budgets must be non-zero")]
    InvalidResourceBudget,
    #[error("fixture input exceeds {limit} bytes")]
    InputTooLarge { limit: usize },
    #[error("fixture artifact exceeds {limit} bytes")]
    ArtifactTooLarge { limit: usize },
    #[error("fixture artifact name is not a portable normalized relative path")]
    InvalidArtifactName,
    #[error("fixture terminal reason is empty or too large")]
    InvalidReason,
    #[error("fixture script exceeds {limit} output events")]
    TooManyEvents { limit: usize },
    #[error("fixture event sequence mismatch: expected {expected}, observed {observed}")]
    InvalidEventSequence { expected: u64, observed: u64 },
    #[error("fixture event text exceeds {limit} bytes")]
    EventTextTooLarge { limit: usize },
    #[error("fixture total event text exceeds {limit} bytes")]
    TotalEventTextTooLarge { limit: usize },
    #[error("fixture serialization failed: {0}")]
    Serialize(String),
    #[error("unsupported remote fixture receipt schema")]
    UnsupportedReceiptSchema,
    #[error("unsupported remote fixture protocol")]
    UnsupportedFixtureProtocol,
    #[error("remote fixture receipt backend identity mismatch")]
    BackendIdentityMismatch,
    #[error("remote fixture receipt body digest mismatch")]
    ReceiptDigestMismatch,
    #[error("remote fixture receipt event digest mismatch")]
    EventDigestMismatch,
    #[error("remote fixture receipt has invalid event or terminal semantics")]
    InvalidReceiptSemantics,
    #[error("remote fixture attestation signature is malformed")]
    MalformedSignature,
    #[error("remote fixture attestation signature verification failed")]
    InvalidSignature,
}
