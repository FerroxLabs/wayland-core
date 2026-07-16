use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::message::TokenUsage;

pub const DURABLE_CHILD_SCHEMA_VERSION: u16 = 1;
pub const MAX_DURABLE_CHILD_ID_BYTES: usize = 256;
pub const MAX_DURABLE_CHILD_STRING_BYTES: usize = 512;
pub const MAX_DURABLE_CHILD_ARTIFACTS: usize = 256;
pub const MAX_DURABLE_CHILD_APPLIED_EVENTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ChildId(String);

impl ChildId {
    pub fn new(value: impl Into<String>) -> Result<Self, DurableChildError> {
        let value = value.into();
        validate_identifier("child_id", &value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ChildId {
    type Error = DurableChildError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for ChildId {
    type Error = DurableChildError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ChildId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for ChildId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildParent {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_child_id: Option<ChildId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildOrigin {
    Spawn,
    Delegate,
    ForkSkill,
    Workflow,
    Swarm,
    Mesh,
    Fleet,
    Anvil,
    Council,
    Synthesis,
    Pipeline,
    Host,
}

/// Digest-only durable request evidence. Plaintext has no representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildRequestEvidence {
    pub exact_digest: String,
}

impl ChildRequestEvidence {
    #[must_use]
    pub fn redacted(exact_digest: impl Into<String>) -> Self {
        Self {
            exact_digest: exact_digest.into(),
        }
    }

    #[must_use]
    pub fn exact_digest(&self) -> &str {
        &self.exact_digest
    }
}

/// Redacted policy evidence only. It cannot be converted back into execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildPolicySnapshot {
    pub contract_version: String,
    pub exact_digest: String,
    pub posture: String,
    pub approvals: String,
    pub sandbox: String,
    pub source: String,
    pub managed_floor_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangerous_activation_id_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildWorkspaceMode {
    SharedReadOnly,
    Isolated,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildWorkspace {
    pub mode: ChildWorkspaceMode,
    /// Opaque workspace identity. Host filesystem paths are intentionally absent.
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildDeliveryTarget {
    ParentTurn,
    ParentChild { child_id: ChildId },
    SessionOutbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildDeliveryState {
    NotRequired,
    Pending,
    InFlight,
    Delivered { receipt_digest: String },
    Failed { error_digest: String },
    Unknown { evidence_digest: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableChildStatus {
    Prepared,
    Queued,
    Running,
    Paused,
    RecoveryRequired,
    Succeeded,
    Failed,
    Cancelled,
    /// Retention tombstone. The record and immutable lineage remain addressable.
    Expired,
}

impl DurableChildStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Expired
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildDesiredState {
    Run,
    Pause,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildRecoveryState {
    Clean,
    Required { reason_digest: String },
    Resolved { evidence_digest: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildTimestamps {
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableChildResult {
    pub exact_digest: String,
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_digests: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableChildRecord {
    pub schema_version: u16,
    /// Stable idempotency identity for the declaration operation.
    pub declaration_id: String,
    pub child_id: ChildId,
    pub parent: ChildParent,
    pub origin: ChildOrigin,
    pub request: ChildRequestEvidence,
    pub policy_snapshot: ChildPolicySnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub workspace: ChildWorkspace,
    pub status: DurableChildStatus,
    pub desired_state: ChildDesiredState,
    pub recovery: ChildRecoveryState,
    pub revision: u64,
    pub timestamps: ChildTimestamps,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<DurableChildResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_target: Option<ChildDeliveryTarget>,
    pub delivery_state: ChildDeliveryState,
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<ChildId>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub applied_events: BTreeMap<String, String>,
}

impl DurableChildRecord {
    pub fn validate_declaration(&self) -> Result<(), DurableChildError> {
        if self.schema_version != DURABLE_CHILD_SCHEMA_VERSION {
            return Err(DurableChildError::UnsupportedSchema(self.schema_version));
        }
        validate_identifier("declaration_id", &self.declaration_id)?;
        validate_identifier("child_id", self.child_id.as_str())?;
        validate_identifier("parent.session_id", &self.parent.session_id)?;
        validate_identifier("workspace.workspace_id", &self.workspace.workspace_id)?;
        validate_digest("request.exact_digest", self.request.exact_digest())?;
        validate_string(
            "policy_snapshot.contract_version",
            &self.policy_snapshot.contract_version,
        )?;
        validate_digest(
            "policy_snapshot.exact_digest",
            &self.policy_snapshot.exact_digest,
        )?;
        for (field, value) in [
            ("policy_snapshot.posture", &self.policy_snapshot.posture),
            ("policy_snapshot.approvals", &self.policy_snapshot.approvals),
            ("policy_snapshot.sandbox", &self.policy_snapshot.sandbox),
            ("policy_snapshot.source", &self.policy_snapshot.source),
        ] {
            validate_string(field, value)?;
        }
        if let Some(digest) = &self.policy_snapshot.dangerous_activation_id_digest {
            validate_digest("dangerous_activation_id_digest", digest)?;
        }
        if self.parent.parent_child_id.as_ref() == Some(&self.child_id)
            || self.retry_of.as_ref() == Some(&self.child_id)
            || matches!(
                &self.delivery_target,
                Some(ChildDeliveryTarget::ParentChild { child_id }) if child_id == &self.child_id
            )
        {
            return Err(DurableChildError::IdentityCycle);
        }
        for (field, value) in [
            ("parent.turn_id", self.parent.turn_id.as_deref()),
            (
                "parent.workflow_run_id",
                self.parent.workflow_run_id.as_deref(),
            ),
            ("parent.graph_node_id", self.parent.graph_node_id.as_deref()),
            (
                "parent.parent_call_id",
                self.parent.parent_call_id.as_deref(),
            ),
            ("provider", self.provider.as_deref()),
            ("model", self.model.as_deref()),
        ] {
            if let Some(value) = value {
                validate_identifier(field, value)?;
            }
        }
        if self.status != DurableChildStatus::Prepared
            || self.desired_state != ChildDesiredState::Run
            || !matches!(self.recovery, ChildRecoveryState::Clean)
            || self.revision != 0
            || self.result.is_some()
            || !self.applied_events.is_empty()
            || self.timestamps.queued_at_unix_ms.is_some()
            || self.timestamps.started_at_unix_ms.is_some()
            || self.timestamps.terminal_at_unix_ms.is_some()
        {
            return Err(DurableChildError::InvalidDeclarationState);
        }
        if self.timestamps.updated_at_unix_ms != self.timestamps.created_at_unix_ms {
            return Err(DurableChildError::InvalidTimestamp);
        }
        match (&self.delivery_target, &self.delivery_state) {
            (None, ChildDeliveryState::NotRequired) | (Some(_), ChildDeliveryState::Pending) => {}
            _ => return Err(DurableChildError::InvalidDeliveryState),
        }
        if let Some(ChildDeliveryTarget::ParentChild { child_id }) = &self.delivery_target
            && self.parent.parent_child_id.as_ref() != Some(child_id)
        {
            return Err(DurableChildError::InvalidDeliveryState);
        }
        if self.attempt == 0 {
            return Err(DurableChildError::InvalidField("attempt"));
        }
        if (self.attempt == 1) != self.retry_of.is_none() {
            return Err(DurableChildError::InvalidField("retry_of"));
        }
        if self.applied_events.len() > MAX_DURABLE_CHILD_APPLIED_EVENTS {
            return Err(DurableChildError::CollectionTooLarge("applied_events"));
        }
        for (event_id, digest) in &self.applied_events {
            validate_identifier("applied_events.event_id", event_id)?;
            validate_digest("applied_events.digest", digest)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transition", rename_all = "snake_case", deny_unknown_fields)]
pub enum DurableChildTransition {
    Enqueue,
    Start,
    RequestPause,
    Paused,
    Resume,
    RequestCancel,
    Succeed {
        result: DurableChildResult,
    },
    Fail {
        result: DurableChildResult,
    },
    Cancel,
    RequireRecovery {
        reason_digest: String,
    },
    ResolveRecovery {
        evidence_digest: String,
    },
    SucceedAfterRecovery {
        result: DurableChildResult,
    },
    FailAfterRecovery {
        result: DurableChildResult,
    },
    CancelAfterRecovery {
        evidence_digest: String,
    },
    DeliveryStarted,
    DeliveryDelivered {
        receipt_digest: String,
    },
    DeliveryFailed {
        error_digest: String,
    },
    DeliveryUnknown {
        evidence_digest: String,
    },
    RetryFailedDelivery {
        prior_error_digest: String,
    },
    ReconcileUnknownDelivery {
        prior_evidence_digest: String,
        resolution: ChildDeliveryReconciliation,
    },
    Expire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChildDeliveryReconciliation {
    Delivered { receipt_digest: String },
    Failed { error_digest: String },
    NotDelivered { proof_digest: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DurableChildError {
    #[error("unsupported durable child schema {0}")]
    UnsupportedSchema(u16),
    #[error("invalid durable child field {0}")]
    InvalidField(&'static str),
    #[error("invalid SHA-256 digest in durable child field {0}")]
    InvalidDigest(&'static str),
    #[error("durable child parent/retry identity forms a self-cycle")]
    IdentityCycle,
    #[error("durable child declaration must begin in pristine prepared state")]
    InvalidDeclarationState,
    #[error("durable child timestamp is non-monotonic")]
    InvalidTimestamp,
    #[error("durable child delivery target and state disagree")]
    InvalidDeliveryState,
    #[error("durable child collection {0} exceeds its bound")]
    CollectionTooLarge(&'static str),
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), DurableChildError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.len() > MAX_DURABLE_CHILD_ID_BYTES
    {
        return Err(DurableChildError::InvalidField(field));
    }
    Ok(())
}

fn validate_string(field: &'static str, value: &str) -> Result<(), DurableChildError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.len() > MAX_DURABLE_CHILD_STRING_BYTES
    {
        return Err(DurableChildError::InvalidField(field));
    }
    Ok(())
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), DurableChildError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DurableChildError::InvalidDigest(field));
    }
    Ok(())
}

/// Configuration for a sub-agent invocation.
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// Descriptive name for logging
    pub name: String,
    /// The task prompt
    pub prompt: String,
    /// Max turns for this sub-agent (typically lower than main agent)
    pub max_turns: usize,
    /// Max output tokens per response
    pub max_tokens: u32,
    /// Optional system prompt override
    pub system_prompt: Option<String>,
    /// Slice-1 MoP: pin this sub-agent to a named provider (resolved by
    /// `CouncilProviderResolver`). `None` ⇒ inherit the spawner's provider.
    pub provider: Option<String>,
    /// Optional model override applied to the child engine config. `None` ⇒
    /// inherit the (resolved) provider's default model.
    pub model: Option<String>,
    /// Crucible #3: optional sampling temperature applied to the child engine's
    /// requests via `child_config`. `None` ⇒ inherit the base config's
    /// temperature (the engine then omits the field unless the base set one).
    pub temperature: Option<f32>,
}

/// Overrides applied when spawning a fork-mode skill sub-agent.
#[derive(Debug, Clone, Default)]
pub struct ForkOverrides {
    /// Replace the parent's configured model with this one.
    pub model: Option<String>,
    /// Reasoning effort ("low"/"medium"/"high"/"max").
    pub effort: Option<String>,
    /// Restrict registered tools to this list; empty = all built-in tools.
    pub allowed_tools: Vec<String>,
}

/// Result from a completed sub-agent execution.
#[derive(Debug)]
pub struct SubAgentResult {
    pub name: String,
    pub text: String,
    pub usage: TokenUsage,
    pub turns: usize,
    pub is_error: bool,
}

impl SubAgentResult {
    /// Build a terminal error result for a sub-agent that never ran (e.g. its
    /// pinned provider could not be resolved). Zero usage, zero turns,
    /// `is_error = true`.
    pub fn error(name: &str, text: &str) -> Self {
        Self {
            name: name.to_string(),
            text: text.to_string(),
            usage: TokenUsage::default(),
            turns: 0,
            is_error: true,
        }
    }
}

/// Abstraction over fork-mode agent spawning — enables mock implementations in tests.
#[async_trait]
pub trait Spawner: Send + Sync {
    /// Spawn a fork-mode sub-agent with optional overrides and wait for its result.
    async fn spawn_fork(&self, config: SubAgentConfig, overrides: ForkOverrides) -> SubAgentResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_agent_config_carries_optional_provider_and_model() {
        let c = SubAgentConfig {
            name: "p".into(),
            prompt: "x".into(),
            max_turns: 1,
            max_tokens: 16,
            system_prompt: None,
            provider: Some("openai".into()),
            model: Some("gpt-5.5".into()),
            temperature: None,
        };
        assert_eq!(c.provider.as_deref(), Some("openai"));
        assert_eq!(c.model.as_deref(), Some("gpt-5.5"));
    }
}
