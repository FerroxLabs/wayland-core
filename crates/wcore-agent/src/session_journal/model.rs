use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wcore_types::tool::ToolEffectContract;

use super::GENESIS_CHECKSUM;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CompletionOutcome {
    Succeeded,
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttemptPurpose {
    Conversation,
    Compaction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderAttemptNotStartedReason {
    EgressDenied { policy: String },
    BeforeDispatchFailed { error: String },
    BudgetDenied { reason: String },
    Cancelled { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolNotStartedReason {
    PolicyDenied { policy: String },
    HookDenied { reason: String },
    BudgetDenied { reason: String },
    CircuitOpen,
    UnknownTool,
    ApprovalDenied { approval_id: String },
    ApprovalCancelled { approval_id: String },
    ApprovalTimedOut { approval_id: String },
    InvalidInput { error: String },
    DispatchFailed { error: String },
    Cancelled { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolUnknownReason {
    Interrupted,
    TimedOut { timeout_ms: u64 },
    Cancelled { reason: String },
    Panicked { message: String },
    TransportLost,
    AmbiguousFailure { error: String },
    ResultPersistenceFailed { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ToolResolutionSource {
    Reconciler { reconciler: String },
    Operator { operator_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ToolResolution {
    Succeeded {
        result: serde_json::Value,
    },
    Failed {
        error: String,
        result: Option<serde_json::Value>,
    },
    NotStarted {
        reason: ToolNotStartedReason,
    },
}

/// Durable representation of an input whose exact bytes may contain secrets.
///
/// The exact digest remains authoritative for identity and idempotency. The
/// payload itself is either omitted/redacted or supplied as an independently
/// secured envelope; raw plaintext has no representation in this schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case")]
pub enum StoredToolInput {
    Redacted {
        exact_digest: String,
        summary: Option<serde_json::Value>,
    },
    Secured {
        exact_digest: String,
        envelope: serde_json::Value,
    },
}

impl StoredToolInput {
    #[must_use]
    pub fn redacted(exact_digest: impl Into<String>) -> Self {
        Self::Redacted {
            exact_digest: exact_digest.into(),
            summary: None,
        }
    }

    #[must_use]
    pub fn exact_digest(&self) -> &str {
        match self {
            Self::Redacted { exact_digest, .. } | Self::Secured { exact_digest, .. } => {
                exact_digest
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChildNotStartedReason {
    PolicyDenied { policy: String },
    ApprovalDenied { approval_id: String },
    ApprovalCancelled { approval_id: String },
    ApprovalTimedOut { approval_id: String },
    InvalidRequest { error: String },
    DispatchFailed { error: String },
    Cancelled { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum ApprovalOrigin {
    Turn { turn_id: String },
    ProviderAttempt { attempt_id: String },
    ToolExecution { tool_execution_id: String },
    Child { child_id: String },
    Delivery { delivery_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetUnit {
    Tokens,
    Requests,
    ToolCalls,
    Milliseconds,
    Bytes,
    Credits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetAmount {
    pub value: u64,
    pub unit: BudgetUnit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum BudgetOwner {
    Session,
    Turn { turn_id: String },
    ProviderAttempt { attempt_id: String },
    ToolExecution { tool_execution_id: String },
    Child { child_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPurpose {
    Conversation,
    Compaction,
    ToolExecution,
    ChildExecution,
    Delivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointPurpose {
    Recovery,
    Compaction,
    UserRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum CheckpointOrigin {
    Session,
    Turn { turn_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum DeliveryOrigin {
    Turn {
        turn_id: String,
    },
    InboundReply {
        inbound_reply_id: String,
    },
    Cron {
        schedule_id: String,
        fire_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStage {
    DispatchAccepted,
    PayloadSent,
    AwaitingAcknowledgement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryEvidence {
    pub last_observed_stage: DeliveryStage,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DeliveryUnknownReason {
    TimedOut { timeout_ms: u64 },
    TransportLost,
    AcknowledgementMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliveryNotStartedReason {
    PolicyDenied { policy: String },
    ApprovalDenied { approval_id: String },
    ApprovalCancelled { approval_id: String },
    ApprovalTimedOut { approval_id: String },
    InvalidDestination { error: String },
    DispatchFailed { error: String },
    Cancelled { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeliveryCompletion {
    Confirmed {
        outcome: CompletionOutcome,
        receipt: serde_json::Value,
    },
    Unknown {
        reason: DeliveryUnknownReason,
        evidence: DeliveryEvidence,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderStreamEvent {
    TextDelta {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        extra: Option<serde_json::Value>,
    },
    ThinkingDelta {
        text: String,
    },
    ThinkingSubject {
        subject: String,
    },
    Done {
        stop_reason: serde_json::Value,
        finish_reason: serde_json::Value,
        usage: serde_json::Value,
    },
    Error {
        message: String,
    },
    Citations {
        urls: Vec<String>,
    },
    SearchResults {
        results: Vec<serde_json::Value>,
    },
    ProviderMeta {
        metadata: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalResolution {
    Decided { decision: ApprovalDecision },
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// The append-only journal schema is a public, versioned wire contract. Boxing
// one variant solely to reduce stack size would change its Rust API during the
// F13 compatibility window even though serde would hide the allocation.
#[allow(clippy::large_enum_variant)]
#[non_exhaustive]
pub enum SessionEvent {
    SessionImported {
        source_schema_version: u32,
        session: serde_json::Value,
        session_digest: String,
    },
    ConversationMessageCommitted {
        turn_id: String,
        message_index: u64,
        message: serde_json::Value,
        message_digest: String,
    },
    ConversationStateCommitted {
        turn_id: String,
        messages: Vec<serde_json::Value>,
        messages_digest: String,
    },
    TurnStarted {
        turn_id: String,
        user_message: String,
    },
    TurnCommitted {
        turn_id: String,
        assistant_message: String,
    },
    TurnFailed {
        turn_id: String,
        error: String,
    },
    TurnCancelled {
        turn_id: String,
    },
    StreamStarted {
        stream_id: String,
        attempt_id: String,
    },
    StreamBatchCommitted {
        stream_id: String,
        ordinal: u64,
        events: Vec<ProviderStreamEvent>,
    },
    StreamFinished {
        stream_id: String,
    },
    ProviderAttemptPrepared {
        attempt_id: String,
        turn_id: String,
        purpose: ProviderAttemptPurpose,
        provider: String,
        model: String,
        request_digest: String,
    },
    ProviderAttemptStarted {
        attempt_id: String,
    },
    ProviderAttemptFinished {
        attempt_id: String,
        outcome: CompletionOutcome,
        response_digest: Option<String>,
    },
    ProviderAttemptNotStarted {
        attempt_id: String,
        reason: ProviderAttemptNotStartedReason,
    },
    ToolIntentRecorded {
        tool_execution_id: String,
        provider_call_id: String,
        turn_id: String,
        ordinal: u64,
        tool: String,
        requested_input: serde_json::Value,
        requested_input_digest: String,
        effective_input: serde_json::Value,
        effective_input_digest: String,
    },
    /// F13 versioned intent record. The legacy variant remains constructible
    /// and replayable so downstream producers are not forced to change every
    /// struct-like enum construction at this boundary.
    ToolIntentRecordedV2 {
        tool_execution_id: String,
        idempotency_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_of: Option<String>,
        provider_call_id: String,
        turn_id: String,
        ordinal: u64,
        tool: String,
        requested_input: StoredToolInput,
        requested_input_digest: String,
        effective_input: StoredToolInput,
        effective_input_digest: String,
        effect_contract: ToolEffectContract,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effect_receipt: Option<serde_json::Value>,
    },
    ToolExecutionStarted {
        tool_execution_id: String,
    },
    ToolExecutionFinished {
        tool_execution_id: String,
        outcome: CompletionOutcome,
        result: serde_json::Value,
    },
    ToolExecutionNotStarted {
        tool_execution_id: String,
        reason: ToolNotStartedReason,
    },
    ToolExecutionUnknown {
        tool_execution_id: String,
        reason: ToolUnknownReason,
        evidence: serde_json::Value,
    },
    ToolExecutionResolved {
        tool_execution_id: String,
        resolution: ToolResolution,
        source: ToolResolutionSource,
        evidence: serde_json::Value,
    },
    ApprovalRequested {
        approval_id: String,
        origin: ApprovalOrigin,
        intent_digest: String,
    },
    ApprovalResolved {
        approval_id: String,
        resolution: ApprovalResolution,
    },
    BudgetReserved {
        event_id: String,
        reservation_id: String,
        owner: BudgetOwner,
        purpose: BudgetPurpose,
        amount: BudgetAmount,
    },
    BudgetSettled {
        event_id: String,
        reservation_id: String,
        amount: BudgetAmount,
    },
    BudgetReleased {
        event_id: String,
        reservation_id: String,
    },
    CheckpointCommitted {
        checkpoint_id: String,
        purpose: CheckpointPurpose,
        origin: CheckpointOrigin,
        state_digest: String,
        state: serde_json::Value,
    },
    ChildPrepared {
        child_id: String,
        turn_id: String,
        request: serde_json::Value,
    },
    ChildStarted {
        child_id: String,
    },
    ChildFinished {
        child_id: String,
        outcome: CompletionOutcome,
        result: serde_json::Value,
    },
    ChildNotStarted {
        child_id: String,
        reason: ChildNotStartedReason,
    },
    DeliveryPrepared {
        delivery_id: String,
        origin: DeliveryOrigin,
        destination: String,
        payload: serde_json::Value,
    },
    DeliveryStarted {
        delivery_id: String,
    },
    DeliveryNotStarted {
        delivery_id: String,
        reason: DeliveryNotStartedReason,
    },
    DeliveryFinished {
        delivery_id: String,
        completion: DeliveryCompletion,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExternalEffectState {
    Prepared,
    Unknown,
    NotStarted,
    Completed { outcome: CompletionOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolEffectState {
    Prepared,
    Running,
    Succeeded,
    Failed {
        error: String,
    },
    NotStarted,
    Unknown {
        reason: ToolUnknownReason,
        evidence: serde_json::Value,
    },
}

impl ToolEffectState {
    #[must_use]
    pub fn requires_reconciliation(&self) -> bool {
        matches!(self, Self::Running | Self::Unknown { .. })
    }
}

impl ExternalEffectState {
    #[must_use]
    pub fn requires_reconciliation(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnCompletion {
    Committed { assistant_message: String },
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnState {
    pub user_message: String,
    pub completion: Option<TurnCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamState {
    pub attempt_id: String,
    pub next_ordinal: u64,
    pub batches: Vec<Vec<ProviderStreamEvent>>,
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttemptState {
    pub turn_id: String,
    pub purpose: ProviderAttemptPurpose,
    pub provider: String,
    pub model: String,
    pub request_digest: String,
    pub response_digest: Option<String>,
    pub not_started_reason: Option<ProviderAttemptNotStartedReason>,
    pub effect: ExternalEffectState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolState {
    pub idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    pub provider_call_id: String,
    pub turn_id: String,
    pub ordinal: u64,
    pub tool: String,
    pub requested_input: StoredToolInput,
    pub requested_input_digest: String,
    pub effective_input: StoredToolInput,
    pub effective_input_digest: String,
    pub effect_contract: ToolEffectContract,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_receipt: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub not_started_reason: Option<ToolNotStartedReason>,
    pub resolution_source: Option<ToolResolutionSource>,
    pub resolution_evidence: Option<serde_json::Value>,
    pub effect: ToolEffectState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalState {
    pub origin: ApprovalOrigin,
    pub intent_digest: String,
    pub resolution: Option<ApprovalResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetState {
    pub owner: BudgetOwner,
    pub purpose: BudgetPurpose,
    pub reserved: BudgetAmount,
    pub used: Option<BudgetAmount>,
    pub released: bool,
    pub event_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointState {
    pub purpose: CheckpointPurpose,
    pub origin: CheckpointOrigin,
    pub state_digest: String,
    pub state: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildState {
    pub turn_id: String,
    pub request: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub not_started_reason: Option<ChildNotStartedReason>,
    pub effect: ExternalEffectState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryState {
    pub origin: DeliveryOrigin,
    pub destination: String,
    pub payload: serde_json::Value,
    pub completion: Option<DeliveryCompletion>,
    pub not_started_reason: Option<DeliveryNotStartedReason>,
    pub effect: ExternalEffectState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedSessionBaseline {
    pub source_schema_version: u32,
    pub session_digest: String,
    pub imported_message_count: u64,
    pub session: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReducedSessionState {
    pub session_id: Option<String>,
    pub last_seq: Option<u64>,
    pub last_checksum: String,
    #[serde(default)]
    pub imported_baseline: Option<ImportedSessionBaseline>,
    #[serde(default)]
    pub conversation: Vec<serde_json::Value>,
    pub turns: BTreeMap<String, TurnState>,
    pub streams: BTreeMap<String, StreamState>,
    pub provider_attempts: BTreeMap<String, ProviderAttemptState>,
    pub tools: BTreeMap<String, ToolState>,
    pub approvals: BTreeMap<String, ApprovalState>,
    pub budgets: BTreeMap<String, BudgetState>,
    pub budget_event_ids: BTreeMap<String, String>,
    pub checkpoints: BTreeMap<String, CheckpointState>,
    pub children: BTreeMap<String, ChildState>,
    pub deliveries: BTreeMap<String, DeliveryState>,
}

impl Default for ReducedSessionState {
    fn default() -> Self {
        Self {
            session_id: None,
            last_seq: None,
            last_checksum: GENESIS_CHECKSUM.to_owned(),
            imported_baseline: None,
            conversation: Vec::new(),
            turns: BTreeMap::new(),
            streams: BTreeMap::new(),
            provider_attempts: BTreeMap::new(),
            tools: BTreeMap::new(),
            approvals: BTreeMap::new(),
            budgets: BTreeMap::new(),
            budget_event_ids: BTreeMap::new(),
            checkpoints: BTreeMap::new(),
            children: BTreeMap::new(),
            deliveries: BTreeMap::new(),
        }
    }
}
