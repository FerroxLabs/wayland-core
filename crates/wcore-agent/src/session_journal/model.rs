use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use wcore_protocol::events::RecoveryCursor;
use wcore_types::child_transaction::{ChildGatePlan, ChildTransactionReceipt};
use wcore_types::goal::{GoalStrategy, GoalTerminalState, TaskUnknownReason, WaitKind};
use wcore_types::spawner::{ChildId, DurableChildRecord, DurableChildTransition};
use wcore_types::tool::ToolEffectContract;

use super::GENESIS_CHECKSUM;
use crate::goal::GoalAuthorityRecord;

/// Skip predicate for the `Option<serde_json::Value>` fields the journal and
/// snapshot digests cover — 23B-H1.
///
/// The journal's integrity check re-serializes a decoded event and compares the
/// hash against the one stored on disk, so the encoding has to be a round-trip
/// fixed point. `Option::is_none` is not one for a `Value` field: serde writes
/// `Some(Value::Null)` as an explicit `"field":null`, decodes that back to
/// `None` (Option's Deserialize maps a JSON null to None), and then OMITS the
/// field on re-serialization. The recomputed checksum differs from the stored
/// one and the reader rejects a journal the writer wrote correctly, with
/// `journal checksum mismatch at sequence N` — permanently, since every
/// operator verb reads the journal.
///
/// Skipping `Some(Value::Null)` as well as `None` makes the encoding a fixed
/// point without changing what anything means: the wire contract already
/// treats an explicit null as equivalent to an absent field (pinned by
/// `known_explicit_event_defaults_are_wire_compatible_but_unknowns_fail_closed`),
/// so a null that is never written is a null nobody can miss.
///
/// READ SIDE: a journal ALREADY on disk carries the old encoding, and its
/// stored hash covers bytes this predicate no longer produces. That is no
/// longer this predicate's problem for the JOURNAL — `computed_checksum` now
/// hashes the checksum material exactly as it was written, so any encoding a
/// producer stored the hash of verifies, and
/// `session_journal::restore_explicit_null_receipt` only restores the value
/// fidelity the decode loses. The SNAPSHOT digest still re-encodes, and still
/// depends on [`LegacyEffectReceiptEncoding`].
fn is_absent_json_value(value: &Option<serde_json::Value>) -> bool {
    if LEGACY_EFFECT_RECEIPT_ENCODING.with(std::cell::Cell::get) {
        value.is_none()
    } else {
        matches!(value, None | Some(serde_json::Value::Null))
    }
}

thread_local! {
    /// Selects the pre-23B-H1 encoding of the `effect_receipt` fields for the
    /// duration of a single serialization. Never observable outside the scope
    /// of a [`LegacyEffectReceiptEncoding`] guard.
    static LEGACY_EFFECT_RECEIPT_ENCODING: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Scopes the encoding `effect_receipt` serializes under.
///
/// Enabled, `Some(Value::Null)` writes an explicit `"effect_receipt":null` —
/// exactly what the pre-23B-H1 writer emitted, so a journal or snapshot
/// written by that build re-hashes to the digest stored beside it. Disabled
/// (the default, and what every write path uses) it is skipped like `None`.
///
/// The guard restores the previous value on drop, so a panic mid-serialization
/// cannot leave the thread in legacy mode.
pub(crate) struct LegacyEffectReceiptEncoding(bool);

impl LegacyEffectReceiptEncoding {
    pub(crate) fn scoped(enabled: bool) -> Self {
        Self(LEGACY_EFFECT_RECEIPT_ENCODING.with(|mode| mode.replace(enabled)))
    }
}

impl Drop for LegacyEffectReceiptEncoding {
    fn drop(&mut self) {
        LEGACY_EFFECT_RECEIPT_ENCODING.with(|mode| mode.set(self.0));
    }
}

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
    PolicyDenied {
        policy: String,
    },
    HookDenied {
        reason: String,
    },
    BudgetDenied {
        reason: String,
    },
    CircuitOpen,
    UnknownTool,
    ApprovalDenied {
        approval_id: String,
    },
    ApprovalCancelled {
        approval_id: String,
    },
    ApprovalTimedOut {
        approval_id: String,
    },
    InvalidInput {
        error: String,
    },
    DispatchFailed {
        error: String,
    },
    Cancelled {
        reason: String,
    },
    /// The attempt was interrupted with its outcome unobserved, and the
    /// declared effect contract makes a re-dispatch under the SAME durable
    /// idempotency key converge on exactly one external effect.
    ///
    /// This is the only not-started reason that does NOT assert the effect
    /// failed to land. It asserts something narrower and checkable: whatever
    /// happened, re-issuing this exact execution under the key already recorded
    /// for it cannot add a second effect, so the attempt is terminalized FOR
    /// RE-DISPATCH rather than answered. `resume_recovered_tool_round` is its
    /// only writer and its only reader; a session that stops between writing it
    /// and re-dispatching leaves this receipt behind, and the next resume
    /// re-dispatches from it — which is why it names the reconciler that
    /// vouched for the claim.
    RedispatchableUnderDurableKey {
        reconciler: String,
    },
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
    /// C-4b — opaque provider signature over the turn's reasoning (Gemini
    /// `thoughtSignature` on a thought part). Journaled so a recovered turn
    /// replays the signed thought verbatim instead of stripping the signature.
    ThinkingSignature {
        signature: String,
    },
    Done {
        stop_reason: serde_json::Value,
        finish_reason: serde_json::Value,
        usage: serde_json::Value,
    },
    Error {
        message: String,
    },
    /// T3 — a tool call the provider severed at its OUTPUT token cap. Recorded
    /// so the durable stream says what the live stream said: this attempt was
    /// cut mid-call and produced nothing runnable. Additive to the tagged wire
    /// contract; journals written before it simply never carry the variant.
    TruncatedToolCall {
        name: String,
        partial_arg_bytes: u64,
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
        #[serde(default, skip_serializing_if = "is_absent_json_value")]
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
    /// Retained writer-derived authority for one delegated mutation. The
    /// reducer derives the opaque token digest from this event's committed
    /// sequence and checksum; callers cannot provide it.
    ChildTransactionOpened {
        opening: ChildTransactionOpening,
    },
    /// One content-addressed delegated-mutation receipt bound to its durable
    /// opening token.
    ChildTransactionReceiptCommitted {
        transaction_id: String,
        opening_token_digest: String,
        receipt_digest: String,
        receipt: ChildTransactionReceipt,
    },
    /// Parent-landing authority events. Each is minted ONLY through the
    /// authorized `append_conditionally` path (the public `append` denylist
    /// rejects them) and is bound to its durable opening token, so neither the
    /// lower-layer swarm primitive, the child, nor a snapshot-shaped caller can
    /// mint a landing, recovery, or rollback authority. The reducer validates
    /// every transition against the exact prior lifecycle state.
    ChildTransactionLandingPrepared {
        transaction_id: String,
        opening_token_digest: String,
        subject: LandingSubject,
    },
    ChildTransactionLandingRefAdvanced {
        transaction_id: String,
        opening_token_digest: String,
        successor: LandingSuccessor,
    },
    ChildTransactionLandingProjected {
        transaction_id: String,
        opening_token_digest: String,
        successor: LandingSuccessor,
    },
    ChildTransactionLanded {
        transaction_id: String,
        opening_token_digest: String,
        successor: LandingSuccessor,
    },
    ChildTransactionLandingConflict {
        transaction_id: String,
        opening_token_digest: String,
        detail: String,
    },
    ChildTransactionLandingRecoveryRequired {
        transaction_id: String,
        opening_token_digest: String,
        detail: String,
    },
    ChildTransactionRollbackPrepared {
        transaction_id: String,
        opening_token_digest: String,
        successor: LandingSuccessor,
    },
    ChildTransactionRolledBack {
        transaction_id: String,
        opening_token_digest: String,
        successor: LandingSuccessor,
    },
    /// Durable Goal lifecycle records (F22-02).
    ///
    /// These enter additively at schema 5 with no version bump, on the shape
    /// authorized by the 22-01 cross-binary determination. Like the child
    /// transaction authority events they are refused by the public `append`
    /// path: only `crate::goal::GoalKernel` may mint a Goal transition, so a
    /// transition with no attributable kernel append cannot exist.
    GoalOpened {
        goal_id: String,
        objective: String,
        authority: GoalAuthorityRecord,
        opened_at_unix_ms: u64,
    },
    GoalIterationStarted {
        goal_id: String,
        iteration: u32,
    },
    GoalWaitBegun {
        goal_id: String,
        wait: WaitKind,
    },
    GoalWaitResolved {
        goal_id: String,
    },
    /// A new process picked this Goal up after a crash.
    GoalRunResumed {
        goal_id: String,
        resume_count: u32,
    },
    GoalTerminated {
        goal_id: String,
        terminal: GoalTerminalState,
    },
    /// A strategy claimed the Goal's ONE loop owner (F22C, Success Criterion 3).
    ///
    /// The claim is durable rather than a call-stack frame precisely because a
    /// call stack does not survive the restart 22-03 inflicts. The reducer
    /// refuses a second claim while one is live, which is the nesting refusal:
    /// "no nested verification/retry owner" becomes a durable rule instead of a
    /// comment. A refused claim leaves the Goal non-terminal and resumable — a
    /// refusal that poisoned the Goal would be worse than the nesting it
    /// prevented.
    GoalLoopOwnerClaimed {
        goal_id: String,
        strategy: GoalStrategy,
        epoch: u32,
        /// Wall clock at the claim, supplied by the claimant. The reducer is a
        /// deterministic replay and cannot read a clock, so time enters the
        /// durable boundary as data — the same way the task ledger's claim does.
        now_unix_ms: u64,
        /// When this claim stops being evidence that an owner is alive.
        lease_expires_unix_ms: u64,
    },
    /// The claimed loop owner released its claim AND terminated the Goal, in ONE
    /// event (F22C).
    ///
    /// Atomic on purpose. Splitting it into a release followed by a terminate
    /// would open a window in which a plain `GoalTerminated` could be appended
    /// by something that never held the claim, which is the exact bypass this
    /// pair exists to close: while a claim is live the reducer refuses a plain
    /// `GoalTerminated`, so the canonical strategy transition is the only route
    /// to a terminal state for any Goal that ever claimed an owner.
    GoalLoopOwnerFinished {
        goal_id: String,
        epoch: u32,
        terminal: GoalTerminalState,
    },
    /// Durable Fleet task ledger records (F22-03).
    ///
    /// These extend the Goal's own chain rather than opening a second store, so
    /// a crash cannot leave a Goal and its tasks disagreeing. Like every other
    /// Goal variant they are refused by the public `append` path: only
    /// `crate::goal::GoalLedger` may mint one.
    GoalTaskDeclared {
        goal_id: String,
        task_id: String,
        depends_on: BTreeSet<String>,
        idempotency_key: String,
    },
    GoalTaskTransitioned {
        goal_id: String,
        task_id: String,
        transition: GoalTaskTransition,
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
    #[serde(default, skip_serializing_if = "is_absent_json_value")]
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

/// Exact journal/snapshot authority retained when a child transaction opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildTransactionSnapshotBinding {
    pub session_id: String,
    pub storage_identity_digest: String,
    pub binding_schema_version: u32,
    pub durable_authority_generation: String,
    pub snapshot_schema_version: u32,
    pub cursor: Option<u64>,
    pub cursor_checksum: String,
    pub state_digest: String,
    pub binding_digest: String,
}

/// Immutable transaction subject chosen by parent authority before effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildTransactionOpening {
    pub transaction_id: String,
    pub child_id: ChildId,
    pub child_declaration_id: String,
    pub child_revision: u64,
    pub workspace_id: String,
    pub base_revision: String,
    pub request_digest: String,
    pub policy_digest: String,
    pub gate_plan: ChildGatePlan,
    pub snapshot: ChildTransactionSnapshotBinding,
}

/// One receipt plus the exact durable-child revision that authorized it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommittedChildTransactionReceipt {
    pub opening_token_digest: String,
    pub receipt_digest: String,
    pub receipt: ChildTransactionReceipt,
    pub child_snapshot: DurableChildRecord,
}

/// Exact expected parent/candidate identity bound when a landing is prepared,
/// before the parent-owned compare-and-swap primitive is invoked. Every field
/// is the value the upper layer requires to still hold at swap time; the
/// `preimage_digest` binds the whole parent preimage the swarm primitive
/// computed under its lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandingSubject {
    /// The 06C acceptance receipt digest that authorized this integration.
    pub accepted_receipt_digest: String,
    /// The fully-qualified target branch ref the landing advances.
    pub target_ref: String,
    /// The commit the accepted candidate was forked from (its base).
    pub base_commit: String,
    /// The commit the target ref must currently name (the CAS `<old>`).
    pub expected_commit: String,
    /// The tree the expected commit points at.
    pub expected_tree: String,
    /// The symbolic ref `HEAD` names, if symbolic.
    pub symbolic_head: Option<String>,
    /// The tree the parent index recorded before the swap.
    pub index_tree: String,
    /// A digest binding the clean parent worktree status before the swap.
    pub worktree_digest: String,
    /// Identity of the parent-owned cross-process landing lock.
    pub lock_identity: String,
    /// Digest binding the whole parent preimage the primitive captured.
    pub preimage_digest: String,
}

/// The exact successor identity a landing advanced the parent to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandingSuccessor {
    pub landed_commit: String,
    pub landed_tree: String,
    pub quarantine_ref: String,
}

/// The reduced lifecycle state of one delegated-mutation landing. Transitions
/// are validated deterministically by the reducer; each carries the exact
/// successor identity where one exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LandingState {
    /// `LandingPrepared` appended; the CAS primitive has not returned terminal.
    Prepared,
    /// The target ref advanced by the exact CAS; projection pending.
    RefAdvanced { successor: LandingSuccessor },
    /// Index/worktree projected; final verification pending.
    Projected { successor: LandingSuccessor },
    /// Terminal success: ref, symbolic HEAD, index, and worktree all coherent.
    Landed { successor: LandingSuccessor },
    /// Rollback authorized while the landed successor is still live.
    RollbackPrepared { successor: LandingSuccessor },
    /// The landing was exactly reversed by reverse CAS.
    RolledBack { successor: LandingSuccessor },
    /// Fail-closed: parent drift/conflict stopped the landing; no mutation.
    Conflict { detail: String },
    /// An inconsistency requires explicit resolution; no foreign overwrite.
    RecoveryRequired { detail: String },
}

/// The reduced landing lifecycle for one transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandingRecord {
    pub subject: LandingSubject,
    pub state: LandingState,
}

/// Deterministic replay projection for one delegated-mutation transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildTransactionState {
    pub opening: ChildTransactionOpening,
    pub opening_seq: u64,
    pub opening_checksum: String,
    pub opening_token_digest: String,
    pub receipts: Vec<CommittedChildTransactionReceipt>,
    /// The landing lifecycle, present once a landing has been prepared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<LandingRecord>,
}

impl ChildTransactionState {
    /// The digest of the latest committed receipt, if any.
    ///
    /// The 20-12 acceptance pipeline reopens the durable state after committing
    /// its authoritative receipt and matches this against the digest its receipt
    /// closure computed, so acceptance rests on the durably reduced receipt
    /// rather than on the in-memory bytes it appended.
    #[must_use]
    pub fn latest_receipt_digest(&self) -> Option<&str> {
        self.receipts
            .last()
            .map(|committed| committed.receipt_digest.as_str())
    }

    /// The reduced landing lifecycle state, if a landing has been prepared.
    #[must_use]
    pub fn landing_state(&self) -> Option<&LandingState> {
        self.landing.as_ref().map(|record| &record.state)
    }
}

/// Where a durable Goal is in its lifecycle.
///
/// The set is closed and every transition the kernel can write lands in exactly
/// one of these. `Opened` is distinct from `Running` on purpose: a Goal that has
/// been authorized but has not yet consumed an iteration of its loop bound is
/// not the same thing as one that has, and collapsing them would make the bound
/// off by one on the resume path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GoalLifecycle {
    /// Authorized, no iteration consumed yet.
    Opened,
    /// Executing an iteration.
    Running,
    /// Not executing, blocked on something named.
    Waiting { wait: WaitKind },
    /// Finished, in exactly one canonical terminal category.
    Terminated { terminal: GoalTerminalState },
}

/// Deterministic replay projection for one durable Goal.
///
/// Every field is reconstructed by replaying the chain. Nothing here is carried
/// in memory across a load, which is what makes the chain — not the kernel — the
/// source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalState {
    pub goal_id: String,
    pub objective: String,
    pub authority: GoalAuthorityRecord,
    pub lifecycle: GoalLifecycle,
    /// Iterations consumed against the recorded loop bound.
    pub iterations_started: u32,
    /// How many times this Goal has been resumed after a crash.
    pub resume_count: u32,
    pub opened_at_unix_ms: u64,
    /// Journal sequence of this Goal's most recent transition.
    pub last_transition_seq: u64,
    /// Journal checksum at this Goal's most recent transition.
    pub last_transition_checksum: String,
    /// The durable task ledger for this Goal (F22-03).
    ///
    /// Tasks hang off the Goal rather than off a second top-level map for the
    /// reason 22-03 names: a Goal with two sources of truth can disagree after
    /// a crash. `skip_serializing_if` carries the same weight it does on
    /// `ReducedSessionState::goals` — a Goal with no tasks must serialize
    /// exactly as it did before this field existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tasks: BTreeMap<String, GoalTaskState>,
    /// The ONE loop owner currently executing this Goal (F22C), if any.
    ///
    /// `skip_serializing_if` carries the same weight it does on `tasks`: a Goal
    /// that never claimed an owner must serialize EXACTLY as it did before this
    /// field existed, or 22-01's M1 byte-identity determination stops holding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_owner: Option<GoalLoopOwner>,
    /// How many loop-owner claims this Goal has ever granted (F22C).
    ///
    /// Kept separately from [`Self::loop_owner`] because the claim is cleared on
    /// finish and the next epoch must still be the successor of the last one —
    /// a counter that reset with the claim would let a stale termination value
    /// match a fresh claim. Same `skip_serializing_if` reasoning: a Goal that
    /// never claimed an owner serializes exactly as it did before F22C.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub loop_owner_epochs: u32,
}

/// `skip_serializing_if` predicate for a counter whose absence means zero.
/// Takes `&u32` because that is the signature serde requires.
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// The single strategy currently executing a Goal (F22C, Success Criterion 3).
///
/// Durable, not a call-stack frame. The `epoch` binds a termination to the claim
/// it came from: a `GoalLoopOwnerFinished` naming a stale epoch is refused, so a
/// termination value produced by an earlier run cannot terminate a later one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalLoopOwner {
    /// Which of the five engines owns the loop.
    pub strategy: GoalStrategy,
    /// Monotonic claim counter for this Goal.
    pub epoch: u32,
    /// When this claim stops being evidence that an owner is alive.
    ///
    /// Without this a `kill -9` deadlocked the Goal permanently: the claim
    /// outlived the process holding it and no successor could ever claim or
    /// terminate. Measured live, on the shipped binary, by killing a run
    /// mid-wave. Task claims in the same ledger already carried a lease for
    /// exactly this reason and the loop-owner claim did not, which was an
    /// asymmetry rather than a design.
    ///
    /// Reclaim is safe because of the epoch, not in spite of it:
    /// `GoalLoopOwnerFinished` requires the LIVE epoch, so the moment a
    /// successor claims `epoch + 1` a resurrected predecessor's termination is
    /// refused. The lease supplies only the liveness evidence; the epoch
    /// supplies the exclusion.
    pub lease_expires_unix_ms: u64,
}

impl GoalLoopOwner {
    /// Whether this claim is still evidence that an owner is alive.
    ///
    /// A live claim refuses a nested one. An expired claim does not — the owner
    /// that held it is gone, and refusing forever would be a durable deadlock
    /// dressed up as a safety property.
    #[must_use]
    pub fn is_live_at(&self, now_unix_ms: u64) -> bool {
        now_unix_ms < self.lease_expires_unix_ms
    }
}

impl GoalState {
    /// The recovery cursor a reconnecting host resumes from.
    ///
    /// This is the protocol crate's EXISTING cursor shape — journal sequence
    /// plus digest — rather than a second definition. Two cursor definitions
    /// over one journal are guaranteed to drift, and the host contract in plan
    /// 22-04 has to hand exactly this to a reconnecting Desktop.
    #[must_use]
    pub fn cursor(&self) -> RecoveryCursor {
        RecoveryCursor {
            journal_sequence: Some(self.last_transition_seq),
            journal_digest: self.last_transition_checksum.clone(),
        }
    }

    /// Whether this Goal has reached a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self.lifecycle, GoalLifecycle::Terminated { .. })
    }

    /// Whether every dependency of `task` carries a durable completion.
    ///
    /// A dependency that is claimed, running, revoked or unknown does NOT
    /// count: the ledger releases a dependent on a completion that survived to
    /// disk, never on one that was merely observed in memory.
    #[must_use]
    pub fn dependencies_met(&self, task: &GoalTaskState) -> bool {
        task.depends_on.iter().all(|dependency| {
            self.tasks
                .get(dependency)
                .is_some_and(|state| state.completion.is_some())
        })
    }

    /// The tasks a worker may claim right now, in deterministic order.
    ///
    /// Excludes tasks that already carry a completion, that hold a live claim,
    /// that await explicit resolution, and whose dependencies are unmet.
    #[must_use]
    pub fn claimable_tasks(&self) -> Vec<&GoalTaskState> {
        if self.is_terminal() {
            return Vec::new();
        }
        self.tasks
            .values()
            .filter(|task| {
                task.completion.is_none()
                    && task.live_attempt().is_none()
                    && !task.requires_resolution()
                    && self.dependencies_met(task)
            })
            .collect()
    }
}

/// What one attempt at a task is currently doing, or why it stopped (F22-03).
///
/// The set is closed and an attempt is in exactly one of these. `Unknown` is
/// deliberately NOT a kind of failure: a failed attempt says the effect did not
/// happen, an unknown one says the ledger cannot tell, and those are settled
/// differently. Collapsing them is how a silent retry gets built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GoalTaskAttemptStatus {
    /// Owned by a live claim.
    Live,
    /// The claim was revoked. The owner may still be running; that is exactly
    /// what the epoch is for.
    Revoked { reason: String },
    /// The attempt produced a durable completion.
    Completed,
    /// The attempt's outcome could not be established.
    Unknown { reason: TaskUnknownReason },
}

/// One claim on a task, and the budget reservation that paid for it.
///
/// `budget_reservation_id` names a reservation committed through the EXISTING
/// budget events. The reducer refuses an attempt naming a reservation that does
/// not exist, which is what stops a reassignment from minting a fresh budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTaskAttempt {
    /// The monotonic claim epoch this attempt owns.
    pub epoch: u64,
    pub worker_id: String,
    pub budget_reservation_id: String,
    pub lease_expires_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_liveness_unix_ms: Option<u64>,
    pub status: GoalTaskAttemptStatus,
}

/// A task's completion, durable at the moment it was PRODUCED.
///
/// `delivered` is a separate field on purpose: production and delivery are two
/// events, and a worker that finishes and dies before the parent observes it
/// still has a completion here. That is the outbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTaskCompletion {
    /// The claim epoch that produced it.
    pub epoch: u64,
    /// The outcome, in the ONE canonical terminal taxonomy — never a second
    /// vocabulary invented for tasks.
    pub outcome: GoalTerminalState,
    /// Digest identifying the effect the attempt produced.
    pub effect_digest: String,
    /// Whether the parent has observed this completion.
    pub delivered: bool,
}

/// One workspace ownership handoff, and the delegated-mutation transaction that
/// authorized it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTaskHandoff {
    pub from_epoch: u64,
    pub to_epoch: u64,
    pub transaction_id: String,
    pub to_worker: String,
}

/// One transition in the durable task ledger.
///
/// EVERY variant that can produce or record an effect on behalf of a task
/// carries `epoch`, and the reducer compares it against the task's committed
/// epoch before applying anything. That comparison is the fencing property: a
/// superseded owner presenting the epoch it won is refused at the durable
/// boundary, not by each caller remembering to check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transition", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GoalTaskTransition {
    /// A worker won the task. `epoch` must be the successor of the committed
    /// epoch, so two workers racing cannot both win.
    Claimed {
        epoch: u64,
        worker_id: String,
        budget_reservation_id: String,
        lease_expires_unix_ms: u64,
    },
    /// The owner proved it is still alive.
    LivenessProved { epoch: u64, at_unix_ms: u64 },
    /// A supervisor revoked the claim. The owner may still be running.
    ClaimRevoked { epoch: u64, reason: String },
    /// The owner produced a durable completion.
    Completed {
        epoch: u64,
        outcome: GoalTerminalState,
        effect_digest: String,
    },
    /// The attempt's outcome could not be established. Requires resolution;
    /// never a silent retry.
    OutcomeUnknown {
        epoch: u64,
        reason: TaskUnknownReason,
    },
    /// The parent observed a durable completion.
    CompletionDelivered { epoch: u64 },
    /// Workspace ownership moved to a new owner through a delegated-mutation
    /// transaction that must already exist in reduced state.
    WorkspaceHandedOff {
        epoch: u64,
        to_epoch: u64,
        transaction_id: String,
        to_worker: String,
        budget_reservation_id: String,
        lease_expires_unix_ms: u64,
    },
}

impl GoalTaskTransition {
    /// The claim epoch this transition presents as its authority.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        match self {
            Self::Claimed { epoch, .. }
            | Self::LivenessProved { epoch, .. }
            | Self::ClaimRevoked { epoch, .. }
            | Self::Completed { epoch, .. }
            | Self::OutcomeUnknown { epoch, .. }
            | Self::CompletionDelivered { epoch }
            | Self::WorkspaceHandedOff { epoch, .. } => *epoch,
        }
    }
}

/// Deterministic replay projection for one durable task.
///
/// Every field is reconstructed by replaying the chain. There is no epoch
/// counter, no claim table and no outbox held anywhere else: the in-memory
/// ledger is this projection and nothing more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTaskState {
    pub task_id: String,
    /// Task ids that must carry a durable completion before this one is
    /// claimable.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub depends_on: BTreeSet<String>,
    /// The key that makes the task's EFFECT idempotent at the effect boundary.
    /// The ledger fences who may record a completion; this is what stops the
    /// effect itself from landing twice when an attempt is legitimately retried.
    pub idempotency_key: String,
    /// One entry per claim, oldest first. The last entry's epoch is the
    /// committed epoch; there is no separate counter to disagree with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<GoalTaskAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<GoalTaskCompletion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoffs: Vec<GoalTaskHandoff>,
    /// How many times this task transitioned from blocked to claimable. The
    /// exactly-once-unblock property is a count, not an assertion.
    pub dependency_releases: u64,
    pub last_transition_seq: u64,
    pub last_transition_checksum: String,
}

impl GoalTaskState {
    /// The committed claim epoch. Zero means never claimed.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.attempts.last().map_or(0, |attempt| attempt.epoch)
    }

    /// The attempt that currently holds a live claim, if any.
    #[must_use]
    pub fn live_attempt(&self) -> Option<&GoalTaskAttempt> {
        self.attempts
            .last()
            .filter(|attempt| matches!(attempt.status, GoalTaskAttemptStatus::Live))
    }

    /// Whether an operator or reconciler must settle this task before anything
    /// else may happen to it.
    #[must_use]
    pub fn requires_resolution(&self) -> bool {
        self.attempts
            .last()
            .is_some_and(|attempt| matches!(attempt.status, GoalTaskAttemptStatus::Unknown { .. }))
    }

    /// Whether this task carries a durable completion the parent has not yet
    /// observed. This is the outbox a restarted parent drains.
    #[must_use]
    pub fn completion_pending_delivery(&self) -> bool {
        self.completion
            .as_ref()
            .is_some_and(|completion| !completion.delivered)
    }

    /// Total budget reserved across every attempt of this task.
    #[must_use]
    pub fn reserved_total(&self, budgets: &BTreeMap<String, BudgetState>) -> u64 {
        self.attempts
            .iter()
            .filter_map(|attempt| budgets.get(&attempt.budget_reservation_id))
            .map(|budget| budget.reserved.value)
            .sum()
    }
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub child_transactions: BTreeMap<String, ChildTransactionState>,
    /// Durable Goals (F22-02).
    ///
    /// `skip_serializing_if` is NOT cosmetic here and must not be removed: the
    /// 22-01 determination measured that an existing journal reduces to a
    /// byte-identical state under a binary carrying this field, and that
    /// property holds only while a session with no Goal serializes exactly as it
    /// did before. `goal_journal_compat_test.rs` pins it against the real
    /// retained corpus and goes red if this attribute is dropped.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub goals: BTreeMap<String, GoalState>,
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
            child_transactions: BTreeMap::new(),
            goals: BTreeMap::new(),
            deliveries: BTreeMap::new(),
        }
    }
}
