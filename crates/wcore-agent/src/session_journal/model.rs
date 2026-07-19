use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wcore_types::spawner::{ChildId, DurableChildRecord, DurableChildTransition};
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

pub const HOOK_PHASE_LIFECYCLE_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolHookPhase {
    PreToolUse,
    PostToolUse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookSlotSource {
    Rust,
    Shell,
    Plugin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookManifestSlot {
    pub ordinal: u64,
    pub slot_id: String,
    pub source: HookSlotSource,
    pub descriptor_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookSlotTerminalStatus {
    Completed,
    SkippedAfterBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookSlotReceipt {
    pub ordinal: u64,
    pub slot_id: String,
    pub descriptor_digest: String,
    pub status: HookSlotTerminalStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPhaseNotStartedReason {
    CancelledBeforeStart,
    Superseded,
    ToolOutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookPhaseConsumption {
    pub hook_phase_id: String,
    pub outcome_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HookPhaseState {
    Prepared,
    Started {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_digest: Option<String>,
    },
    Finished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_digest: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_input_digest: Option<String>,
        outcome_digest: String,
        slot_receipts_digest: String,
        slot_receipts: Vec<HookSlotReceipt>,
    },
    NotStarted {
        reason: HookPhaseNotStartedReason,
    },
    NotApplicable,
    AbandonedUnknown,
    Consumed {
        outcome_digest: String,
        checkpoint_id: String,
    },
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

/// Schema for the durable enforcement-authority payload carried by the
/// session journal. This version is independent of the outer journal schema:
/// recovery must opt into each authority shape explicitly rather than
/// interpreting an unknown payload as a fresh budget.
pub const BUDGET_AUTHORITY_SCHEMA_VERSION: u32 = 2;
pub const LEGACY_BUDGET_AUTHORITY_SCHEMA_VERSION: u32 = 1;

/// Journal head that an authority replacement was derived from.
///
/// The committed event itself receives the next sequence/checksum from the
/// journal. Binding its payload to the prior head prevents a snapshot captured
/// from stale runtime state from being appended after newer conversation or
/// budget state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetAuthorityCursor {
    pub journal_sequence: Option<u64>,
    pub journal_checksum: String,
}

/// Explicit interpretation of wall-time authority across process restart.
///
/// `ActiveRuntime` preserves already-consumed monotonic runtime but excludes
/// downtime. `AbsoluteDeadline` is the fail-closed wall-clock form: recovery
/// must preserve the supplied deadline and may only tighten it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "semantics", rename_all = "snake_case", deny_unknown_fields)]
pub enum BudgetWallClockAuthority {
    ActiveRuntime,
    AbsoluteDeadline { deadline_unix_millis: u64 },
}

/// Optional in-flight turn budget tree. Its execution snapshot includes the
/// root-to-parent chain needed to restore roll-up enforcement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveTurnBudgetAuthority {
    pub turn_id: String,
    pub execution: wcore_budget::ExecutionBudgetSnapshot,
}

// Snapshot constructors reject non-finite monetary values, so the contained
// `f64` values retain reflexive equality after construction/deserialization.
impl Eq for ActiveTurnBudgetAuthority {}

/// One provider admission bound to the logical dispatch it authorizes.
///
/// `prior_attempt_ids` distinguishes a newly reserved configured-fallback
/// attempt from earlier paid attempts under the same logical dispatch. On
/// restart, only attempt identities absent from this set can prove that this
/// particular reservation reached the physical-send boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBudgetReservationAuthority {
    pub reservation: wcore_budget::BudgetReservation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_attempt_ids: Vec<String>,
}

/// Complete durable budget authority at one journal boundary.
///
/// Runtime-only handles (event sinks, cancellation tasks, process handles) are
/// intentionally absent. Bootstrap restores those around these immutable
/// enforcement snapshots only after the reducer accepts this payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetAuthorityState {
    pub schema_version: u32,
    pub authority_epoch: u64,
    pub prior_cursor: BudgetAuthorityCursor,
    pub budget_session_id: String,
    pub provider_tracker: wcore_budget::BudgetTrackerSnapshot,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_reservations: BTreeMap<String, ProviderBudgetReservationAuthority>,
    pub execution_root: wcore_budget::ExecutionBudgetSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<ActiveTurnBudgetAuthority>,
    pub captured_at_unix_millis: u64,
    pub wall_clock: BudgetWallClockAuthority,
    pub conversation_digest: String,
}

// See `ActiveTurnBudgetAuthority`: validated snapshots exclude NaN/infinite
// values, making equality suitable for journal/state comparisons.
impl Eq for BudgetAuthorityState {}

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
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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
    /// Atomically advances the durable conversation and records the exact
    /// recovery boundary that authorizes the next agent-loop iteration.
    ConversationRecoveryCheckpointCommitted {
        turn_id: String,
        messages: Vec<serde_json::Value>,
        messages_digest: String,
        checkpoint_id: String,
        checkpoint_state_digest: String,
        checkpoint: serde_json::Value,
    },
    /// Recovery checkpoint that atomically consumes finished hook outcomes.
    /// The legacy event remains replayable for journals written before hook
    /// lifecycle authority was introduced.
    ConversationRecoveryCheckpointCommittedV2 {
        turn_id: String,
        messages: Vec<serde_json::Value>,
        messages_digest: String,
        checkpoint_id: String,
        checkpoint_state_digest: String,
        checkpoint: serde_json::Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        consumed_hook_phases: Vec<HookPhaseConsumption>,
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
    /// Recovery-correlated provider attempt. The legacy event remains part of
    /// the public journal contract, but only this shape can bind a physical
    /// attempt to the exact logical dispatch that authorized it.
    ProviderAttemptPreparedV2 {
        attempt_id: String,
        dispatch_id: String,
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
    /// Terminal receipt for a recovery-correlated provider attempt.
    ProviderAttemptFinishedV2 {
        attempt_id: String,
        dispatch_id: String,
        outcome: CompletionOutcome,
        response_digest: Option<String>,
    },
    ProviderAttemptNotStarted {
        attempt_id: String,
        reason: ProviderAttemptNotStartedReason,
    },
    /// Proved no-send receipt for a recovery-correlated provider attempt.
    ProviderAttemptNotStartedV2 {
        attempt_id: String,
        dispatch_id: String,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_hook_phase_id: Option<String>,
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
    HookPhasePrepared {
        hook_phase_id: String,
        lifecycle_version: u64,
        turn_id: String,
        provider_call_id: String,
        ordinal: u64,
        phase: ToolHookPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_execution_id: Option<String>,
        input_digest: String,
        hook_authority_digest: String,
        hook_manifest_digest: String,
        hook_slots: Vec<HookManifestSlot>,
    },
    HookPhaseStarted {
        hook_phase_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_digest: Option<String>,
    },
    HookPhaseFinished {
        hook_phase_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_digest: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_input_digest: Option<String>,
        outcome_digest: String,
        slot_receipts_digest: String,
        slot_receipts: Vec<HookSlotReceipt>,
    },
    HookPhaseNotStarted {
        hook_phase_id: String,
        reason: HookPhaseNotStartedReason,
    },
    HookPhaseNotApplicable {
        hook_phase_id: String,
    },
    HookPhaseAbandonedUnknown {
        hook_phase_id: String,
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
    BudgetAuthorityCommitted {
        authority: BudgetAuthorityState,
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
    /// F18 typed child declaration. Legacy child events remain replayable but
    /// cannot be mistaken for a complete durable child resource.
    ChildDeclaredV2 {
        record: DurableChildRecord,
    },
    /// One revision-checked transition in the durable child state machine.
    ChildTransitionedV2 {
        child_id: ChildId,
        event_id: String,
        expected_revision: u64,
        at_unix_ms: u64,
        transition: DurableChildTransition,
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
    /// Present only for V2 attempts whose physical identity is bound to an
    /// exact logical dispatch. `None` is intentionally recovery-ineligible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_hook_phase_id: Option<String>,
    pub result: Option<serde_json::Value>,
    pub not_started_reason: Option<ToolNotStartedReason>,
    pub resolution_source: Option<ToolResolutionSource>,
    pub resolution_evidence: Option<serde_json::Value>,
    pub effect: ToolEffectState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookPhaseExecutionState {
    pub lifecycle_version: u64,
    pub turn_id: String,
    pub provider_call_id: String,
    pub ordinal: u64,
    pub phase: ToolHookPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_execution_id: Option<String>,
    pub input_digest: String,
    pub hook_authority_digest: String,
    pub hook_manifest_digest: String,
    pub hook_slots: Vec<HookManifestSlot>,
    pub state: HookPhaseState,
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
    /// Present only for F18 V2 children. The legacy fields above remain as a
    /// compatibility projection for old snapshots and recovery callers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<DurableChildRecord>,
    /// Digest of the pristine declaration payload, retained across mutations
    /// so an exact declaration retry remains distinguishable from conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable_declaration_digest: Option<String>,
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
#[serde(deny_unknown_fields)]
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hook_phases: BTreeMap<String, HookPhaseExecutionState>,
    pub approvals: BTreeMap<String, ApprovalState>,
    pub budgets: BTreeMap<String, BudgetState>,
    pub budget_event_ids: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_authority: Option<BudgetAuthorityState>,
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
            hook_phases: BTreeMap::new(),
            approvals: BTreeMap::new(),
            budgets: BTreeMap::new(),
            budget_event_ids: BTreeMap::new(),
            budget_authority: None,
            checkpoints: BTreeMap::new(),
            children: BTreeMap::new(),
            deliveries: BTreeMap::new(),
        }
    }
}
