use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::anvil::{AnvilReceipt, AnvilReceiptInvalidation};
use crate::diagnostics::{RuntimeDiagnosticsSnapshotV1, RuntimeDiagnosticsUnavailableReason};

pub use wcore_types::message::FinishReason;

/// Serde helper: skip serializing a `bool` field when it is `false`.
///
/// Used on W0 forward-additive `Capabilities` flags so default-off flags
/// don't appear in the serialized JSON — preserving v0.1.21 shape for
/// hosts that haven't learned about them yet. Removing this helper or
/// changing its semantics breaks the W0 invariant; the golden tests in
/// `tests/golden_v0_1_21.rs` will catch any regression.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Stable identities for capabilities whose production activation must be
/// proved rather than inferred from registration or source presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityId {
    PricingRefresher,
    MidFlightMonitor,
    CooldownTracker,
    LearnedPolicy,
    SmartHandoff,
    DelegateIsolation,
    ProcedureSkillDrafting,
    LegacyAutoSkillDrafting,
}

/// Append-only activation stages. A ready capability may be reached more than
/// once; every successful occurrence completes the
/// `reached -> outcome_changed -> observed` cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStage {
    Declared,
    Configured,
    Constructed,
    Ready,
    Reached,
    OutcomeChanged,
    Observed,
    Unavailable,
}

impl CapabilityStage {
    /// Whether `next` is a legal next event for one capability within a
    /// session. `Observed -> Reached` starts another successful occurrence.
    pub fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Declared, Self::Configured | Self::Unavailable)
                | (Self::Configured, Self::Constructed | Self::Unavailable)
                | (Self::Constructed, Self::Ready | Self::Unavailable)
                | (Self::Ready | Self::Observed, Self::Reached)
                | (Self::Reached, Self::OutcomeChanged)
                | (Self::OutcomeChanged, Self::Observed)
        )
    }
}

/// Stable reasons why an activation chain ended before readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityReasonCode {
    DisabledByConfig,
    DependencyUnavailable,
    NoProductionConstructor,
    RuntimePathUnwired,
    IsolationNotEnforced,
}

/// One typed claim in a capability's activation chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityActivation {
    pub capability: CapabilityId,
    pub stage: CapabilityStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<CapabilityReasonCode>,
}

impl CapabilityActivation {
    pub const fn stage(capability: CapabilityId, stage: CapabilityStage) -> Self {
        Self {
            capability,
            stage,
            reason: None,
        }
    }

    pub const fn unavailable(capability: CapabilityId, reason: CapabilityReasonCode) -> Self {
        Self {
            capability,
            stage: CapabilityStage::Unavailable,
            reason: Some(reason),
        }
    }

    /// Reject reason-bearing live stages and reason-less unavailability.
    pub const fn is_well_formed(&self) -> bool {
        matches!(
            (self.stage, self.reason),
            (CapabilityStage::Unavailable, Some(_))
                | (
                    CapabilityStage::Declared
                        | CapabilityStage::Configured
                        | CapabilityStage::Constructed
                        | CapabilityStage::Ready
                        | CapabilityStage::Reached
                        | CapabilityStage::OutcomeChanged
                        | CapabilityStage::Observed,
                    None
                )
        )
    }
}

/// Typed control-flow directive emitted by the mid-flight monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorDirective {
    Replan,
    Stop,
}

/// Stable reason classes for monitor control-flow decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorReason {
    OutputStall,
    RepeatedError,
    RepeatedToolRoute,
    BudgetExceeded,
}

/// Stable workflow-node lifecycle states exported to host control planes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Blocked,
}

/// Stable terminal states for one workflow execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowTerminalState {
    Succeeded,
    Failed,
}

/// Stable terminal disposition for one child run within a workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowChildTerminalState {
    Succeeded,
    Failed,
}

/// Typed, provider-neutral failure evidence for workflow lifecycle events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// Correlation metadata attached to a workflow child-agent relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowChildCorrelation {
    pub run_id: String,
    pub child_run_id: String,
    pub parent_child_run_id: Option<String>,
    pub child_sequence: u64,
    pub event_id: String,
    pub terminal_state: Option<WorkflowChildTerminalState>,
}

/// Complete producer payload for a correlated workflow start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunStarted {
    pub workflow_id: String,
    pub name: String,
    pub node_count: usize,
    pub run_id: String,
    pub event_id: String,
    pub sequence: u64,
    pub parent_run_id: Option<String>,
}

/// Complete producer payload for one workflow-node lifecycle transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowNodeLifecycle {
    pub run_id: String,
    pub node_id: String,
    pub child_run_id: Option<String>,
    pub event_id: String,
    pub sequence: u64,
    pub state: WorkflowNodeState,
    pub failure: Option<WorkflowFailure>,
}

/// Complete producer payload for a correlated workflow terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunFinished {
    pub workflow_id: String,
    pub run_id: String,
    pub event_id: String,
    pub sequence: u64,
    pub terminal_state: WorkflowTerminalState,
    pub failure: Option<WorkflowFailure>,
}

/// Opaque, content-bound position in the durable session journal.
///
/// `journal_sequence = None` is the unambiguous genesis position. The digest
/// is required even at genesis so a host cannot advance recovery using an
/// unbound numeric offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCursor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal_sequence: Option<u64>,
    pub journal_digest: String,
}

/// Operator-observed result for a tool effect whose authoritative outcome
/// cannot be reconstructed by a Core reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorToolEffectOutcome {
    Succeeded,
    Failed,
    NotStarted,
}

/// Closed vocabulary for the external record an operator used to resolve an
/// otherwise unknown tool effect. Unknown sources are authority-critical and
/// fail deserialization rather than degrading to an untyped label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorResolutionEvidenceSource {
    ToolReceipt,
    ProviderReceipt,
    ProcessObservation,
    ExternalSystemRecord,
}

/// Content-bound evidence for an operator resolution. Evidence contains only
/// an opaque reference and digest; it never carries tool arguments, output,
/// credentials, or free-form authority claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorResolutionEvidence {
    pub source: OperatorResolutionEvidenceSource,
    pub reference_id: String,
    pub observed_at_unix_ms: u64,
    pub digest: String,
}

/// Cursor-bound authority claim shared by the host command and Core receipt.
/// The closed shape makes unknown authority-bearing additions fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorToolEffectResolution {
    pub recovery_version: u16,
    pub session_id: String,
    pub turn_id: String,
    pub cursor: RecoveryCursor,
    pub tool_execution_id: String,
    pub outcome: OperatorToolEffectOutcome,
    pub operator_id: String,
    pub evidence: OperatorResolutionEvidence,
}

/// Stable recovery lifecycle exposed to both standalone and hosted clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryLifecycle {
    Ready,
    Streaming,
    AwaitingApproval,
    ToolInFlight,
    ReconciliationRequired,
    Suspended,
    Completed,
    Cancelled,
    Failed,
}

/// Fail-closed reasons why Core cannot produce a trustworthy recovery view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryUnavailableReason {
    SessionNotFound,
    UnsupportedVersion,
    CursorInvalid,
    CursorAhead,
    CursorDigestMismatch,
    HistoryGap,
    JournalCorrupt,
    SnapshotUnavailable,
    UnknownCriticalState,
}

/// Typed reasons why an interrupted turn cannot be continued directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReconcileReason {
    ApprovalExpired,
    ProviderOutcomeUnknown,
    ToolOutcomeUnknown,
    EffectRequiresOperator,
    BudgetExhausted,
    ContextUnrestorable,
    CancellationAmbiguous,
    UnknownCriticalState,
}

/// Sanitized interrupted-turn projection. It deliberately contains only
/// opaque identifiers and typed state: never transcript text, prompts, tool
/// arguments or output, paths, approval secrets, or provider payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryTurnSnapshot {
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    pub lifecycle: RecoveryLifecycle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconcile_reason: Option<RecoveryReconcileReason>,
}

/// Sanitized budget projection needed to make a safe resume decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryBudgetSnapshot {
    pub tokens_used: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<u64>,
    pub cost_used_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_limit_usd: Option<f64>,
}

/// Terminal disposition for one correlated provider-budget grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetGrantOutcome {
    /// The budget mutation was applied exactly once.
    Granted,
    Refused,
}

/// Closed refusal vocabulary for budget grants. Hosts must not infer policy
/// state from human-readable strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetGrantRefusalReason {
    HostNotAuthorized,
    ManagedPolicy,
    NoExhaustedBudget,
    InvalidGrant,
    BudgetTrackerUnavailable,
    PersistenceFailure,
    RequestIdConflict,
    LedgerCapacityExceeded,
    /// A grant cannot be accepted while its turn is still executing. After
    /// the terminal turn event, retry with a fresh request id; replaying the
    /// refused id returns the same terminal refusal.
    TurnInProgress,
}

/// Content-bound result cached by Core for at-most-once grant application.
/// Identical `request_id` replay emits the exact stored value; conflicting
/// reuse emits a refusal and never mutates the stored result.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetGrantResult {
    pub request_id: String,
    pub additional_tokens: u64,
    pub additional_cost_usd: f64,
    pub outcome: BudgetGrantOutcome,
    pub refusal_reason: Option<BudgetGrantRefusalReason>,
}

impl Serialize for BudgetGrantResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if !crate::commands::is_valid_budget_grant_request_id(&self.request_id) {
            return Err(serde::ser::Error::custom(
                "budget grant result has an invalid request_id",
            ));
        }
        if !self.additional_cost_usd.is_finite() || self.additional_cost_usd < 0.0 {
            return Err(serde::ser::Error::custom(
                "budget grant result cost must be finite and non-negative",
            ));
        }
        match (self.outcome, self.refusal_reason) {
            (BudgetGrantOutcome::Granted, None) | (BudgetGrantOutcome::Refused, Some(_)) => {}
            (BudgetGrantOutcome::Granted, Some(_)) => {
                return Err(serde::ser::Error::custom(
                    "granted budget result must omit refusal_reason",
                ));
            }
            (BudgetGrantOutcome::Refused, None) => {
                return Err(serde::ser::Error::custom(
                    "refused budget result must include refusal_reason",
                ));
            }
        }

        #[derive(Serialize)]
        struct Wire<'a> {
            request_id: &'a str,
            additional_tokens: u64,
            additional_cost_usd: f64,
            outcome: BudgetGrantOutcome,
            #[serde(skip_serializing_if = "Option::is_none")]
            refusal_reason: Option<BudgetGrantRefusalReason>,
        }

        Wire {
            request_id: &self.request_id,
            additional_tokens: self.additional_tokens,
            additional_cost_usd: self.additional_cost_usd,
            outcome: self.outcome,
            refusal_reason: self.refusal_reason,
        }
        .serialize(serializer)
    }
}

impl BudgetGrantResult {
    pub fn granted(request_id: String, additional_tokens: u64, additional_cost_usd: f64) -> Self {
        Self {
            request_id,
            additional_tokens,
            additional_cost_usd,
            outcome: BudgetGrantOutcome::Granted,
            refusal_reason: None,
        }
    }

    pub fn refused(
        request_id: String,
        additional_tokens: u64,
        additional_cost_usd: f64,
        refusal_reason: BudgetGrantRefusalReason,
    ) -> Self {
        Self {
            request_id,
            additional_tokens,
            additional_cost_usd,
            outcome: BudgetGrantOutcome::Refused,
            refusal_reason: Some(refusal_reason),
        }
    }
}

impl<'de> Deserialize<'de> for BudgetGrantResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            request_id: String,
            additional_tokens: u64,
            additional_cost_usd: f64,
            outcome: BudgetGrantOutcome,
            refusal_reason: Option<BudgetGrantRefusalReason>,
        }

        let wire = Wire::deserialize(deserializer)?;
        if !crate::commands::is_valid_budget_grant_request_id(&wire.request_id) {
            return Err(serde::de::Error::custom(
                "budget grant result has an invalid request_id",
            ));
        }
        if !wire.additional_cost_usd.is_finite() || wire.additional_cost_usd < 0.0 {
            return Err(serde::de::Error::custom(
                "budget grant result cost must be finite and non-negative",
            ));
        }
        match (wire.outcome, wire.refusal_reason) {
            (BudgetGrantOutcome::Granted, None) => Ok(Self::granted(
                wire.request_id,
                wire.additional_tokens,
                wire.additional_cost_usd,
            )),
            (BudgetGrantOutcome::Refused, Some(reason)) => Ok(Self::refused(
                wire.request_id,
                wire.additional_tokens,
                wire.additional_cost_usd,
                reason,
            )),
            (BudgetGrantOutcome::Granted, Some(_)) => Err(serde::de::Error::custom(
                "granted budget result must omit refusal_reason",
            )),
            (BudgetGrantOutcome::Refused, None) => Err(serde::de::Error::custom(
                "refused budget result must include refusal_reason",
            )),
        }
    }
}

/// Content-free milestone kinds that may be replayed to reconstruct recovery
/// UI without exposing journal payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReplayKind {
    /// A committed journal transition with no more-specific public milestone.
    /// This keeps replay cursors contiguous without exposing event payloads.
    StateAdvanced,
    TurnStarted,
    StreamStarted,
    StreamCommitted,
    ApprovalRequested,
    ApprovalResolved,
    ToolStarted,
    ToolCommitted,
    EffectUncertain,
    CancellationRequested,
    TurnCompleted,
    TurnCancelled,
    TurnFailed,
}

/// One ordered, sanitized recovery milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReplayItem {
    pub cursor: RecoveryCursor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub kind: RecoveryReplayKind,
}

/// Why `Ready.session_id` holds what it holds.
///
/// `ready` publishes `session_id` as its correlation key, and a host keys its
/// own session tracking on it. It can legitimately be absent — a run with no
/// durable session has no id to give — and until this type existed the
/// producer expressed that by dropping the key off the wire entirely. A host
/// then received `undefined` with no accompanying signal and could not tell
/// "degraded" from "malformed frame" from "an older Core". That passes schema
/// validation, because the field is optional, while breaking the consumer.
///
/// So the absence is now stated rather than implied: `session_id` is always
/// serialized (`null` when there is none) and this field says which cause
/// produced the value it holds. The causes are NOT interchangeable — one is the
/// operator's choice, the others are host limitations the operator may want to
/// fix — and collapsing them is the same mistake as omitting the key.
///
/// # The fourth value, and why a three-value enum was not enough
///
/// This type shipped with three values, on the premise that a host which cannot
/// protect a durable session turns durable sessions OFF. That premise stopped
/// being true the same night: the session journal is not encrypted, and the
/// confidential store protects exactly one field — the sealed copy of the exact
/// provider request that makes AUTOMATIC replay possible. A keyless host now
/// journals without it.
///
/// That produced a state none of the three values described. `session_id` is a
/// real, resumable id, so `disabled_by_*` is plainly wrong; but the session
/// cannot recover an interrupted dispatch by itself, so `durable` over-claims
/// to precisely the consumer that most needs to know — one deciding whether to
/// wait for auto-recovery or ask its operator. Reporting it as `durable` would
/// have been the same defect this type was introduced to fix, one layer up: a
/// wire value that cannot express a state the product reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPersistence {
    /// `session_id` names a durable, journaled session with crash replay. It
    /// survives a restart, can be resumed, and a turn interrupted mid-dispatch
    /// resumes itself from the sealed provider request.
    Durable,
    /// `session_id` names a durable, journaled session WITHOUT crash replay.
    ///
    /// The journal is complete — every turn, provider attempt, tool call,
    /// approval and delivery boundary is recorded, so nothing executes
    /// unrecorded and history survives a restart. What is missing is the sealed
    /// copy of the exact provider request, because this host has no usable OS
    /// keyring and no unlocked credentials vault.
    ///
    /// **What a host should do.** Treat the session as durable for history and
    /// audit: list it, offer resume, keep it. Do NOT show auto-recovery
    /// affordances or wait on one. If a turn is interrupted mid-dispatch, the
    /// next message on that session is refused with a reconciliation error
    /// naming the interrupted turn — surface a resume / reconcile / cancel
    /// choice to the operator rather than a retry spinner. And if a resume is
    /// refused with `init_failed` naming the session, that session is LOCKED
    /// pending a key, not corrupt: leave its journal alone, because restoring
    /// `WAYLAND_VAULT_PASSPHRASE_FD` and resuming again recovers it.
    JournaledWithoutReplay,
    /// `session_id` is null: the operator turned durable sessions off
    /// (`[session] enabled = false`). Nothing is journaled and nothing is
    /// resumable, by request.
    DisabledByOperator,
    /// DECODE-ONLY. `session_id` is null because a host that could not protect
    /// a durable session turned durable sessions off.
    ///
    /// **This producer can no longer emit it**, and that is not an oversight —
    /// see [`SessionPersistence`]'s own docs. It is retained because the value
    /// was published on the wire, so an older Core still sends it and a host
    /// may have stored it against a session it is still tracking. Removing a
    /// value we once sent breaks those consumers for no gain.
    ///
    /// A host meeting it should read it as "an older Core, on a keyless host,
    /// which journaled nothing" — historical, and not something a current Core
    /// will ever say again.
    DisabledByHost,
}

/// A `critical` classification that can only ever be `false`.
///
/// The W0 host decoder contract lets a host drop an unknown event ONLY when
/// the frame explicitly says `"critical": false`; a missing or `true`
/// classification is a hard error. An event that wants to be safely ignorable
/// by an older host must therefore carry the field — and must never be able to
/// carry the other value, because a producer that flipped it would turn "your
/// host is a version behind" into "your host disconnects".
///
/// Making that structural rather than a convention is the whole point: there is
/// no constructor for the `true` case, so no call site can get it wrong and the
/// published schema's `const: false` can never reject one of our own frames.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NonCritical;

impl Serialize for NonCritical {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(false)
    }
}

/// The CLOSED media-type vocabulary a [`ProtocolEvent::RenderArtifact`] may
/// carry.
///
/// Closed for the same reason `ready.session_persistence` is (see
/// `contract/generate.rs`): a future value must not be able to arrive as free
/// text and be accepted by a host that has never heard of it. Widening the
/// vocabulary is therefore an announced contract event — a minor bump plus a
/// schema change — rather than a silent dialect that renders as a blank pane on
/// half the installed hosts.
///
/// Deliberately text-only. #1098 says the Desktop half should define any
/// image/binary carriage; inventing a base64 image envelope here would be Core
/// dictating a wire shape it cannot render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RenderMime {
    #[serde(rename = "text/plain")]
    Plain,
    #[serde(rename = "text/markdown")]
    Markdown,
    #[serde(rename = "text/html")]
    Html,
}

impl RenderMime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "text/plain",
            Self::Markdown => "text/markdown",
            Self::Html => "text/html",
        }
    }

    /// Parse a wire token. `None` for anything outside the closed vocabulary —
    /// callers refuse rather than defaulting, so an unrenderable kind never
    /// reaches a host as some other kind.
    pub fn from_wire(token: &str) -> Option<Self> {
        match token {
            "text/plain" => Some(Self::Plain),
            "text/markdown" => Some(Self::Markdown),
            "text/html" => Some(Self::Html),
            _ => None,
        }
    }

    /// Every declared token, in wire order. The contract generator and the
    /// docs test both read this so the enum stays the single source of truth.
    pub fn all() -> &'static [&'static str] {
        &["text/plain", "text/markdown", "text/html"]
    }
}

/// Byte cap on [`ProtocolEvent::RenderArtifact`]'s `title`.
pub const RENDER_ARTIFACT_TITLE_LIMIT_BYTES: usize = 256;

/// Byte cap on [`ProtocolEvent::RenderArtifact`]'s `content`.
///
/// NOT a taste call. `output_pump.rs` REJECTS any frame larger than
/// `MAX_QUEUED_BYTES` (8 MiB) and, on rejection, sets `sticky_failure` and
/// calls `record_failure()` — whose `failed` flag is sticky, so every later
/// write returns `BrokenPipe`. One oversized render frame would not merely
/// fail to display: it would kill stdout for the rest of the session.
///
/// 1 MiB is the largest round number that provably survives that. serde_json's
/// worst case for a String is a 6x expansion (`\u00XX` for every control byte),
/// so 1 MiB of content is at most 6 MiB on the wire, and title plus envelope
/// plus the truncation marker leave the 8 MiB frame limit intact. The fit is
/// asserted against the real pump constant in this module's tests, not
/// asserted about in a comment.
pub const RENDER_ARTIFACT_CONTENT_LIMIT_BYTES: usize = 1024 * 1024;

/// The in-band notice that stands in for the bytes past the cap.
///
/// In-band because the bytes are: a host renders `content` and nothing else, so
/// a marker in the text is the only truncation signal that reaches a reader who
/// is looking at the rendered surface rather than at the frame. The sibling
/// `truncated` flag is for the host chrome; this is for the human.
fn render_truncation_marker(kept: usize) -> String {
    format!(
        "\n\n[wcore: CONTENT TRUNCATED. This artifact is larger than the \
         {RENDER_ARTIFACT_CONTENT_LIMIT_BYTES}-byte render cap. The {kept} bytes above are \
         the START of it; everything after them is not shown here. Nothing was \
         modified — only what is displayed was cut.]\n"
    )
}

/// Clamp `content` to [`RENDER_ARTIFACT_CONTENT_LIMIT_BYTES`], returning the
/// bytes to send and whether anything was cut.
///
/// Crossing the cap TRUNCATES; it never discards. Discarding inverts the cap's
/// own purpose — the same argument `wcore-sandbox`'s buffered-output cap makes
/// (FerroxLabs/wayland#1071), where dropping 20 MB of output handed the caller
/// 129 bytes, none of them the ones that explained anything.
///
/// The cut lands on a UTF-8 character boundary, so a multi-byte character
/// straddling the cap is dropped whole rather than emitted as a broken prefix.
pub fn truncate_render_content(content: &str) -> (String, bool) {
    if content.len() <= RENDER_ARTIFACT_CONTENT_LIMIT_BYTES {
        return (content.to_string(), false);
    }
    let mut cut = RENDER_ARTIFACT_CONTENT_LIMIT_BYTES;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut kept = content[..cut].to_string();
    kept.push_str(&render_truncation_marker(cut));
    (kept, true)
}

/// Clamp `title` to [`RENDER_ARTIFACT_TITLE_LIMIT_BYTES`] on a character
/// boundary. A title is chrome, so an over-long one is silently shortened
/// rather than refused — refusing would lose the artifact over its label.
pub fn truncate_render_title(title: &str) -> String {
    if title.len() <= RENDER_ARTIFACT_TITLE_LIMIT_BYTES {
        return title.to_string();
    }
    let mut cut = RENDER_ARTIFACT_TITLE_LIMIT_BYTES;
    while cut > 0 && !title.is_char_boundary(cut) {
        cut -= 1;
    }
    title[..cut].to_string()
}

/// Events emitted by the agent to the client (Agent -> Client)
///
/// `Clone` is derived (Wave 2) so the in-process TUI bridge can fan an
/// event out across the protocol writer and the channel-backed sink
/// without re-serializing.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ProtocolEvent {
    Ready {
        version: String,
        /// ALWAYS serialized, `null` when this run has no durable session.
        /// Deliberately NOT `skip_serializing_if`: this is the wire type's
        /// declared correlation key (`EVENT_SPECS`), and a correlation key
        /// that can vanish is indistinguishable from a bug at the consumer.
        /// [`SessionPersistence`] carries the reason for a null.
        session_id: Option<String>,
        /// Why `session_id` holds what it holds. Never omitted.
        session_persistence: SessionPersistence,
        capabilities: Capabilities,
        /// Pinned producer contract for contract-aware hosts. This remains
        /// optional on the Rust type so legacy fixtures can prove their old
        /// shape, while production Ready emission always supplies it.
        #[serde(skip_serializing_if = "Option::is_none")]
        contract: Option<crate::contract::ContractDescriptor>,
        /// Initial complete policy snapshot for contract-aware hosts. Legacy
        /// producers/tests may omit it; the JSON-stream producer always sets
        /// it before accepting a turn.
        #[serde(skip_serializing_if = "Option::is_none")]
        execution_policy: Option<crate::execution_policy::ExecutionPolicySnapshot>,
    },
    /// Complete effective execution-policy snapshot. This is output-only:
    /// wire peers cannot deserialize it into authority.
    ExecutionPolicy {
        #[serde(flatten)]
        snapshot: crate::execution_policy::ExecutionPolicySnapshot,
    },
    /// Effective repository trust and sandbox grants. Output-only authority
    /// receipt; hosts cannot submit this shape to widen a session.
    WorkspacePolicy {
        policy: wcore_types::workspace_trust::WorkspacePolicyReceipt,
    },
    /// Complete sanitized recovery projection at one durable journal cursor.
    SessionRecoverySnapshot {
        recovery_version: u16,
        request_id: String,
        session_id: String,
        cursor: RecoveryCursor,
        state_digest: String,
        lifecycle: RecoveryLifecycle,
        #[serde(skip_serializing_if = "Option::is_none")]
        pending_turn: Option<RecoveryTurnSnapshot>,
        budget: RecoveryBudgetSnapshot,
    },
    /// Ordered, content-free milestones after a host-provided cursor.
    SessionRecoveryReplay {
        recovery_version: u16,
        request_id: String,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<RecoveryCursor>,
        through: RecoveryCursor,
        items: Vec<RecoveryReplayItem>,
    },
    /// Typed refusal to recover when Core cannot prove a trustworthy view.
    SessionRecoveryUnavailable {
        recovery_version: u16,
        request_id: String,
        session_id: String,
        reason: RecoveryUnavailableReason,
    },
    /// Durable lifecycle transition for one recoverable turn.
    TurnRecoveryLifecycle {
        recovery_version: u16,
        session_id: String,
        turn_id: String,
        cursor: RecoveryCursor,
        lifecycle: RecoveryLifecycle,
        #[serde(skip_serializing_if = "Option::is_none")]
        reconcile_reason: Option<RecoveryReconcileReason>,
    },
    /// Durable receipt for a validated operator resolution of an otherwise
    /// unknown tool effect. This event echoes the exact authority-bound input
    /// so hosts can replay it without inventing state.
    UnknownToolEffectResolved {
        #[serde(flatten)]
        resolution: OperatorToolEffectResolution,
    },
    /// Typed capability construction/runtime evidence. Startup events are
    /// emitted after `Ready`; runtime events are emitted at the real success
    /// seam. Unknown hosts drop this additive event.
    CapabilityActivation {
        #[serde(flatten)]
        activation: CapabilityActivation,
    },
    StreamStart {
        msg_id: String,
    },
    TextDelta {
        text: String,
        msg_id: String,
    },
    Thinking {
        text: String,
        msg_id: String,
        /// #318 — optional per-turn thinking SUBJECT: a short opaque display
        /// label for the reasoning block (Flux `reasoning_summary`). Present
        /// only on the subject-carrying chunk (where `text` is typically
        /// empty); omitted from the JSON on ordinary thinking-text chunks so
        /// the v0 wire shape is preserved for hosts that don't read it.
        #[serde(skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
    },
    ToolRequest {
        msg_id: String,
        call_id: String,
        tool: ToolInfo,
    },
    /// A tool call that is about to run WITHOUT being asked about - force
    /// mode, an allow-listed tool, a command-scoped auto-approval, a recovered
    /// approval, or a tool the host already granted `ApprovalScope::Always`.
    ///
    /// # Why this is not just a `tool_request`
    ///
    /// `tool_request` carries two meanings that are mutually exclusive here.
    /// The Wayland desktop host uses it as the registration anchor - every
    /// later `tool_*` frame is matched against the `call_id` it registered, and
    /// an unregistered `call_id` fails the session closed mid-turn. It ALSO
    /// renders that frame as an approve/deny card. Emitting `tool_request` for
    /// an auto-approved call would therefore ask the operator to confirm a tool
    /// they have already permanently allowed, which is a worse defect than the
    /// one it fixes.
    ///
    /// # Why the name has no `tool_` prefix
    ///
    /// DELIBERATE, DO NOT "TIDY". The desktop validator routes on
    /// `type.startsWith('tool_')` and fails closed on any `tool_`-prefixed type
    /// whose `call_id` it has not already registered. A `tool_announced` would
    /// reproduce exactly the mid-turn kill this event exists to prevent.
    ///
    /// # Compatibility
    ///
    /// Additive and safe for hosts that predate it: an unknown, non-`tool_`
    /// type falls through the desktop decoder's default arm and is dropped with
    /// a warning, the same path `workspace_policy` already takes. This matters
    /// because the engine is updated INDEPENDENTLY of the desktop app by its
    /// in-app updater, so a new engine routinely runs under an older host.
    CallAnnounced {
        msg_id: String,
        call_id: String,
        tool: ToolInfo,
    },
    ToolRunning {
        msg_id: String,
        call_id: String,
        tool_name: String,
    },
    ToolResult {
        msg_id: String,
        call_id: String,
        tool_name: String,
        status: ToolStatus,
        output: String,
        output_type: OutputType,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    ToolCancelled {
        msg_id: String,
        call_id: String,
        reason: String,
    },
    StreamEnd {
        msg_id: String,
        /// Why the stream ended. Required; engine emits `Error` if it can't
        /// classify the provider's stop signal. Host UIs should render
        /// `Length` as a truncation warning (closes the Gemini Pro
        /// reasoning-token empty-response bug at the protocol contract).
        finish_reason: FinishReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// CORE-2: per-run usage delta — the tokens consumed by THIS run only
        /// (summing every provider round-trip of the run's tool loop), while
        /// `usage` stays session-cumulative for back-compat. Same inner field
        /// names as `usage`. None on synthetic stream-ends and on paths that
        /// don't track a run-scoped delta.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage_delta: Option<Usage>,
        /// #279(c): stable per-run correlation handle grouping every event of
        /// one agent run (survives multi-message / --resume). None on synthetic
        /// stream-ends (slash/exit/stop) that aren't a model run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_run_id: Option<String>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        error: ErrorInfo,
    },
    Info {
        msg_id: String,
        message: String,
    },
    ConfigChanged {
        capabilities: Capabilities,
    },
    McpReady {
        name: String,
        tools: Vec<String>,
    },
    /// An MCP server failed (or timed out) at connect. The companion to
    /// [`McpReady`]: it carries the preserved failure cause so a host /
    /// the TUI `/doctor` view can tell the user *why* a server's tools never
    /// appeared, instead of the server silently vanishing. Additive — hosts
    /// that don't recognise it drop it per the W0 decoder contract.
    McpFailed {
        name: String,
        reason: String,
    },
    /// Terminal receipt for a correlated runtime-MCP removal request.
    McpRemovalResult {
        lifecycle_version: u16,
        request_id: String,
        name: String,
        outcome: McpRemovalOutcome,
        removed_tools: Vec<String>,
    },
    /// Correlated, versioned, redacted effective runtime state for local host
    /// diagnostics. This contains no environment values, launch arguments,
    /// request headers, credentials, or raw process errors.
    RuntimeDiagnosticsSnapshot {
        diagnostics_version: u16,
        request_id: String,
        snapshot: RuntimeDiagnosticsSnapshotV1,
    },
    /// Correlated rejection for a diagnostics request that this producer
    /// cannot satisfy. Hosts must treat this as terminal for `request_id`.
    RuntimeDiagnosticsUnavailable {
        diagnostics_version: u16,
        supported_version: u16,
        request_id: String,
        reason: RuntimeDiagnosticsUnavailableReason,
    },
    /// W1: F9 structured trace for one turn. Gated by the W0-reserved
    /// `capabilities.structured_traces` flag — the engine only emits this
    /// variant when the corresponding ProtocolSink builder was configured
    /// with `with_structured_traces(true)`. Hosts that don't recognise the
    /// `trace_event` `type` MUST drop it silently per the W0 host decoder
    /// contract; hosts that opt in surface it via their trace UI.
    ///
    /// The trace payload is `serde_json::Value` rather than a typed
    /// `TurnTrace` so this crate stays independent of `wcore-observability`
    /// (which depends on `wcore-config`, which would otherwise create a
    /// downstream protocol-crate dependency).
    TraceEvent {
        msg_id: String,
        trace: Value,
    },
    /// W6 F7: end-of-session cost aggregate. Gated by the W0-reserved
    /// `capabilities.cost_attribution` flag — engine emits this variant
    /// only when `AdvertisedCapabilitiesConfig.cost_attribution = true`
    /// (bootstrap flips this when ProviderCompat has cost rows; single
    /// authority per audit rev-2 finding 5). Hosts that don't recognise the
    /// `session_cost` `type` MUST drop it silently per the W0 host decoder
    /// contract; hosts that opt in surface it via their cost UI.
    ///
    /// Per-turn cost still rides inside `TraceEvent.trace.cost_usd` (gated
    /// by `capabilities.structured_traces`); this variant is the typed
    /// aggregate for hosts that don't want to parse trace JSON.
    SessionCost {
        session_id: String,
        total_cost_usd: f64,
        per_turn: Vec<TurnCost>,
    },
    /// W7: F2 sub-agent event. The inner payload is a serialized
    /// `ProtocolEvent` (kept as `serde_json::Value` here to avoid a
    /// recursive variant — the engine serializes the sub-agent's event
    /// to a Value before wrapping). `parent_call_id` groups events
    /// emitted by sub-agents spawned by a single `SpawnTool` call.
    /// Gated by the W0-reserved `capabilities.sub_agent_traces` flag —
    /// the engine only emits this when the corresponding ProtocolSink
    /// builder was configured with `with_sub_agent_traces(true)`. Hosts
    /// that don't recognise the `sub_agent_event` `type` MUST drop it
    /// silently per the W0 host decoder contract.
    SubAgentEvent {
        parent_call_id: String,
        agent_name: String,
        inner: Value,
    },
    /// Correlated v1 form of `sub_agent_event`. It deliberately serializes
    /// with the legacy wire tag while keeping the old Rust variant available
    /// to in-process TUI consumers. Hosts therefore see one additive event
    /// shape rather than a second event type.
    #[serde(rename = "sub_agent_event")]
    CorrelatedSubAgentEvent {
        parent_call_id: String,
        agent_name: String,
        inner: Value,
        run_id: String,
        child_run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_child_run_id: Option<String>,
        child_sequence: u64,
        event_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_state: Option<WorkflowChildTerminalState>,
    },
    /// ForgeFlows-Live: a workflow (ForgeFlows / Dynamic Workflows) run
    /// started. Emitted once, before the first node dispatches, so hosts
    /// (the TUI Workflows tab and the external `wayland` desktop app) get a
    /// clean lifecycle signal instead of inferring the run from the first
    /// `workflow:<node_id>`-prefixed `SubAgentEvent`. `workflow_id` is a
    /// stable correlation handle for the run; `name` is the author's display
    /// name; `node_count` is the number of lifecycle nodes in the run graph.
    /// Rides the existing W0-reserved `capabilities.sub_agent_traces` flag
    /// (the same observability surface as `SubAgentEvent`) — no dedicated
    /// capability is added. Hosts that don't recognise the `workflow_started`
    /// `type` MUST drop it silently per the W0 host decoder contract.
    WorkflowStarted {
        workflow_id: String,
        name: String,
        node_count: usize,
    },
    /// Correlated v1 form of `workflow_started`; see
    /// [`ProtocolEvent::CorrelatedSubAgentEvent`] for the compatibility
    /// rationale behind the shared wire tag.
    #[serde(rename = "workflow_started")]
    CorrelatedWorkflowStarted {
        workflow_id: String,
        name: String,
        node_count: usize,
        run_id: String,
        event_id: String,
        sequence: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_run_id: Option<String>,
    },
    /// One ordered transition for a node in a correlated workflow run.
    WorkflowNodeEvent {
        run_id: String,
        node_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        child_run_id: Option<String>,
        event_id: String,
        sequence: u64,
        state: WorkflowNodeState,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<WorkflowFailure>,
    },
    /// ForgeFlows-Live: a workflow run finished. Emitted once, after the run
    /// completes (success or failure), as the terminal bookend to
    /// `WorkflowStarted`. `succeeded` is `true` only when the run produced
    /// no errored stages. Rides the existing `capabilities.sub_agent_traces`
    /// flag (no dedicated capability). Hosts that don't recognise the
    /// `workflow_finished` `type` MUST drop it silently per the W0 host
    /// decoder contract.
    WorkflowFinished {
        workflow_id: String,
        succeeded: bool,
    },
    /// Correlated v1 form of `workflow_finished`. `succeeded` is retained for
    /// legacy hosts and always agrees with `terminal_state`.
    #[serde(rename = "workflow_finished")]
    CorrelatedWorkflowFinished {
        workflow_id: String,
        succeeded: bool,
        run_id: String,
        event_id: String,
        sequence: u64,
        terminal_state: WorkflowTerminalState,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<WorkflowFailure>,
    },
    /// W7: F4 streaming tool-result chunk. Long-running tools (e.g.
    /// `Bash` on a multi-minute build) emit one of these per chunk of
    /// stdout/stderr while running, ahead of the final `ToolResult`.
    /// Gated by the W0-reserved `capabilities.streaming_tools` flag
    /// (`ProtocolSink::with_streaming_tools(true)`). Hosts that don't
    /// recognise `tool_chunk` MUST drop it silently; the existing
    /// `ToolResult` still arrives at the end carrying the full
    /// buffered output for buffered hosts.
    ToolChunk {
        msg_id: String,
        call_id: String,
        tool_name: String,
        chunk: String,
    },
    /// W7: F8 provider circuit-breaker state transition. Emitted when
    /// `ResilientProvider` transitions between Closed / Open / HalfOpen,
    /// or when a fallback provider is engaged. NOT gated by an opt-in
    /// flag — circuit transitions are always-visible diagnostics, like
    /// `Error`. (Documented under "errors are always allowed" in
    /// `docs/json-stream-protocol.md` Host Decoder Contract section.)
    ///
    /// **Design rationale (rev-2, audit F4):** The W0 capability pattern
    /// is host-advertisement of decoder capability, NOT host-opt-in of
    /// emission. Errors today are always emitted because hosts that
    /// don't know `error` still drop the line silently per the W0
    /// forward-compat baseline. Same logic applies here:
    /// `provider_circuit_event` is a failure-mode diagnostic — opting
    /// in for it would mean a buggy host renders no fallback indication
    /// for an entire incident. The always-on choice is consistent with
    /// W0 (cross-audit approved 2026-05-15).
    ProviderCircuitEvent {
        primary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        fallback: Option<String>,
        /// "closed" | "open" | "half_open"
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// F15: deterministic provider-failover decision evidence. The provider
    /// crate owns the typed receipt; the protocol carries its serialized shape
    /// opaquely to preserve crate layering and forward-compatible decoding.
    ProviderFailoverReceipt {
        receipt: serde_json::Value,
    },
    /// One physical provider request attempt. Always emitted so evaluators and
    /// hosts can distinguish real recovery from a fixture-side request count.
    /// Unknown hosts drop this additive event under the W0 decoder contract.
    ProviderAttempt {
        /// Stable failure class (`http_503`, `timeout`, `stream_truncated`, ...).
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
    },
    /// Core scheduled another provider attempt after a typed failure. Kept
    /// separate from `ProviderAttempt` so a retry decision never inflates the
    /// physical-attempt count.
    ProviderRetry {
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
    },
    /// A typed provider failure discovered after the physical send completed
    /// (for example a truncated SSE body). It does not imply a retry.
    ProviderFailure {
        failure: String,
    },
    /// F10 always-on structured monitor decision. Hosts use this additive
    /// event to distinguish a deliberate stop/replan from a generic engine
    /// error or informational string.
    MidFlightMonitorDecision {
        directive: MonitorDirective,
        reason: MonitorReason,
    },
    /// W7: S4 approval requested — engine wants the host's permission
    /// before proceeding with `call_id`. `resume_token` echoes back in
    /// the host's `ApprovalResume` command. Gated by the W0-reserved
    /// `capabilities.hitl_suspend` flag.
    ///
    /// **Wave SC SECURITY MAJOR (correlation-id model).** The
    /// `correlation_id` field is the opaque public handle the host UI
    /// uses to match this `ApprovalRequired` against the eventual
    /// resolution. It always equals `call_id`.
    ///
    /// `resume_token` is NOT the same value, and is not always present:
    ///
    /// - **Bridge-backed** approvals (Crucible council, egress consent)
    ///   carry the unguessable bridge SECRET, and are answered with
    ///   `ProtocolCommand::ApprovalResume { resume_token }`. The secret
    ///   is deliberately not the `call_id`, which the model can see —
    ///   routing on it would let a tool approve itself (GHSA-8r7g).
    /// - **Ordinary tool** gates have no bridge entry, so `resume_token`
    ///   is the EMPTY STRING. They are answered with
    ///   `ProtocolCommand::ToolApprove` / `ToolDeny`, keyed by `call_id`.
    ///   A host that echoes the empty token back in `ApprovalResume`
    ///   resolves nothing and the tool hangs until its TTL.
    ///
    /// `ProtocolSink::redact_tokens` strips in-flight secrets from
    /// streaming tool output as defense-in-depth against tools that
    /// snoop stdout.
    ApprovalRequired {
        call_id: String,
        resume_token: String,
        /// Wave SC opaque handle for UI matching. Same value as
        /// `resume_token` in this revision; future revisions may
        /// diverge the two.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        correlation_id: String,
        reason: String,
        context: String,
        /// Crucible Stage 2 — the typed proposal card, set only when a council
        /// approval is requested; `None` for every other approval.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<wcore_types::crucible::CruciblePlan>,
    },
    /// W7: S4 session is in Suspended state — emitted alongside
    /// ApprovalRequired so hosts that render a state pill can update
    /// independently of the modal flow.
    Suspend {
        reason: String,
        resume_token: String,
    },
    /// W7: S4 approval resolved — engine echoes the resume decision
    /// back so the host can clear UI state regardless of who emitted
    /// the corresponding command (CLI, UI, plugin).
    ApprovalResume {
        resume_token: String,
        approved: bool,
    },
    /// W8a A.7: ExecutionBudget cap exceeded — singular event per
    /// session, fires once when the first cap trips. Always-emitted +
    /// host-tolerated additive variant per audit F5: hosts that don't
    /// know the `budget_exceeded` type drop the line silently per the
    /// W0 host decoder contract, so no dedicated capability flag is
    /// reserved. `reason` is one of the deterministic
    /// `ExecutionBudgetView::first_exceeded_reason()` strings
    /// (`max_wall_time`, `max_tool_runtime`, `max_concurrent_process_tools`,
    /// `max_agent_depth`, `max_tokens_in`, `max_tokens_out`,
    /// `max_cost_usd`); `observed` and `limit` are human-readable
    /// formatted strings (e.g. `"62.0s"` / `"60.0s"`, `"16384"` / `"4096"`).
    BudgetExceeded {
        reason: String,
        observed: String,
        limit: String,
    },
    /// Correlated result for `continue_with_budget`. This is the sole
    /// authority-bearing acknowledgement; free-form `info` text is not a
    /// grant receipt.
    BudgetGrantResult {
        #[serde(flatten)]
        result: BudgetGrantResult,
    },
    /// Wave RB RELIABILITY MAJOR: a tool's `execute_with_ctx` panicked.
    /// The orchestration dispatcher caught the panic via
    /// `tokio::task::JoinError::is_panic()` and converted it to a
    /// structured `ToolResult { is_error: true, content: "Tool panicked: ..." }`
    /// so the LLM context sees a normal tool failure and the session
    /// continues. This event is emitted ALONGSIDE the synthetic
    /// `ToolResult` so a host can render the panic as a distinct
    /// diagnostic (vs. a normal `is_error: true` ToolResult).
    ///
    /// Always-on (no capability flag) — same rationale as `Error`,
    /// `BudgetExceeded`, and `ProviderCircuitEvent`: panic-recovery
    /// diagnostics are always-visible per the audit F4 W0 design.
    /// Hosts that don't recognise `tool_panicked` MUST drop the line
    /// silently per the W0 host decoder contract.
    ToolPanicked {
        msg_id: String,
        call_id: String,
        tool_name: String,
        /// Best-effort panic message extracted from the `JoinError`'s
        /// payload (downcast to `&str` / `String`). May be `"<non-string panic payload>"`
        /// if the panic used a non-string payload type.
        panic_message: String,
    },
    /// Wave RB STABILITY MINOR #10: a plugin failed to register with the
    /// host because one of its `Scoped*Registry::new(...)` calls returned
    /// an error other than the expected `AccessDenied` "permission not
    /// requested" sentinel. The plugin still loads — partial registration
    /// is allowed — but the host can render a diagnostic so the user
    /// understands why a tool/hook/etc. they expected is missing.
    ///
    /// Always-on (no capability flag) — same rationale as `Error` and
    /// `ProviderCircuitEvent`: plugin-registration failures are failure
    /// diagnostics. Hosts that don't recognise `plugin_registration_failed`
    /// drop the line silently per the W0 host decoder contract.
    PluginRegistrationFailed {
        plugin_name: String,
        /// Which scoped registry failed (e.g. `"tools"`, `"hooks"`,
        /// `"agents"`, `"skills"`, `"rules"`, `"mcp"`, `"providers"`).
        surface: String,
        /// The `PluginError` rendered via Display.
        error_kind: String,
        message: String,
    },
    /// W8a H.1: opaque event emitted by a registered plugin. Gated by
    /// the W0-reserved `capabilities.plugins` flag — the engine only
    /// emits this variant in sessions that advertise plugins=true (set
    /// in `build_capabilities` once at least one plugin is loaded).
    /// `plugin_name` matches the plugin's manifest name; `event_type`
    /// is plugin-defined free-form (e.g. `"memory_capture"`,
    /// `"index_rebuild_complete"`); `payload` is the plugin-supplied
    /// JSON value. Hosts that don't recognise the variant drop it
    /// silently per the W0 forward-compat baseline.
    PluginEvent {
        plugin_name: String,
        event_type: String,
        payload: Value,
    },
    /// W10B: F12 GEPA evolution event. Emitted at every scored child when the
    /// host has the `gepa_enabled` capability advertised. Older hosts that
    /// don't know this variant drop it silently per the W0 host decoder
    /// contract.
    ///
    /// `evolution_event` rides on its own dedicated capability flag rather
    /// than overloading `structured_traces` — see F6 audit fix in W10B
    /// rev-2: hosts that want W1 turn traces shouldn't be forced to also
    /// accept thousands of W10B events per `evolve` run.
    EvolutionEvent {
        run_id: String,
        generation: u32,
        parent_id: String,
        child_id: String,
        mutation_kind: String,
        score: f64,
        retained: bool,
    },
    /// W8c.1 E.14: browser-suite op event. Emitted by the engine once per
    /// completed browser op (Navigate, Snapshot, Click, ...) so the host
    /// can render a compact tool-call trail. Gated by the W0-reserved
    /// `capabilities.browser_suite` flag — engine advertises the flag
    /// when the wayland-browser plugin is loaded. Hosts that don't
    /// recognise `browser_event` MUST drop it silently per the W0 host
    /// decoder contract.
    BrowserEvent {
        msg_id: String,
        call_id: String,
        /// Op kind as serialized by `BrowserOp` (e.g. `"navigate"`).
        op: String,
        /// Origin / target URL when relevant (`Navigate`, `NewTab`,
        /// `Download`). `None` for ops without a URL (`Snapshot`, `Click`).
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// One-line human-readable summary (e.g. `"loaded"`,
        /// `"clicked @e3 button \"Submit\""`).
        summary: String,
    },
    /// W8c.1 E.14: a browser op was blocked by `BrowserPolicy` before
    /// dispatch — the host renders an explicit block notification so the
    /// user can react. Always emitted alongside the corresponding error
    /// `ToolResult`; the dedicated variant gives hosts a typed surface
    /// for blocked-URL telemetry. Gated by `capabilities.browser_suite`.
    BrowserPolicyDenied {
        msg_id: String,
        url: String,
        reason: String,
    },
    /// W8c.2 F.9: computer-use op event. Emitted by the engine once per
    /// completed CUA op (LeftClick, Type, Screenshot, ...) so the host
    /// can render a compact action trail. Gated by the W0-reserved
    /// `capabilities.computer_use` flag — engine advertises the flag
    /// when the wayland-cua plugin is loaded. Hosts that don't
    /// recognise `cua_event` MUST drop it silently per the W0 host
    /// decoder contract.
    CuaEvent {
        msg_id: String,
        call_id: String,
        /// Op kind as serialized by `CuaOp` (e.g. `"left_click"`).
        op: String,
        /// `[x, y]` screen coords for ops that have them (mouse/key);
        /// `None` for `Screenshot`, `AxTree`, `Wait`, `FrontmostApp`.
        #[serde(skip_serializing_if = "Option::is_none")]
        coords: Option<[i32; 2]>,
        /// One-line human-readable summary (e.g. `"clicked at (100, 200)"`,
        /// `"typed 14 chars"`).
        summary: String,
    },
    /// W8c.2 F.9: a CUA op was blocked by `CuaPolicy` before dispatch.
    /// Mirrors `BrowserPolicyDenied` — surfaces a typed channel so the
    /// host can render policy violations as a distinct notification
    /// kind. Gated by `capabilities.computer_use`.
    CuaPolicyDenied {
        msg_id: String,
        /// The op kind tag that was rejected.
        op: String,
        /// Frontmost-app id at the time of dispatch (best-effort; may
        /// be empty if the backend can't determine it).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        app: String,
        reason: String,
    },
    /// #537/#141 host-send-transport hook: the engine runs host-delegated
    /// (`WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1` at spawn) and an approved
    /// `send_message` tool call is asking the HOST to perform the actual
    /// delivery through its own outbound channel plugins (the engine's
    /// channel table is empty under the desktop). The host fulfils the send
    /// and replies with the `host_send_message_result` command, correlated
    /// by `call_id`. `platform` / `chat_id` / `thread_id` mirror the
    /// engine's `ParsedTarget`; `body` is the message text.
    ///
    /// SECURITY (wayland#543 audit finding 4): the host performs the send
    /// WITHOUT re-gating — it trusts that the engine's tool-approval flow
    /// (`tool_request` / allow-list / mode gate) already ran. This event is
    /// only ever emitted from inside `SendMessageTool::execute`, which the
    /// orchestration approval gate fronts; `send_message` is `Exec`-category
    /// and absent from every auto-approve default (see
    /// `wcore-agent/tests/host_send_delegation.rs`).
    ///
    /// Always-on additive variant (no capability flag) — hosts that don't
    /// recognise `host_send_message_request` drop it silently per the W0
    /// host decoder contract; only hosts that opted in via the env var
    /// ever receive it.
    HostSendMessageRequest {
        call_id: String,
        /// `MessagingPlatform::as_str()` token, e.g. `"email"`.
        platform: String,
        /// Recipient (for email: the destination address). Omitted when the
        /// target string carried no chat id.
        #[serde(skip_serializing_if = "Option::is_none")]
        chat_id: Option<String>,
        /// Reply-to / thread handle. Omitted when absent.
        #[serde(skip_serializing_if = "Option::is_none")]
        thread_id: Option<String>,
        /// The message text.
        body: String,
        /// Optional subject line. The current `send_message` schema carries
        /// no subject input, so the engine omits this today; the field is
        /// part of the wire contract (the desktop host threads it into the
        /// outgoing message when present).
        #[serde(skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
        /// Session id of the emitting engine, when known. Omitted otherwise.
        #[serde(skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
    /// FerroxLabs/wayland#1098: "show this to the user" as a RENDER
    /// capability instead of an OS `open`.
    ///
    /// Handing a filesystem path to LaunchServices (or `xdg-open`, or
    /// `cmd /c start`) is a filesystem + process capability doing a UI job.
    /// It is also why #1102 exists: the macOS seatbelt profile is
    /// `(deny default)` and never grants the SBPL operation `lsopen`, so
    /// `open` fails with `-54`. Granting `lsopen` would be an
    /// execution-confinement ESCAPE — a sandboxed shell could ask launchd to
    /// start any installed app OUTSIDE our profile. This event is what makes
    /// refusing that cost nothing: it needs ZERO filesystem authority at the
    /// host, works headless, works over SSH, and is identical on all three
    /// platforms.
    ///
    /// SECURITY: the payload is CONTENT, never a path. The engine-side
    /// producer (`wcore_tools::render::RenderArtifactTool`) obtains that
    /// content through the SAME vfs/policy path as an ordinary `read`, so a
    /// file the agent may not read is a file it may not render. Nothing here
    /// widens what the agent can reach; it only changes how what it already
    /// read reaches the user.
    ///
    /// `content` is UNTRUSTED — it is either model-authored or read out of the
    /// workspace. A host that renders `text/html` MUST do so in a sandboxed
    /// renderer with no host-process bridge. See
    /// `docs/json-stream-protocol.md` §1.N+13.
    ///
    /// Always-on additive variant behind the `render_artifact_v1` contract
    /// capability. Unlike every other additive event, this one carries
    /// `critical: false` EXPLICITLY: the documented host rule
    /// (`docs/json-stream-protocol.md` "Rules" §3, implemented by
    /// `tests/desktop_contract_corpus_only_host.rs`) is that an unknown type
    /// is dropped only when it says so, and hard-errors when the
    /// classification is missing. Without the field a host pinned to an older
    /// corpus would reject the frame instead of ignoring it.
    RenderArtifact {
        /// Turn the artifact belongs to. Empty when no turn is active.
        msg_id: String,
        /// The `render_artifact` tool call that produced it.
        call_id: String,
        /// Short human label for the rendered surface (tab title / card
        /// heading). Capped at [`RENDER_ARTIFACT_TITLE_LIMIT_BYTES`].
        title: String,
        /// CLOSED vocabulary — see [`RenderMime`].
        mime: RenderMime,
        /// The bytes to display, already bounded by
        /// [`RENDER_ARTIFACT_CONTENT_LIMIT_BYTES`].
        content: String,
        /// `true` when `content` is a truncated prefix and carries the in-band
        /// truncation marker. A host SHOULD badge the surface as partial.
        truncated: bool,
        /// Always the JSON literal `false`; see the variant docs.
        critical: NonCritical,
    },
    /// W5 M6 / #279(d) + #280: a context compaction occurred. Gated by
    /// capabilities.non_destructive_compact; hosts that don't recognise
    /// compact_offload MUST drop it per the host decoder contract.
    CompactOffload {
        msg_id: String,
        /// Why compaction fired (e.g. "window_pressure", "manual").
        reason: String,
        /// Tokens reclaimed. 0 when not measurable.
        tokens_freed: u64,
        /// Active-window fill AFTER compaction, same opaque 0..=100 u32 as
        /// Usage.active_window_percent (from ContextWindow::percent()).
        #[serde(skip_serializing_if = "Option::is_none")]
        active_window_percent: Option<u32>,
    },
    /// Anvil (gated-forge) receipt — the engine's honest verdict for a `/forge`
    /// climb (spec §8). Carries the terminal state, the trust-tier stamp
    /// actually earned, check counts + coverage, iterations, settled cost, and
    /// the gate/artifact digests that bind the receipt to what was verified.
    ///
    /// **TRUST BOUNDARY (normative, spec §8):** a host renders a receipt "chip"
    /// ONLY from this TOP-LEVEL variant. Receipt-shaped content arriving nested
    /// in [`ProtocolEvent::SubAgentEvent`]'s `inner` or
    /// [`ProtocolEvent::PluginEvent`]'s `payload` (both opaque `Value`) is
    /// INERT — a sub-agent or plugin can never forge a verified verdict. This
    /// holds structurally: spawned children carry no protocol writer, so their
    /// events are always wrapped (never promoted to the top level). The
    /// `host_decoder_contract` tests lock the wire-level invariant. Same class
    /// as ratchet `00364cf` (a previewed fragment cannot forge the
    /// Approve/Reject verdict).
    ///
    /// Emission is engine-only, from the climb exit path, and lands with the
    /// climb slice (A1.5/A1.6) — this variant is defined here first so the wire
    /// contract and the trust boundary can be reviewed and tested in isolation.
    /// Like [`ProtocolEvent::BudgetExceeded`], it is an additive variant a
    /// v0.1.21 host drops silently (W0 forward-compat).
    AnvilReceipt {
        /// Versioned, authoritative producer-owned receipt payload. Flattened
        /// so existing top-level `type = "anvil_receipt"` dispatch remains
        /// forward-additive while the contract gains stable identity,
        /// correlation, content binding, and replay semantics.
        #[serde(flatten)]
        receipt: AnvilReceipt,
    },
    /// A previously authoritative receipt became stale, was revoked, or was
    /// superseded. Only this top-level Core-origin event changes authority;
    /// receipt-shaped tool text and opaque nested payloads remain inert.
    AnvilReceiptInvalidated {
        #[serde(flatten)]
        invalidation: AnvilReceiptInvalidation,
    },
    /// Complete host-observable projection of one durable Goal at a cursor
    /// (F22-C1).
    ///
    /// Additive. Before this event a host could not observe a Goal in any form
    /// — durable Goals existed only on the CLI surface — so nothing about any
    /// existing event's shape changes to carry it.
    ///
    /// The projection is derived from the reduced journal state, never from
    /// anything held in memory beside it. `state_digest` is taken over the
    /// canonical JSON of the FULL reduced `GoalState`, including the parts this
    /// projection summarises, so a host can always establish which chain state
    /// its view corresponds to.
    GoalSnapshot {
        goal_version: u16,
        session_id: String,
        goal_id: String,
        cursor: RecoveryCursor,
        state_digest: String,
        goal: crate::goal::GoalProjection,
    },
    /// One durable Goal transition, as a content-free milestone (F22-C1).
    ///
    /// Carries the milestone and the cursor it landed at, not the payload —
    /// the same split `turn_recovery_lifecycle` uses against
    /// `session_recovery_snapshot`. A host that wants the state after a
    /// transition reads the next `goal_snapshot`, which keeps the transition
    /// stream cheap and keeps exactly one shape authoritative for Goal content.
    GoalTransition {
        goal_version: u16,
        session_id: String,
        goal_id: String,
        cursor: RecoveryCursor,
        transition: crate::goal::GoalTransitionKind,
        /// The lifecycle the Goal is in AFTER the transition.
        lifecycle: crate::goal::GoalLifecycleWire,
    },
    /// A host Goal CONTROL command was refused, with a typed reason (F22-C1).
    ///
    /// Additive, and mandatory rather than convenient. The five Goal commands
    /// are answered in a command loop whose match ends in a catch-all that
    /// merely logs (`wcore-cli/src/main.rs`), so without an explicit refusal
    /// event every rejected command would be indistinguishable from one that
    /// was accepted and did nothing — the advertised-but-dead shape this
    /// surface exists to avoid. `session_recovery_unavailable` is the same
    /// pattern for the recovery commands.
    ///
    /// Correlated by `request_id`, because a refusal has no cursor to correlate
    /// on: the whole point of several reasons is that the Goal or the state the
    /// host named does not exist.
    GoalControlRefused {
        goal_version: u16,
        request_id: String,
        session_id: String,
        /// The Goal the refused command named. Carried even when no such Goal
        /// exists, so a host can tell which of several in-flight requests was
        /// refused without holding its own request table.
        goal_id: String,
        reason: GoalControlRefusalReason,
    },
    /// wayland#896 — a quiescence lease was granted over profile state.
    ///
    /// `coverage.complete` is always true here: incomplete coverage is refused
    /// with `quiesce_refused`, never reported as a partial grant. `epoch` is
    /// the opaque mutation token the host echoes on release; it is the ONLY
    /// thing that settles whether the capture taken under this lease is a valid
    /// recovery point.
    QuiesceLeaseGranted {
        quiescence_version: u16,
        request_id: String,
        lease_id: String,
        session_id: String,
        epoch: String,
        coverage: crate::quiescence::QuiesceCoverage,
        acquired_unix_ms: u64,
        expires_unix_ms: u64,
        /// True when this grant re-observed a lease the same `lease_id` already
        /// held, rather than taking a fresh one. The epoch is unchanged.
        idempotent_replay: bool,
    },
    /// wayland#896 — a lease was released, with the verdict on whether the
    /// covered state moved while it was held.
    ///
    /// A `mutated` verdict is not an error: the lease worked exactly as
    /// designed and is telling the host its capture is torn. A host that
    /// stores the capture anyway has stored a snapshot that never existed.
    QuiesceLeaseReleased {
        quiescence_version: u16,
        request_id: String,
        lease_id: String,
        session_id: String,
        epoch_at_acquire: String,
        epoch_at_release: String,
        verdict: crate::quiescence::QuiesceReleaseVerdict,
        released_unix_ms: u64,
    },
    /// wayland#896 — a lapsed lease was observed and reclaimed.
    ///
    /// Expiry is OBSERVED, not scheduled: Core is not a daemon, so a holder
    /// that crashed is reported by the next acquire, release or status that
    /// meets its record. That is what makes a dead holder reclaimable rather
    /// than a permanent wedge, and this receipt is how the trail stays gapless.
    QuiesceLeaseExpired {
        quiescence_version: u16,
        /// The lease that lapsed. `<unparsable>` when the record on disk could
        /// not be decoded — such a record has no expiry to reach, so it is
        /// reclaimed rather than left to wedge the control plane forever.
        lease_id: String,
        /// Owner recorded by the lapsed lease.
        owner: String,
        /// Session that OBSERVED the expiry, which is not the owner.
        session_id: String,
        /// Request during which the expiry was observed.
        request_id: String,
        epoch_at_acquire: String,
        expires_unix_ms: u64,
        observed_unix_ms: u64,
    },
    /// wayland#896 — the lease control plane, without granting anything.
    QuiesceStatusReport {
        quiescence_version: u16,
        request_id: String,
        session_id: String,
        held: Option<crate::quiescence::QuiesceHeldLease>,
        /// Roots a lease could cover right now. A host asks for these rather
        /// than hardcoding a list that silently misses a new profile.
        available: Vec<crate::quiescence::QuiesceProfileIdentity>,
    },
    /// wayland#896 — a quiescence command was refused, with a closed reason.
    ///
    /// Mandatory rather than convenient, for the same reason
    /// `goal_control_refused` is: without an explicit refusal frame a rejected
    /// capture is indistinguishable from one that was accepted and did nothing,
    /// and a host that cannot tell those apart will store an empty recovery
    /// point believing it succeeded.
    QuiesceRefused {
        quiescence_version: u16,
        request_id: String,
        /// The lease the refused command named. Carried even when no such lease
        /// exists, so a host can attribute the refusal without its own table.
        lease_id: String,
        session_id: String,
        reason: crate::quiescence::QuiesceRefusalReason,
        /// Operator-facing detail. Never a second reason vocabulary — a host
        /// branches on `reason` alone.
        detail: String,
    },
    Pong,
}

/// Typed reasons a host Goal control command was refused (F22-C1).
///
/// Closed, and deliberately keeps causes apart that a host would otherwise be
/// tempted to retry identically. `GoalNotFound` and `CursorStale` in particular
/// settle differently: the first means the host named something that never
/// existed, the second means the host's view is behind and a resync fixes it.
/// Collapsing them is how a control plane builds a silent retry loop against a
/// Goal that will never appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalControlRefusalReason {
    /// `goal_version` is not the version this Core speaks.
    UnsupportedVersion,
    /// The command named a session that is not the live one.
    SessionNotFound,
    /// No durable Goal with that id exists in this session's journal.
    GoalNotFound,
    /// A Goal with that id already exists; `goal_open` is not an upsert.
    GoalAlreadyExists,
    /// The supplied cursor is not the Goal's current cursor — the host is
    /// acting on a view that has since moved. Resync and re-issue.
    CursorStale,
    /// The Goal has already terminated, so it cannot advance or be cancelled.
    GoalTerminated,
    /// Advancing would exceed the loop bound the Goal was authorized for.
    IterationCeilingReached,
    /// A task with that id is already declared in this Goal's ledger.
    TaskAlreadyDeclared,
    /// The task named a dependency that is not declared in this Goal's ledger.
    ///
    /// Distinct from [`Self::Malformed`] and from [`Self::JournalError`] on
    /// purpose. The ledger refuses an undeclared dependency because treating
    /// one as satisfied would release a dependent on a task that never exists;
    /// the host's fix is to declare the dependency FIRST and re-issue, which is
    /// a different action from correcting a malformed field and a very
    /// different one from retrying a failed disk write.
    DependencyNotDeclared,
    /// The command was structurally valid but a field was not usable —
    /// an empty id, an out-of-range bound.
    Malformed,
    /// This process has no durable journal, so no Goal can be controlled.
    JournalUnavailable,
    /// The journal rejected the append.
    JournalError,
}

/// Result of a session-scoped runtime MCP removal request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpRemovalOutcome {
    Removed,
    AlreadyAbsent,
    NotRuntimeManaged,
    UnsupportedVersion,
    InvalidRequest,
    RequestIdConflict,
    TurnInProgress,
    CapacityExceeded,
    CleanupUnverified,
    RegistryBusy,
}

/// W6 F7 per-turn cost row carried by [`ProtocolEvent::SessionCost`].
/// `provider` is the structured per-provider id from `ProviderCompat.provider_type()`
/// (e.g. `"anthropic"`, `"bedrock"`, `"openai"`, `"vertex"`, `"ollama"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCost {
    pub turn: usize,
    pub model: String,
    pub provider: String,
    pub cost_usd: f64,
    /// Whether `cost_usd` is a real metered or known-free price. Missing on
    /// legacy rows defaults to false so zero is not silently called free.
    #[serde(default)]
    pub priced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    // v0.1.21 baseline (shape unchanged)
    pub tool_approval: bool,
    pub thinking: bool,
    pub effort: bool,
    pub effort_levels: Vec<String>,
    /// The advertised approval modes. These MUST be the canonical wire
    /// spellings — `default` / `auto_edit` / `force` — i.e. the
    /// `#[serde(rename_all = "snake_case")]` forms of
    /// `crate::commands::SessionMode` and exactly what
    /// `ToolApprovalManager::current_mode()` emits. A host parses these back
    /// through `SessionMode`, so advertising a non-canonical spelling here
    /// (e.g. kebab `auto-edit`) re-opens the D033 round-trip downgrade.
    pub modes: Vec<String>,
    /// The active mode, in the same canonical spelling as `modes` above.
    pub current_mode: String,
    pub mcp: bool,

    // W0 — forward-additive opt-in flags. All default-false; `skip_serializing_if`
    // keeps the JSON output byte-identical to v0.1.21 when these are off.
    //
    // Setting a flag to `true` is ENGINE ADVERTISEMENT, not host opt-in: the
    // engine is signalling "I will emit the corresponding new event variants
    // this session." The host's obligation is to tolerate unknown event types
    // and unknown fields per `docs/json-stream-protocol.md` host-decoder
    // contract — emission gating in future waves is governed by
    // `wcore-config`, not by what the host has acknowledged.
    /// W7: F4 streaming tool-result chunks (e.g. `tool_chunk` events).
    #[serde(default, skip_serializing_if = "is_false")]
    pub streaming_tools: bool,

    /// W7: F2 sub-agent events streamed via ChannelSink with `parent_call_id`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sub_agent_traces: bool,

    /// W6: F7 per-turn / per-session cost events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cost_attribution: bool,

    /// W7: S4 Suspend / ApprovalRequired turn-state events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hitl_suspend: bool,

    /// W5: M6 non-destructive compaction events (`compact_offload`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub non_destructive_compact: bool,

    /// W1: F9 structured `ExecutionTrace` / `TurnTrace` events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub structured_traces: bool,

    /// W4: F13 RPC tool script (`Script` tool) trace-expansion events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rpc_tool_script: bool,

    /// W8: B1 browser tool family events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub browser_suite: bool,

    /// W8: B2 wcore-cua computer-use events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub computer_use: bool,

    /// W2.5/W8: P1 plugin-registered tools/hooks/agents visible to the host.
    #[serde(default, skip_serializing_if = "is_false")]
    pub plugins: bool,

    /// W10B: F12 GEPA `evolution_event` emission. Forward-additive; default
    /// off. Setting this true advertises that the engine will emit
    /// `evolution_event` variants during a `wcore-cli evolve` run. Hosts
    /// that haven't learned about this flag drop the variant silently per
    /// the W0 host decoder contract.
    ///
    /// `structured_traces` (W1) is no longer overloaded with
    /// `evolution_event` — F6 audit fix in W10B rev-2 split the W1 turn-
    /// trace family from the W10B per-child evolution family so hosts can
    /// opt in independently.
    #[serde(default, skip_serializing_if = "is_false")]
    pub gepa_enabled: bool,

    /// F-093 — active user-model backend tag. `"local"` (on-disk JSON) or
    /// `"honcho"` (dialectic user modeling via Honcho server). Empty string
    /// when memory is disabled. Forward-additive: hosts that haven't seen
    /// this field yet ignore it per the W0 decoder contract.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_model_backend: String,

    /// F-092 (W7-N): live-session online evolution. Forward-additive;
    /// default off. When true the engine emits `evolution_event` at
    /// session-end for every real session (not just offline `evolve` runs)
    /// and applies the Paraphrase mutator live to successful trajectories.
    /// Opt-in via `--online-evolution` CLI flag or
    /// `[observability] online_evolution = true` in config.
    #[serde(default, skip_serializing_if = "is_false")]
    pub online_evolution: bool,

    /// Rank 85 — explicit memory-enabled signal. `user_model_backend` is an
    /// empty string both when memory is disabled and when an older host
    /// doesn't know the field, so it can't disambiguate the two. This flag
    /// is emitted (`true`) only when long-term memory is on, giving the host
    /// an unambiguous bool to key on instead of inferring from the backend
    /// tag. Forward-additive; omitted when false per the W0 decoder contract
    /// (absent reads as "off or unknown").
    #[serde(default, skip_serializing_if = "is_false")]
    pub memory_enabled: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            tool_approval: false,
            thinking: false,
            effort: false,
            effort_levels: Vec::new(),
            modes: vec!["default".to_string()],
            current_mode: "default".to_string(),
            mcp: false,
            streaming_tools: false,
            sub_agent_traces: false,
            cost_attribution: false,
            hitl_suspend: false,
            non_destructive_compact: false,
            structured_traces: false,
            rpc_tool_script: false,
            browser_suite: false,
            computer_use: false,
            plugins: false,
            gepa_enabled: false,
            user_model_backend: String::new(),
            online_evolution: false,
            memory_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub category: ToolCategory,
    pub args: Value,
    pub description: String,
    /// Why this call is being shown to the user beyond the ordinary approval
    /// gate, as STRUCTURED data rather than prose in `description`.
    ///
    /// Additive and skipped when absent, so a host that has never heard of it
    /// sees the exact `tool_request` frame it saw before. A host that HAS
    /// heard of it can render the specific remedy the escalation implies
    /// (an "always allow this folder" button) instead of asking the user to
    /// read a path out of a sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub escalation: Option<ToolEscalation>,
}

/// A boundary this tool call is about to cross, named for the host.
///
/// One variant today. It is an enum rather than a bare struct because "why am
/// I being asked?" has more than one possible answer, and a host that switches
/// on `kind` keeps working when the second answer arrives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolEscalation {
    /// The call names a filesystem path outside every root this session can
    /// reach. Answering the approval with
    /// `ApprovalScope::AlwaysPath { root: suggested_root, write: false }` is
    /// guaranteed to be accepted: Core only emits this variant after
    /// dry-running that grant against the session's workspace policy, so the
    /// host is never handed a button that will silently fail.
    PathBoundary {
        /// The path the call actually named, canonicalized.
        target: String,
        /// Always `read`. Write access outside the workspace IS grantable
        /// (#1104), but only from a folder the OPERATOR chose in a picker —
        /// this escalation's `suggested_root` is derived from a path the MODEL
        /// named, and offering "always allow writes here" for that would be a
        /// prompt-injection lever rather than a convenience.
        access: crate::commands::PathGrantAccess,
        /// The CONTAINING DIRECTORY of `target`, which is what a grant opens.
        /// Putting `target` on an "always allow this folder" button would be a
        /// button that lies about its own scope.
        suggested_root: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Info,
    Edit,
    Exec,
    Mcp,
}

impl std::fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Edit => write!(f, "edit"),
            Self::Exec => write!(f, "exec"),
            Self::Mcp => write!(f, "mcp"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    Text,
    Diff,
    Image,
}

/// Token usage emitted with `stream_end`.
///
/// **Token accounting note (Task F, BD-audit Concern 2).** `output_tokens`
/// is reported verbatim from the underlying provider response (Anthropic
/// `usage.output_tokens`, OpenAI `usage.completion_tokens`, Bedrock
/// `usage.output_tokens` from the Anthropic-passthrough event stream).
///
/// Across the providers we ship, `output_tokens` reflects the **billable**
/// completion token count, which includes serialized tool-call arguments
/// and thinking tokens (where exposed). It does **not** equal the visible
/// text-delta byte count divided by ~4. App-side heuristics that compare
/// "characters streamed" against `output_tokens` will see large gaps on
/// tool-heavy turns; that is expected, not a bug. Prefer `finish_reason`
/// over content-length comparison for detecting truncation.
///
/// Empirical baseline landed in W12: `docs/tool-token-empirical-2026-05-15.md`.
/// Run `cargo run -p wcore-agent --bin tool_token_bench --features
/// test-utils` to regenerate the scripted-provider numbers; the same
/// doc's §2 runbook covers the live-API path that still needs real
/// provider credentials to fill in.
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// #279(a): active-window fill 0..=100, sourced by the engine from
    /// wcore_config::context_window::ContextWindow::percent() on the
    /// POST-swap effective model. Opaque u32; wcore-protocol takes NO
    /// wcore-config dep. None (omitted) when the window is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_window_percent: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_policy_event_is_typed_and_additive() {
        use crate::execution_policy::ExecutionPolicySequence;
        use wcore_types::execution_policy::{
            ApprovalPolicy, BaselineExecutionPolicy, EffectiveExecutionPolicy, PolicySource,
        };

        let snapshot = ExecutionPolicySequence::launch(
            EffectiveExecutionPolicy::baseline(&BaselineExecutionPolicy::smart(
                ApprovalPolicy::Bypass,
                PolicySource::LocalCliLaunch,
            )),
            1_700_000_000_000,
        )
        .current()
        .clone();
        let event = ProtocolEvent::ExecutionPolicy { snapshot };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["type"], "execution_policy");
        assert_eq!(json["critical"], true);
        assert_eq!(json["contract_version"], "1.0");
        assert_eq!(json["revision"], 0);
        assert_eq!(json["reason"], "launch");
        assert_eq!(json["effective_at_unix_ms"], 1_700_000_000_000_u64);
        assert_eq!(json["policy"]["posture"], "smart");
        assert_eq!(json["policy"]["approvals"], "bypass");
        assert_eq!(json["policy"]["sandbox"], "required");
        assert_eq!(json["policy"]["source"], "local_cli_launch");
    }
    use serde_json::json;

    #[test]
    fn test_ready_event_serialization() {
        let event = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: Some("abc123".to_string()),
            session_persistence: SessionPersistence::Durable,
            capabilities: Capabilities {
                tool_approval: true,
                thinking: true,
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                ..Default::default()
            },
            contract: None,
            execution_policy: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "ready");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["session_id"], "abc123");
        assert_eq!(json["session_persistence"], "durable");
        assert_eq!(json["capabilities"]["tool_approval"], true);

        // A `None` session id is STATED, not implied by an absent key. The
        // previous assertion here was `json2.get("session_id").is_none()` —
        // it pinned the defect rather than the contract: a host reading
        // `ready.session_id` got `undefined` and could not tell a degraded
        // Core from a malformed frame from a Core too old to know.
        let event_no_sid = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: None,
            session_persistence: SessionPersistence::DisabledByOperator,
            capabilities: Capabilities {
                tool_approval: true,
                thinking: true,
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                ..Default::default()
            },
            contract: None,
            execution_policy: None,
        };
        let json2 = serde_json::to_value(&event_no_sid).unwrap();
        assert_eq!(
            json2.get("session_id"),
            Some(&Value::Null),
            "session_id must be present and null, never absent: {json2}"
        );
        assert_eq!(json2["session_persistence"], "disabled_by_operator");
    }

    /// A consumer that reads ONLY the wire can tell every `ready` posture
    /// apart, and each one carries a `session_id` key.
    ///
    /// This is the property the omit-the-key shape could not provide, written
    /// the way a host actually reads a frame: no Rust types, just the JSON
    /// object. The frames must be pairwise distinguishable on
    /// `session_persistence` alone, and every one of them must publish the
    /// correlation key `EVENT_SPECS` says `ready` correlates on.
    ///
    /// FOUR postures now, and the fourth is the one that makes the pairwise
    /// requirement bite. `journaled_without_replay` and `durable` are the pair
    /// a consumer is most likely to collapse — both name a session, both are
    /// resumable, both survive a restart — and they differ on the single
    /// question a consumer asks this field to decide: may I wait for this
    /// session to recover itself?
    #[test]
    fn every_ready_posture_is_distinguishable_from_the_others_on_the_wire() {
        fn ready(session_id: Option<&str>, persistence: SessionPersistence) -> Value {
            serde_json::to_value(ProtocolEvent::Ready {
                version: "0.12.25".to_string(),
                session_id: session_id.map(str::to_string),
                session_persistence: persistence,
                capabilities: Capabilities::default(),
                contract: None,
                execution_policy: None,
            })
            .unwrap()
        }

        let durable = ready(Some("sess-durable"), SessionPersistence::Durable);
        let unsealed = ready(
            Some("sess-unsealed"),
            SessionPersistence::JournaledWithoutReplay,
        );
        let by_operator = ready(None, SessionPersistence::DisabledByOperator);
        let by_host = ready(None, SessionPersistence::DisabledByHost);

        // Every posture publishes the correlation key. A host can always ask
        // "is session_id present?" and get a truthful yes.
        for frame in [&durable, &unsealed, &by_operator, &by_host] {
            assert!(
                frame.as_object().unwrap().contains_key("session_id"),
                "ready dropped its declared correlation key: {frame}"
            );
        }

        // The two nulls are NOT the same event, the two named sessions are NOT
        // the same event, and no null is a string.
        let postures = [&durable, &unsealed, &by_operator, &by_host]
            .map(|frame| frame["session_persistence"].as_str().unwrap().to_string());
        assert_eq!(
            postures,
            [
                "durable",
                "journaled_without_replay",
                "disabled_by_operator",
                "disabled_by_host"
            ],
            "the wire vocabulary drifted from the type"
        );
        assert_eq!(
            postures
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4,
            "two postures collapsed to one wire value: {postures:?}"
        );

        // Naming a session no longer implies replay, and that is the whole
        // reason the fourth value exists. A consumer keying only on
        // "is session_id non-null?" cannot tell these two apart.
        assert_eq!(durable["session_id"], "sess-durable");
        assert_eq!(unsealed["session_id"], "sess-unsealed");
        assert_ne!(
            durable["session_persistence"], unsealed["session_persistence"],
            "a journaled session with no crash replay must not report as durable"
        );
        assert_eq!(by_operator["session_id"], Value::Null);
        assert_eq!(by_host["session_id"], Value::Null);
    }

    #[test]
    fn ready_can_expose_the_initial_effective_policy_snapshot() {
        use crate::execution_policy::ExecutionPolicySequence;
        use wcore_types::execution_policy::{
            ApprovalPolicy, BaselineExecutionPolicy, EffectiveExecutionPolicy, PolicySource,
        };

        let snapshot = ExecutionPolicySequence::launch(
            EffectiveExecutionPolicy::baseline(&BaselineExecutionPolicy::smart(
                ApprovalPolicy::Prompt,
                PolicySource::DesktopLocalLaunch,
            )),
            42,
        )
        .current()
        .clone();
        let json = serde_json::to_value(ProtocolEvent::Ready {
            version: "0.12.25".to_owned(),
            session_id: Some("session-1".to_owned()),
            session_persistence: SessionPersistence::Durable,
            capabilities: Capabilities::default(),
            contract: None,
            execution_policy: Some(snapshot),
        })
        .unwrap();

        assert_eq!(json["execution_policy"]["revision"], 0);
        assert_eq!(json["execution_policy"]["reason"], "launch");
        assert_eq!(json["execution_policy"]["critical"], true);
        assert_eq!(json["execution_policy"]["policy"]["approvals"], "prompt");
    }

    #[test]
    fn capability_activation_serializes_as_a_flat_additive_event() {
        let event = ProtocolEvent::CapabilityActivation {
            activation: CapabilityActivation::unavailable(
                CapabilityId::DelegateIsolation,
                CapabilityReasonCode::IsolationNotEnforced,
            ),
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "type": "capability_activation",
                "capability": "delegate_isolation",
                "stage": "unavailable",
                "reason": "isolation_not_enforced"
            })
        );
    }

    #[test]
    fn capability_activation_requires_reason_only_when_unavailable() {
        assert!(
            CapabilityActivation::unavailable(
                CapabilityId::PricingRefresher,
                CapabilityReasonCode::NoProductionConstructor,
            )
            .is_well_formed()
        );
        assert!(
            CapabilityActivation::stage(CapabilityId::SmartHandoff, CapabilityStage::Ready)
                .is_well_formed()
        );
        assert!(
            !CapabilityActivation {
                capability: CapabilityId::SmartHandoff,
                stage: CapabilityStage::Unavailable,
                reason: None,
            }
            .is_well_formed()
        );
        assert!(
            !CapabilityActivation {
                capability: CapabilityId::SmartHandoff,
                stage: CapabilityStage::Ready,
                reason: Some(CapabilityReasonCode::DependencyUnavailable),
            }
            .is_well_formed()
        );
    }

    #[test]
    fn capability_activation_transition_cycle_is_explicit() {
        use CapabilityStage as Stage;

        assert!(Stage::Declared.allows(Stage::Configured));
        assert!(Stage::Configured.allows(Stage::Constructed));
        assert!(Stage::Constructed.allows(Stage::Ready));
        assert!(Stage::Ready.allows(Stage::Reached));
        assert!(Stage::Reached.allows(Stage::OutcomeChanged));
        assert!(Stage::OutcomeChanged.allows(Stage::Observed));
        assert!(Stage::Observed.allows(Stage::Reached));
        assert!(Stage::Declared.allows(Stage::Unavailable));
        assert!(!Stage::Unavailable.allows(Stage::Configured));
        assert!(!Stage::Ready.allows(Stage::Observed));
    }

    #[test]
    fn test_text_delta_event_serialization() {
        let event = ProtocolEvent::TextDelta {
            text: "hello".to_string(),
            msg_id: "m1".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["msg_id"], "m1");
    }

    /// #318: a plain thinking-text event (no subject) must serialize WITHOUT
    /// the `subject` key — preserving the v0 wire shape for hosts that don't
    /// read it (skip_serializing_if = Option::is_none).
    #[test]
    fn test_thinking_event_without_subject_omits_key() {
        let event = ProtocolEvent::Thinking {
            text: "step 1".to_string(),
            msg_id: "m1".to_string(),
            subject: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["text"], "step 1");
        assert_eq!(json["msg_id"], "m1");
        assert!(
            json.get("subject").is_none(),
            "subject must be omitted when None, got {json}"
        );
    }

    /// #318: a subject-carrying thinking event serializes the field as exactly
    /// `subject` (the name the desktop matches on), with an empty `text`.
    #[test]
    fn test_thinking_event_with_subject_serializes_subject() {
        let event = ProtocolEvent::Thinking {
            text: String::new(),
            msg_id: "m1".to_string(),
            subject: Some("Reasoning through the problem".to_string()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "thinking");
        assert_eq!(json["text"], "");
        assert_eq!(json["msg_id"], "m1");
        assert_eq!(json["subject"], "Reasoning through the problem");
    }

    /// #537/#141: the host-send frame must serialize with EXACTLY the field
    /// names the desktop's `protocol.ts` union declares
    /// (`host_send_message_request` / call_id / platform / chat_id /
    /// thread_id / body / subject / conversation_id) — the desktop half
    /// (wayland PR #543) is already shipped against this spelling.
    #[test]
    fn test_host_send_message_request_full_serialization() {
        let event = ProtocolEvent::HostSendMessageRequest {
            call_id: "hsm-1".to_string(),
            platform: "email".to_string(),
            chat_id: Some("mike@example.com".to_string()),
            thread_id: Some("t-9".to_string()),
            body: "hello".to_string(),
            subject: Some("Re: invoice".to_string()),
            conversation_id: Some("abc123".to_string()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "host_send_message_request");
        assert_eq!(json["call_id"], "hsm-1");
        assert_eq!(json["platform"], "email");
        assert_eq!(json["chat_id"], "mike@example.com");
        assert_eq!(json["thread_id"], "t-9");
        assert_eq!(json["body"], "hello");
        assert_eq!(json["subject"], "Re: invoice");
        assert_eq!(json["conversation_id"], "abc123");
    }

    /// #1098: the render frame serializes with the field names a host
    /// implementer reads in the spec, and `critical` lands as the JSON literal
    /// `false` (not a string, not omitted) — that literal is the ONLY thing
    /// that makes an older host drop the event instead of disconnecting.
    #[test]
    fn render_artifact_serializes_with_an_explicit_noncritical_classification() {
        let event = ProtocolEvent::RenderArtifact {
            msg_id: "m-1".to_string(),
            call_id: "call-1".to_string(),
            title: "Q3 summary".to_string(),
            mime: RenderMime::Markdown,
            content: "# hello".to_string(),
            truncated: false,
            critical: NonCritical,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "render_artifact");
        assert_eq!(json["msg_id"], "m-1");
        assert_eq!(json["call_id"], "call-1");
        assert_eq!(json["title"], "Q3 summary");
        assert_eq!(json["mime"], "text/markdown");
        assert_eq!(json["content"], "# hello");
        assert_eq!(json["truncated"], false);
        assert_eq!(
            json["critical"],
            Value::Bool(false),
            "an unknown-to-the-host render frame is droppable ONLY while this is literal false"
        );
    }

    /// #1098: the closed MIME vocabulary round-trips exactly, and nothing
    /// outside it parses. A free-text mime would reach the wire and then be
    /// rejected by our OWN published schema's enum.
    #[test]
    fn render_mime_vocabulary_is_closed() {
        for token in RenderMime::all() {
            let parsed = RenderMime::from_wire(token).expect("declared token must parse");
            assert_eq!(parsed.as_str(), *token);
        }
        for rejected in [
            "application/x-shellscript",
            "text/html; charset=utf-8",
            "image/png",
            "TEXT/PLAIN",
            "",
        ] {
            assert!(
                RenderMime::from_wire(rejected).is_none(),
                "{rejected} must not parse into the closed vocabulary"
            );
        }
    }

    /// #1098: crossing the content cap TRUNCATES and says so — it never
    /// discards the artifact. Same rule as the wcore-sandbox buffered-output
    /// cap (FerroxLabs/wayland#1071).
    #[test]
    fn render_content_over_the_cap_is_truncated_not_dropped() {
        // A multi-byte character straddling the cap boundary: the cut must
        // land on a char boundary, never mid-sequence.
        let mut oversized = "a".repeat(RENDER_ARTIFACT_CONTENT_LIMIT_BYTES - 1);
        oversized.push('\u{20ac}'); // 3 bytes, starts one byte below the cap
        oversized.push_str(&"b".repeat(64));
        assert!(oversized.len() > RENDER_ARTIFACT_CONTENT_LIMIT_BYTES);

        let (rendered, truncated) = truncate_render_content(&oversized);
        assert!(truncated, "crossing the cap must report truncation");
        let marker_at = rendered
            .find("[wcore: CONTENT TRUNCATED")
            .expect("the in-band marker must be present so a human reading the surface is told");
        // The marker opens with the blank line that separates it from the
        // content, so the content prefix is everything before those newlines.
        let kept = rendered[..marker_at].trim_end_matches('\n');
        assert!(
            kept.len() <= RENDER_ARTIFACT_CONTENT_LIMIT_BYTES,
            "kept prefix {} exceeds the cap",
            kept.len()
        );
        assert_eq!(
            kept.len(),
            RENDER_ARTIFACT_CONTENT_LIMIT_BYTES - 1,
            "the straddling multi-byte character must be dropped whole"
        );
        assert!(
            kept.chars().all(|c| c == 'a'),
            "the kept bytes must be the START of the content"
        );
        assert!(
            rendered.contains(&RENDER_ARTIFACT_CONTENT_LIMIT_BYTES.to_string()),
            "the marker must name the cap it hit"
        );
    }

    /// #1098: content at exactly the cap is untouched, so nothing is reported
    /// truncated that was not.
    #[test]
    fn render_content_at_the_cap_is_untouched() {
        let exact = "x".repeat(RENDER_ARTIFACT_CONTENT_LIMIT_BYTES);
        let (rendered, truncated) = truncate_render_content(&exact);
        assert!(!truncated);
        assert_eq!(rendered, exact);
    }

    /// #1098: the reason the cap is 1 MiB and not a bigger round number.
    ///
    /// A worst-case escaped render frame — every content byte a control
    /// character, which serde_json emits as the 6-byte `\u00XX` — must still
    /// fit inside the output pump's per-frame limit. Over that limit
    /// `output_pump` does not merely drop the frame: it sets `sticky_failure`
    /// and calls `record_failure()`, and the `failed` flag is sticky, so every
    /// later write returns BrokenPipe. One oversized artifact would kill the
    /// session's entire stdout.
    #[test]
    fn a_worst_case_escaped_render_frame_fits_the_output_pump() {
        let event = ProtocolEvent::RenderArtifact {
            msg_id: "m".repeat(64),
            call_id: "c".repeat(64),
            title: truncate_render_title(&"t".repeat(RENDER_ARTIFACT_TITLE_LIMIT_BYTES * 2)),
            mime: RenderMime::Html,
            // `\u{0}` is serde_json's worst case: 1 byte in, 6 bytes out.
            content: truncate_render_content(
                &"\u{0}".repeat(RENDER_ARTIFACT_CONTENT_LIMIT_BYTES * 2),
            )
            .0,
            truncated: true,
            critical: NonCritical,
        };
        let encoded = serde_json::to_vec(&event).unwrap();
        assert!(
            encoded.len() + 1 < crate::output_pump::MAX_QUEUED_BYTES,
            "worst-case render frame is {} bytes, pump limit is {}",
            encoded.len(),
            crate::output_pump::MAX_QUEUED_BYTES
        );
    }

    /// #537/#141: optional fields are OMITTED (not null) when absent — the
    /// desktop types them as optional (`chat_id?` / `thread_id?` /
    /// `subject?` / `conversation_id?`).
    #[test]
    fn test_host_send_message_request_omits_absent_optionals() {
        let event = ProtocolEvent::HostSendMessageRequest {
            call_id: "hsm-2".to_string(),
            platform: "telegram".to_string(),
            chat_id: None,
            thread_id: None,
            body: "ping".to_string(),
            subject: None,
            conversation_id: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "host_send_message_request");
        assert_eq!(json["platform"], "telegram");
        assert_eq!(json["body"], "ping");
        for key in ["chat_id", "thread_id", "subject", "conversation_id"] {
            assert!(
                json.get(key).is_none(),
                "{key} must be omitted when None, got {json}"
            );
        }
    }

    #[test]
    fn test_tool_request_event_serialization() {
        let event = ProtocolEvent::ToolRequest {
            msg_id: "m1".to_string(),
            call_id: "c1".to_string(),
            tool: ToolInfo {
                name: "Bash".to_string(),
                category: ToolCategory::Exec,
                args: json!({"command": "ls"}),
                description: "Execute: ls".to_string(),
                escalation: None,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_request");
        assert_eq!(json["tool"]["category"], "exec");
    }

    #[test]
    fn test_tool_result_event_serialization() {
        let event = ProtocolEvent::ToolResult {
            msg_id: "m1".to_string(),
            call_id: "c1".to_string(),
            tool_name: "Read".to_string(),
            status: ToolStatus::Success,
            output: "file content".to_string(),
            output_type: OutputType::Text,
            metadata: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["status"], "success");
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn test_error_event_serialization() {
        let event = ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "rate_limit".to_string(),
                message: "Too many requests".to_string(),
                retryable: true,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "error");
        assert!(json.get("msg_id").is_none());
        assert_eq!(json["error"]["retryable"], true);
    }

    #[test]
    fn test_stream_end_with_usage() {
        let event = ProtocolEvent::StreamEnd {
            msg_id: "m1".to_string(),
            finish_reason: FinishReason::Stop,
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: Some(20),
                cache_write_tokens: None,
                active_window_percent: None,
            }),
            usage_delta: None,
            agent_run_id: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "stream_end");
        assert_eq!(json["finish_reason"], "stop");
        assert_eq!(json["usage"]["input_tokens"], 100);
        assert!(json["usage"].get("cache_write_tokens").is_none());
        // CORE-2 back-compat: a None delta must not appear on the wire.
        assert!(json.get("usage_delta").is_none());
    }

    #[test]
    fn test_stream_end_usage_delta_is_sibling_with_same_field_names() {
        // CORE-2: the per-run delta rides as a SIBLING of the cumulative
        // usage, with the same inner field names.
        let event = ProtocolEvent::StreamEnd {
            msg_id: "m1".to_string(),
            finish_reason: FinishReason::Stop,
            usage: Some(Usage {
                input_tokens: 300,
                output_tokens: 30,
                cache_read_tokens: Some(20),
                cache_write_tokens: None,
                active_window_percent: Some(42),
            }),
            usage_delta: Some(Usage {
                input_tokens: 200,
                output_tokens: 20,
                cache_read_tokens: Some(15),
                cache_write_tokens: Some(5),
                active_window_percent: None,
            }),
            agent_run_id: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["usage"]["input_tokens"], 300);
        assert_eq!(json["usage_delta"]["input_tokens"], 200);
        assert_eq!(json["usage_delta"]["output_tokens"], 20);
        assert_eq!(json["usage_delta"]["cache_read_tokens"], 15);
        assert_eq!(json["usage_delta"]["cache_write_tokens"], 5);
        // The window gauge is a session-level reading; it stays on `usage`.
        assert!(json["usage_delta"].get("active_window_percent").is_none());
    }

    #[test]
    fn test_stream_end_finish_reason_serialization() {
        // Required field: each variant should serialize to its snake_case name.
        for (variant, expected) in [
            (FinishReason::Stop, "stop"),
            (FinishReason::Length, "length"),
            (FinishReason::Error, "error"),
            // #457: the engine turn-cap is a distinct, host-visible value.
            (FinishReason::MaxTurns, "max_turns"),
        ] {
            let event = ProtocolEvent::StreamEnd {
                msg_id: "m1".to_string(),
                finish_reason: variant,
                usage: None,
                usage_delta: None,
                agent_run_id: None,
            };
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["finish_reason"], expected, "variant {variant:?}");
        }
    }

    #[test]
    fn test_stream_end_finish_reason_required_in_output() {
        // Verify the field is always present in JSON, even when usage is None.
        let event = ProtocolEvent::StreamEnd {
            msg_id: "m1".to_string(),
            finish_reason: FinishReason::Length,
            usage: None,
            usage_delta: None,
            agent_run_id: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert!(
            json.get("finish_reason").is_some(),
            "finish_reason must be present on every stream_end event"
        );
    }

    #[test]
    fn test_tool_category_display() {
        assert_eq!(ToolCategory::Info.to_string(), "info");
        assert_eq!(ToolCategory::Edit.to_string(), "edit");
        assert_eq!(ToolCategory::Exec.to_string(), "exec");
        assert_eq!(ToolCategory::Mcp.to_string(), "mcp");
    }

    #[test]
    fn test_ready_event_with_expanded_capabilities() {
        let event = ProtocolEvent::Ready {
            version: "0.2.0".to_string(),
            session_id: Some("abc".to_string()),
            session_persistence: SessionPersistence::Durable,
            capabilities: Capabilities {
                tool_approval: true,
                thinking: true,
                effort: true,
                effort_levels: vec!["low".into(), "medium".into(), "high".into()],
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                ..Default::default()
            },
            contract: None,
            execution_policy: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["capabilities"]["thinking"], true);
        assert_eq!(json["capabilities"]["effort"], true);
        assert_eq!(json["capabilities"]["effort_levels"][0], "low");
        assert_eq!(json["capabilities"]["modes"][2], "force");
    }

    #[test]
    fn test_mcp_ready_event_serialization() {
        let event = ProtocolEvent::McpReady {
            name: "team-tools".to_string(),
            tools: vec!["team_send_message".into(), "team_task_create".into()],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "mcp_ready");
        assert_eq!(json["name"], "team-tools");
        assert_eq!(json["tools"][0], "team_send_message");
        assert_eq!(json["tools"][1], "team_task_create");
        assert_eq!(json["tools"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_pong_event_serialization() {
        let event = ProtocolEvent::Pong;
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "pong");
        assert_eq!(json.as_object().unwrap().len(), 1);
    }

    #[test]
    fn midflight_monitor_decision_serializes_typed_control_flow() {
        let event = ProtocolEvent::MidFlightMonitorDecision {
            directive: MonitorDirective::Replan,
            reason: MonitorReason::RepeatedToolRoute,
        };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["type"], "mid_flight_monitor_decision");
        assert_eq!(json["directive"], "replan");
        assert_eq!(json["reason"], "repeated_tool_route");

        let stop = serde_json::to_value(ProtocolEvent::MidFlightMonitorDecision {
            directive: MonitorDirective::Stop,
            reason: MonitorReason::OutputStall,
        })
        .unwrap();
        assert_eq!(stop["directive"], "stop");
        assert_eq!(stop["reason"], "output_stall");
    }

    #[test]
    fn test_config_changed_event_serialization() {
        let event = ProtocolEvent::ConfigChanged {
            capabilities: Capabilities {
                tool_approval: true,
                thinking: false,
                effort: true,
                effort_levels: vec!["low".into(), "medium".into(), "high".into()],
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                current_mode: "default".into(),
                mcp: true,
                ..Default::default()
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "config_changed");
        assert_eq!(json["capabilities"]["thinking"], false);
        assert_eq!(json["capabilities"]["effort"], true);
    }

    #[test]
    fn capabilities_default_has_all_w0_flags_false_and_v0_1_21_baseline() {
        let caps = Capabilities::default();

        // v0.1.21 baseline fields
        assert!(!caps.tool_approval);
        assert!(!caps.thinking);
        assert!(!caps.effort);
        assert!(caps.effort_levels.is_empty());
        assert_eq!(caps.modes, vec!["default".to_string()]);
        assert_eq!(caps.current_mode, "default");
        assert!(!caps.mcp);

        // W0 — new opt-in flags, all default false
        assert!(!caps.streaming_tools);
        assert!(!caps.sub_agent_traces);
        assert!(!caps.cost_attribution);
        assert!(!caps.hitl_suspend);
        assert!(!caps.non_destructive_compact);
        assert!(!caps.structured_traces);
        assert!(!caps.rpc_tool_script);
        assert!(!caps.browser_suite);
        assert!(!caps.computer_use);
        assert!(!caps.plugins);
        assert!(!caps.gepa_enabled);
    }

    #[test]
    fn capabilities_default_off_serializes_without_new_flag_keys() {
        // Critical: with all W0 flags off, the JSON output must be byte-identical
        // to the v0.1.21 shape so hosts that don't know about the new flags see
        // no change in the wire format.
        let event = ProtocolEvent::Ready {
            version: "0.1.21".into(),
            session_id: None,
            session_persistence: SessionPersistence::DisabledByOperator,
            capabilities: Capabilities::default(),
            contract: None,
            execution_policy: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        let caps_obj = &json["capabilities"];

        // v0.1.21 keys present
        for k in [
            "tool_approval",
            "thinking",
            "effort",
            "effort_levels",
            "modes",
            "current_mode",
            "mcp",
        ] {
            assert!(caps_obj.get(k).is_some(), "v0.1.21 key {k} missing");
        }

        // W0 flags ABSENT when default-off (skip_serializing_if invariant)
        for k in [
            "streaming_tools",
            "sub_agent_traces",
            "cost_attribution",
            "hitl_suspend",
            "non_destructive_compact",
            "structured_traces",
            "rpc_tool_script",
            "browser_suite",
            "computer_use",
            "plugins",
            "gepa_enabled",
        ] {
            assert!(
                caps_obj.get(k).is_none(),
                "W0 flag {k} leaked into JSON when default-off"
            );
        }
    }

    #[test]
    fn capabilities_round_trips_through_deserialize() {
        // W0 audit Finding 3: Capabilities now derives Deserialize so future
        // host-side parsing or test fixtures can read it back. Each W0 flag
        // is annotated `#[serde(default)]` so default-off serializations
        // (which omit the key entirely via skip_serializing_if) deserialize
        // back to `false` cleanly.
        let original = Capabilities {
            tool_approval: true,
            thinking: true,
            effort: true,
            effort_levels: vec!["low".into(), "high".into()],
            modes: vec!["default".into(), "force".into()],
            current_mode: "force".into(),
            mcp: true,
            browser_suite: true,
            ..Default::default()
        };
        let serialized = serde_json::to_string(&original).unwrap();
        let parsed: Capabilities = serde_json::from_str(&serialized).unwrap();
        // Spot-check round-trip preserves both v0.1.21 and W0 fields.
        assert!(parsed.tool_approval);
        assert_eq!(
            parsed.modes,
            vec!["default".to_string(), "force".to_string()]
        );
        assert!(parsed.browser_suite);
        // Default-off W0 flags round-trip as false.
        assert!(!parsed.plugins);
        assert!(!parsed.computer_use);
    }

    #[test]
    fn capabilities_default_off_serialization_deserializes_with_w0_flags_false() {
        // The skip_serializing_if invariant means a default-off Capabilities
        // serializes WITHOUT any W0 flag keys. Deserializing that JSON back
        // must yield default-off (all W0 flags = false) thanks to serde(default).
        let serialized = serde_json::to_string(&Capabilities::default()).unwrap();
        let parsed: Capabilities = serde_json::from_str(&serialized).unwrap();
        assert!(!parsed.streaming_tools);
        assert!(!parsed.sub_agent_traces);
        assert!(!parsed.cost_attribution);
        assert!(!parsed.hitl_suspend);
        assert!(!parsed.non_destructive_compact);
        assert!(!parsed.structured_traces);
        assert!(!parsed.rpc_tool_script);
        assert!(!parsed.browser_suite);
        assert!(!parsed.computer_use);
        assert!(!parsed.plugins);
        assert!(!parsed.gepa_enabled);
        assert!(!parsed.memory_enabled);
    }

    #[test]
    fn capabilities_memory_enabled_emits_when_on_and_omits_when_off() {
        // Rank 85: memory_enabled gives the host an explicit bool to key on,
        // disambiguating "memory disabled" from "field unknown". On → present
        // and true; off → omitted (skip_serializing_if) but decodes to false.
        let on = Capabilities {
            memory_enabled: true,
            ..Default::default()
        };
        let on_json = serde_json::to_string(&on).unwrap();
        assert!(
            on_json.contains("\"memory_enabled\":true"),
            "expected memory_enabled key when on: {on_json}"
        );
        assert!(
            serde_json::from_str::<Capabilities>(&on_json)
                .unwrap()
                .memory_enabled
        );

        let off_json = serde_json::to_string(&Capabilities::default()).unwrap();
        assert!(
            !off_json.contains("memory_enabled"),
            "memory_enabled must be omitted when off: {off_json}"
        );
        assert!(
            !serde_json::from_str::<Capabilities>(&off_json)
                .unwrap()
                .memory_enabled
        );
    }

    #[test]
    fn capabilities_flag_on_serializes_with_key_present() {
        let caps = Capabilities {
            browser_suite: true,
            ..Default::default()
        };
        let event = ProtocolEvent::Ready {
            version: "0.2.0".into(),
            session_id: None,
            session_persistence: SessionPersistence::DisabledByOperator,
            capabilities: caps,
            contract: None,
            execution_policy: None,
        };
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["capabilities"]["browser_suite"], true);
        assert!(json["capabilities"].get("computer_use").is_none());
    }

    #[test]
    fn workspace_policy_receipt_serializes_as_output_only_effective_authority() {
        use wcore_types::workspace_trust::{
            WorkspacePolicyReceipt, WorkspaceSandboxProfile, resolve_workspace_trust,
        };

        let event = ProtocolEvent::WorkspacePolicy {
            policy: WorkspacePolicyReceipt {
                trust: resolve_workspace_trust("fingerprint", []),
                profile: WorkspaceSandboxProfile::Strict,
                backend: "bwrap".to_string(),
                writable_roots: vec!["/workspace".to_string()],
                readable_roots: vec!["/workspace".to_string()],
                capabilities: Vec::new(),
            },
        };
        let json = serde_json::to_value(event).unwrap();

        assert_eq!(json["type"], "workspace_policy");
        assert_eq!(json["policy"]["profile"], "strict");
        assert_eq!(json["policy"]["trust"]["level"], "untrusted");
        assert_eq!(json["policy"]["trust"]["fingerprint"], "fingerprint");
        assert_eq!(json["policy"]["backend"], "bwrap");
    }

    #[test]
    fn provider_failover_receipt_preserves_opaque_replay_shape() {
        let receipt = serde_json::json!({
            "reason": "rate_limit",
            "failed_provider": "anthropic",
            "failed_model": "claude-sonnet-4-6",
            "candidates": [{
                "provider": "openai",
                "model": "gpt-5",
                "disposition": { "Ok": null },
                "pricing": {
                    "source": "bundled",
                    "stale": false,
                    "priced": true,
                    "estimated_microcents": 4242
                }
            }],
            "selected_provider": "openai",
            "selected_model": "gpt-5"
        });
        let event = ProtocolEvent::ProviderFailoverReceipt {
            receipt: receipt.clone(),
        };

        let wire = serde_json::to_string(&event).unwrap();
        let replayed: serde_json::Value = serde_json::from_str(&wire).unwrap();

        assert_eq!(replayed["type"], "provider_failover_receipt");
        assert_eq!(replayed["receipt"], receipt);
    }

    #[test]
    fn legacy_turn_cost_defaults_to_unpriced() {
        let row: TurnCost = serde_json::from_value(serde_json::json!({
            "turn": 1,
            "model": "router-model",
            "provider": "router",
            "cost_usd": 0.0
        }))
        .unwrap();
        assert!(!row.priced);
    }
}
