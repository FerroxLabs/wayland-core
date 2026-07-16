//! Fail-closed planning for interrupted durable turns.
//!
//! Core continues only from a proven boundary: either `TurnStarted` is the
//! journal head with no later execution state, or the latest recovery
//! checkpoint is versioned, conversation-bound, and followed only by receipts
//! for its exact provider dispatch. Every other state remains an explicit
//! blocker rather than silently becoming a second provider or tool request.

use crate::session_journal::{
    DeliveryOrigin, ExternalEffectState, JournalError, SessionEvent, SessionJournal,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use wcore_protocol::events::{
    RecoveryBudgetSnapshot, RecoveryCursor, RecoveryLifecycle, RecoveryReconcileReason,
    RecoveryReplayItem, RecoveryReplayKind, RecoveryTurnSnapshot, RecoveryUnavailableReason,
};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, TokenUsage};

#[derive(Debug, Clone)]
pub enum RecoveryDisposition {
    Ready,
    ContinueTurnStart {
        turn_id: String,
        user_message: String,
    },
    ContinueCheckpoint {
        turn_id: String,
        user_message: String,
        checkpoint_id: String,
        checkpoint: Box<RecoveryCheckpoint>,
    },
    AwaitApproval {
        turn_id: String,
        approval_ids: Vec<String>,
    },
    ReconciliationRequired {
        turn_id: String,
        tool_execution_ids: Vec<String>,
    },
    Blocked {
        turn_id: String,
        reason: RecoveryBlocker,
    },
}

pub(crate) const RECOVERY_CHECKPOINT_VERSION: u64 = 4;
pub(crate) const TOOL_HOOK_RECOVERY_AUTHORITY_VERSION: u64 = 1;

/// Exact operation that is safe after the durable checkpoint at the journal
/// head. A checkpoint never means merely "somewhere in the turn loop".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryNextAction {
    /// Replay the exact prepared provider request after proving that its
    /// message prefix is derived from the durable conversation.
    ProviderDispatch,
    /// Continue at the top of the next provider loop after the preceding
    /// tool-result message and every external effect are durable. Request
    /// assembly intentionally runs again because no provider request has yet
    /// been prepared for this loop iteration.
    ContinueLoop,
    /// Resume the exact tool-call round already committed in the assistant
    /// conversation. Only approval and tool-effect receipts correlated to the
    /// committed call ids may follow this checkpoint.
    ContinueToolRound,
    /// Commit the already-durable assistant response without calling the
    /// provider or executing another tool.
    CommitTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryLoopGuardState {
    pub last_signature: Option<u64>,
    pub count: u32,
    pub threshold: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryFailureGuardState {
    pub count: u32,
    pub threshold: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryPosture {
    pub plan_active: bool,
    pub pre_plan_allow_list: Vec<String>,
    pub effective_allow_list: Vec<String>,
    /// Tool breakers with any live failure history are reopened
    /// conservatively after process restart. This may narrow availability for
    /// one cooldown, but can never erase an original same-turn denial.
    pub conservatively_open_breakers: Vec<String>,
    /// Digest of independently reconstructed policy, workspace, cwd and tool
    /// inventory data. It is compared before any continuation authority is
    /// restored.
    pub authority_digest: String,
    /// Per-component digests make a fail-closed restart refusal actionable
    /// without persisting or exposing the underlying policy or prompt values.
    #[serde(default)]
    pub authority_component_digests: BTreeMap<String, String>,
    /// Versioned semantics for the executable tool-hook inventory embedded in
    /// `authority_component_digests`. Recovery must reconstruct and match this
    /// authority before continuing from a pre-execution tool-round checkpoint.
    pub tool_hook_authority_version: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryTerminalCompletion {
    #[default]
    Committed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryTerminalResult {
    #[serde(default)]
    pub completion: RecoveryTerminalCompletion,
    pub text: String,
    pub stop_reason: String,
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
    pub usage_delta: TokenUsage,
    pub turns: u64,
    pub active_window_percent: Option<u32>,
    pub agent_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCheckpoint {
    pub(crate) version: u64,
    /// Stable provider-routing identity used by the exact request. Resumed
    /// engines must restore this before rebuilding the request digest.
    pub(crate) conversation_id: String,
    pub(crate) next_action: RecoveryNextAction,
    pub(crate) conversation_digest: String,
    pub(crate) message_count: u64,
    pub(crate) turn_index: u64,
    pub(crate) stream_attempt: u32,
    pub(crate) overflow_retried: bool,
    pub(crate) length_wedge_retried: bool,
    pub(crate) request_digest: Option<String>,
    /// Logical provider dispatch authorized by this checkpoint. Every
    /// physical fallback/retry attempt for the request must carry this exact
    /// identity. Terminal checkpoints never carry a dispatch identity.
    pub(crate) dispatch_id: Option<String>,
    /// Exact protected provider request admitted at this boundary. Recovery
    /// replays this canonical snapshot instead of rebuilding transient hook,
    /// skill, or date contributions after a restart.
    pub(crate) sealed_prepared_request: Option<crate::recovery_confidential::SealedPreparedRequest>,
    pub(crate) posture: RecoveryPosture,
    pub(crate) loop_guard: RecoveryLoopGuardState,
    pub(crate) failure_guard: RecoveryFailureGuardState,
    pub(crate) run_usage: TokenUsage,
    pub(crate) terminal_result: Option<RecoveryTerminalResult>,
}

impl RecoveryCheckpoint {
    pub(crate) fn from_value(value: &serde_json::Value) -> Result<Self, JournalError> {
        let checkpoint: Self =
            serde_json::from_value(value.clone()).map_err(|source| JournalError::Json {
                context: "decoding recovery checkpoint",
                source,
            })?;
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub(crate) fn to_value(&self) -> Result<serde_json::Value, JournalError> {
        self.validate()?;
        serde_json::to_value(self).map_err(|source| JournalError::Json {
            context: "encoding recovery checkpoint",
            source,
        })
    }

    /// Prove that the exact prepared request was built from this durable
    /// conversation. Request assembly may add only transient text blocks to
    /// the final user message and cache hints to existing messages; it may not
    /// alter, remove, reorder, or invent durable conversation content.
    pub(crate) fn validate_opened_prepared_request_conversation(
        &self,
        prepared_request: &serde_json::Value,
        conversation: &[serde_json::Value],
    ) -> Result<wcore_types::llm::LlmRequest, JournalError> {
        if !matches!(self.next_action, RecoveryNextAction::ProviderDispatch) {
            return Err(JournalError::InvalidTransition(
                "only provider-dispatch checkpoints carry a prepared request".to_string(),
            ));
        }
        let request =
            crate::session_journal::decode_prepared_provider_request_snapshot(prepared_request)?;
        let request_digest = crate::session_journal::provider_request_digest(&request)?;
        if self.request_digest.as_deref() != Some(request_digest.as_str())
            || request.conversation_id.as_deref() != Some(self.conversation_id.as_str())
        {
            return Err(JournalError::InvalidTransition(
                "provider-dispatch checkpoint request authority does not match its digest or conversation identity"
                    .to_string(),
            ));
        }
        let durable_messages = conversation
            .iter()
            .cloned()
            .map(|value| {
                serde_json::from_value::<Message>(value).map_err(|source| JournalError::Json {
                    context: "decoding durable conversation for recovery binding",
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if request.messages.len() != durable_messages.len() {
            return Err(JournalError::InvalidTransition(
                "prepared request message count does not match the durable conversation"
                    .to_string(),
            ));
        }
        for (index, (prepared, durable)) in
            request.messages.iter().zip(&durable_messages).enumerate()
        {
            if prepared.role != durable.role || prepared.timestamp != durable.timestamp {
                return Err(JournalError::InvalidTransition(format!(
                    "prepared request message {index} does not match durable identity"
                )));
            }

            let allows_transient_tail =
                index + 1 == durable_messages.len() && matches!(durable.role, Role::User);
            if prepared.content.len() < durable.content.len()
                || (!allows_transient_tail && prepared.content.len() != durable.content.len())
            {
                return Err(JournalError::InvalidTransition(format!(
                    "prepared request message {index} does not preserve durable content"
                )));
            }
            for (prepared_block, durable_block) in prepared.content.iter().zip(&durable.content) {
                let prepared_value =
                    serde_json::to_value(prepared_block).map_err(|source| JournalError::Json {
                        context: "encoding prepared request block for recovery binding",
                        source,
                    })?;
                let durable_value =
                    serde_json::to_value(durable_block).map_err(|source| JournalError::Json {
                        context: "encoding durable conversation block for recovery binding",
                        source,
                    })?;
                if prepared_value != durable_value {
                    return Err(JournalError::InvalidTransition(format!(
                        "prepared request message {index} changes durable content"
                    )));
                }
            }
            if prepared.content[durable.content.len()..]
                .iter()
                .any(|block| !matches!(block, ContentBlock::Text { .. }))
            {
                return Err(JournalError::InvalidTransition(format!(
                    "prepared request message {index} carries a non-text transient tail"
                )));
            }
        }

        Ok(request)
    }

    fn validate(&self) -> Result<(), JournalError> {
        if self.version != RECOVERY_CHECKPOINT_VERSION {
            return Err(JournalError::InvalidTransition(format!(
                "unsupported recovery checkpoint version {}",
                self.version
            )));
        }
        if self.conversation_id.is_empty() || self.conversation_id.len() > 256 {
            return Err(JournalError::InvalidTransition(
                "recovery checkpoint carries an invalid conversation identity".to_string(),
            ));
        }
        if !valid_digest(&self.conversation_digest) || !valid_digest(&self.posture.authority_digest)
        {
            return Err(JournalError::InvalidTransition(
                "recovery checkpoint carries a malformed authority digest".to_string(),
            ));
        }
        if self.posture.tool_hook_authority_version != TOOL_HOOK_RECOVERY_AUTHORITY_VERSION {
            return Err(JournalError::InvalidTransition(
                "recovery checkpoint carries an unsupported tool-hook authority version"
                    .to_string(),
            ));
        }
        if self
            .posture
            .conservatively_open_breakers
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
            || self
                .posture
                .conservatively_open_breakers
                .iter()
                .any(String::is_empty)
        {
            return Err(JournalError::InvalidTransition(
                "recovery checkpoint breaker authority is not canonical".to_string(),
            ));
        }
        if self.loop_guard.threshold != 0 && self.loop_guard.count >= self.loop_guard.threshold {
            return Err(JournalError::InvalidTransition(
                "recovery checkpoint loop guard is already terminal".to_string(),
            ));
        }
        if self.failure_guard.threshold != 0
            && self.failure_guard.count >= self.failure_guard.threshold
        {
            return Err(JournalError::InvalidTransition(
                "recovery checkpoint failure guard is already terminal".to_string(),
            ));
        }
        match self.next_action {
            RecoveryNextAction::ProviderDispatch => {
                if !self.request_digest.as_deref().is_some_and(valid_digest)
                    || !self.dispatch_id.as_deref().is_some_and(|dispatch_id| {
                        !dispatch_id.is_empty() && dispatch_id.len() <= 256
                    })
                    || self.sealed_prepared_request.is_none()
                    || self.terminal_result.is_some()
                {
                    return Err(JournalError::InvalidTransition(
                        "provider-dispatch checkpoint has inconsistent continuation data"
                            .to_string(),
                    ));
                }
                let sealed = self.sealed_prepared_request.as_ref().ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "provider-dispatch checkpoint is missing its prepared request".to_string(),
                    )
                })?;
                sealed.validate().map_err(|_| {
                    JournalError::InvalidTransition(
                        "provider-dispatch checkpoint carries an invalid sealed request"
                            .to_string(),
                    )
                })?;
            }
            RecoveryNextAction::ContinueLoop | RecoveryNextAction::ContinueToolRound => {
                if self.stream_attempt != 0
                    || self.overflow_retried
                    || self.length_wedge_retried
                    || self.request_digest.is_some()
                    || self.dispatch_id.is_some()
                    || self.sealed_prepared_request.is_some()
                    || self.terminal_result.is_some()
                {
                    return Err(JournalError::InvalidTransition(
                        "non-provider recovery checkpoint has inconsistent continuation data"
                            .to_string(),
                    ));
                }
            }
            RecoveryNextAction::CommitTurn => {
                let terminal = self.terminal_result.as_ref().ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "commit-turn checkpoint is missing its terminal result".to_string(),
                    )
                })?;
                if self.request_digest.is_some()
                    || self.dispatch_id.is_some()
                    || self.sealed_prepared_request.is_some()
                    || !matches!(
                        terminal.stop_reason.as_str(),
                        "end_turn" | "max_tokens" | "max_turns"
                    )
                    || (matches!(terminal.completion, RecoveryTerminalCompletion::Cancelled)
                        && (terminal.stop_reason != "end_turn" || !terminal.text.is_empty()))
                {
                    return Err(JournalError::InvalidTransition(
                        "commit-turn checkpoint has inconsistent terminal data".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryBlocker {
    ProviderOutcomeUnknown,
    HookOutcomeUnknown,
    ContextCheckpointMissing,
    ChildOutcomeUnknown,
    DeliveryOutcomeUnknown,
}

#[derive(Debug, Clone)]
pub struct RecoveryPlan {
    pub session_id: String,
    pub journal_sequence: Option<u64>,
    pub journal_digest: String,
    pub state_digest: String,
    pub budget: RecoveryBudgetSnapshot,
    pub disposition: RecoveryDisposition,
}

fn recovery_budget_snapshot(
    state: &crate::session_journal::ReducedSessionState,
) -> Result<RecoveryBudgetSnapshot, JournalError> {
    let Some(authority) = state.budget_authority.as_ref() else {
        return Ok(RecoveryBudgetSnapshot {
            tokens_used: 0,
            token_limit: None,
            cost_used_usd: 0.0,
            cost_limit_usd: None,
        });
    };
    let tracker = wcore_budget::BudgetTracker::from_snapshot(authority.provider_tracker.clone())
        .map_err(|error| {
            JournalError::InvalidTransition(format!(
                "recovery budget authority is invalid: {error}"
            ))
        })?;
    let (tokens_used, cost_used_usd) = tracker.session_totals(&authority.budget_session_id);
    let (token_limit, cost_limit_usd) =
        tracker.effective_session_limits(&authority.budget_session_id);
    Ok(RecoveryBudgetSnapshot {
        tokens_used,
        token_limit,
        cost_used_usd,
        cost_limit_usd,
    })
}

struct ValidatedRecoveryCheckpoint {
    checkpoint_id: String,
    checkpoint: RecoveryCheckpoint,
}

pub(crate) struct RecoveredToolRoundAuthority {
    pub(crate) conversation: Vec<serde_json::Value>,
    pub(crate) checkpoint: RecoveryCheckpoint,
}

struct ValidatedProviderSuffix {
    correlated_attempt_ids: BTreeSet<String>,
}

#[derive(Debug)]
struct RecoveryToolCallAuthority {
    ordinal: u64,
    tool: String,
    requested_input_digest: String,
}

fn recovery_tool_call_authority(
    conversation: &[serde_json::Value],
) -> Option<BTreeMap<String, RecoveryToolCallAuthority>> {
    let assistant = conversation.iter().rev().find_map(|message| {
        let message = serde_json::from_value::<Message>(message.clone()).ok()?;
        matches!(message.role, Role::Assistant).then_some(message)
    })?;

    let mut calls = BTreeMap::new();
    for block in &assistant.content {
        let ContentBlock::ToolUse {
            id, name, input, ..
        } = block
        else {
            continue;
        };
        let ordinal = u64::try_from(calls.len()).ok()?;
        let requested_input_digest = crate::session_journal::state_payload_digest(input).ok()?;
        if id.is_empty()
            || name.is_empty()
            || calls
                .insert(
                    id.clone(),
                    RecoveryToolCallAuthority {
                        ordinal,
                        tool: name.clone(),
                        requested_input_digest,
                    },
                )
                .is_some()
        {
            return None;
        }
    }
    (!calls.is_empty()).then_some(calls)
}

fn recovery_terminal_authority(
    conversation: &[serde_json::Value],
    terminal: Option<&RecoveryTerminalResult>,
) -> bool {
    let Some(terminal) = terminal else {
        return false;
    };
    if matches!(terminal.completion, RecoveryTerminalCompletion::Cancelled) {
        if !terminal.text.is_empty() || terminal.stop_reason != "end_turn" {
            return false;
        }
        let Some(calls) = recovery_tool_call_authority(conversation) else {
            return false;
        };
        let Some(results) = conversation.last().and_then(|message| {
            let message = serde_json::from_value::<Message>(message.clone()).ok()?;
            if !matches!(message.role, Role::User) {
                return None;
            }
            let result_ids = message
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                    _ => None,
                })
                .collect::<Option<BTreeSet<_>>>()?;
            if result_ids.len() != message.content.len() {
                return None;
            }
            Some(result_ids)
        }) else {
            return false;
        };
        return results == calls.into_keys().collect();
    }
    if terminal.stop_reason == "max_turns" {
        return !conversation.is_empty();
    }
    let Some(assistant) = conversation.iter().rev().find_map(|message| {
        let message = serde_json::from_value::<Message>(message.clone()).ok()?;
        matches!(message.role, Role::Assistant).then_some(message)
    }) else {
        return false;
    };
    let text = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    text == terminal.text
}

fn validate_tool_round_checkpoint_suffix(
    suffix: &[crate::session_journal::JournalEnvelope],
    turn_id: &str,
    conversation: &[serde_json::Value],
) -> bool {
    let Some(calls) = recovery_tool_call_authority(conversation) else {
        return false;
    };
    let mut approval_ids = BTreeSet::new();
    let mut tool_execution_ids = BTreeSet::new();
    let mut hook_phase_ids = BTreeSet::new();

    suffix.iter().all(|entry| match &entry.event {
        SessionEvent::ApprovalRequested {
            approval_id,
            origin:
                crate::session_journal::ApprovalOrigin::Turn {
                    turn_id: approval_turn_id,
                },
            ..
        } if approval_turn_id == turn_id && calls.contains_key(approval_id) => {
            approval_ids.insert(approval_id.clone())
        }
        SessionEvent::ApprovalResolved { approval_id, .. } => approval_ids.remove(approval_id),
        SessionEvent::ToolIntentRecordedV2 {
            tool_execution_id,
            provider_call_id,
            turn_id: tool_turn_id,
            ordinal,
            tool,
            requested_input_digest,
            ..
        } if tool_turn_id == turn_id
            && calls.get(provider_call_id).is_some_and(|expected| {
                expected.ordinal == *ordinal
                    && expected.tool == *tool
                    && expected.requested_input_digest == *requested_input_digest
            }) =>
        {
            tool_execution_ids.insert(tool_execution_id.clone())
        }
        SessionEvent::ToolExecutionStarted { tool_execution_id }
        | SessionEvent::ToolExecutionFinished {
            tool_execution_id, ..
        }
        | SessionEvent::ToolExecutionNotStarted {
            tool_execution_id, ..
        }
        | SessionEvent::ToolExecutionUnknown {
            tool_execution_id, ..
        }
        | SessionEvent::ToolExecutionResolved {
            tool_execution_id, ..
        } => tool_execution_ids.contains(tool_execution_id),
        SessionEvent::HookPhasePrepared {
            hook_phase_id,
            turn_id: hook_turn_id,
            provider_call_id,
            ordinal,
            ..
        } if hook_turn_id == turn_id
            && calls
                .get(provider_call_id)
                .is_some_and(|expected| expected.ordinal == *ordinal) =>
        {
            hook_phase_ids.insert(hook_phase_id.clone())
        }
        SessionEvent::HookPhaseStarted { hook_phase_id, .. }
        | SessionEvent::HookPhaseFinished { hook_phase_id, .. }
        | SessionEvent::HookPhaseNotStarted { hook_phase_id, .. }
        | SessionEvent::HookPhaseNotApplicable { hook_phase_id }
        | SessionEvent::HookPhaseAbandonedUnknown { hook_phase_id } => {
            hook_phase_ids.contains(hook_phase_id)
        }
        // Budget authority is independently cursor-bound by the reducer and
        // may advance while an approved tool executes.
        SessionEvent::BudgetAuthorityCommitted { .. } => true,
        _ => false,
    })
}

fn validate_provider_checkpoint_suffix(
    suffix: &[crate::session_journal::JournalEnvelope],
    turn_id: &str,
    dispatch_id: Option<&str>,
    request_digest: Option<&str>,
) -> Option<ValidatedProviderSuffix> {
    let expected_dispatch_id = dispatch_id?;
    let mut attempt_ids = BTreeSet::new();
    let mut streams = BTreeMap::<String, String>::new();

    for entry in suffix {
        match &entry.event {
            SessionEvent::ProviderAttemptPreparedV2 {
                attempt_id,
                dispatch_id: attempt_dispatch_id,
                turn_id: attempt_turn_id,
                purpose: crate::session_journal::ProviderAttemptPurpose::Conversation,
                request_digest: attempt_request_digest,
                ..
            } if attempt_turn_id == turn_id
                && attempt_dispatch_id == expected_dispatch_id
                && request_digest == Some(attempt_request_digest.as_str())
                && attempt_ids.insert(attempt_id.clone()) => {}
            SessionEvent::ProviderAttemptStarted { attempt_id }
                if attempt_ids.contains(attempt_id) => {}
            SessionEvent::StreamStarted {
                stream_id,
                attempt_id,
            } if attempt_ids.contains(attempt_id)
                && streams
                    .insert(stream_id.clone(), attempt_id.clone())
                    .is_none() => {}
            SessionEvent::StreamBatchCommitted { stream_id, .. }
            | SessionEvent::StreamFinished { stream_id }
                if streams.contains_key(stream_id) => {}
            SessionEvent::ProviderAttemptFinishedV2 {
                attempt_id,
                dispatch_id: terminal_dispatch_id,
                ..
            }
            | SessionEvent::ProviderAttemptNotStartedV2 {
                attempt_id,
                dispatch_id: terminal_dispatch_id,
                ..
            } if attempt_ids.contains(attempt_id)
                && terminal_dispatch_id == expected_dispatch_id => {}
            // Provider budget admission and settlement are journaled between
            // the request checkpoint and the physical-attempt receipts. The
            // reducer has already cursor-bound and validated this authority.
            SessionEvent::BudgetAuthorityCommitted { .. } => {}
            _ => return None,
        }
    }

    Some(ValidatedProviderSuffix {
        correlated_attempt_ids: attempt_ids,
    })
}

fn turn_descendants_match_checkpoint(
    state: &crate::session_journal::ReducedSessionState,
    turn_id: &str,
    correlated_attempt_ids: &BTreeSet<String>,
) -> bool {
    if correlated_attempt_ids.is_empty() {
        return crate::session_journal::require_turn_descendants_terminal(state, turn_id).is_ok();
    }

    let approval_belongs_to_turn = |origin: &crate::session_journal::ApprovalOrigin| match origin {
        crate::session_journal::ApprovalOrigin::Turn {
            turn_id: origin_turn,
        } => origin_turn == turn_id,
        crate::session_journal::ApprovalOrigin::ProviderAttempt { attempt_id } => state
            .provider_attempts
            .get(attempt_id)
            .is_some_and(|attempt| attempt.turn_id == turn_id),
        crate::session_journal::ApprovalOrigin::ToolExecution { tool_execution_id } => state
            .tools
            .get(tool_execution_id)
            .is_some_and(|tool| tool.turn_id == turn_id),
        crate::session_journal::ApprovalOrigin::Child { child_id } => state
            .children
            .get(child_id)
            .is_some_and(|child| child.turn_id == turn_id),
        crate::session_journal::ApprovalOrigin::Delivery { delivery_id } => {
            state.deliveries.get(delivery_id).is_some_and(|delivery| {
                matches!(
                    &delivery.origin,
                    DeliveryOrigin::Turn {
                        turn_id: origin_turn
                    } if origin_turn == turn_id
                )
            })
        }
    };
    let budget_belongs_to_turn = |owner: &crate::session_journal::BudgetOwner| match owner {
        crate::session_journal::BudgetOwner::Session => false,
        crate::session_journal::BudgetOwner::Turn {
            turn_id: owner_turn,
        } => owner_turn == turn_id,
        crate::session_journal::BudgetOwner::ProviderAttempt { attempt_id } => state
            .provider_attempts
            .get(attempt_id)
            .is_some_and(|attempt| attempt.turn_id == turn_id),
        crate::session_journal::BudgetOwner::ToolExecution { tool_execution_id } => state
            .tools
            .get(tool_execution_id)
            .is_some_and(|tool| tool.turn_id == turn_id),
        crate::session_journal::BudgetOwner::Child { child_id } => state
            .children
            .get(child_id)
            .is_some_and(|child| child.turn_id == turn_id),
    };
    let approvals_terminal = state.approvals.values().all(|approval| {
        approval.resolution.is_some() || !approval_belongs_to_turn(&approval.origin)
    });
    let providers_terminal_or_correlated = state.provider_attempts.iter().all(|(id, attempt)| {
        attempt.turn_id != turn_id
            || !matches!(
                attempt.effect,
                ExternalEffectState::Prepared | ExternalEffectState::Unknown
            )
            || correlated_attempt_ids.contains(id)
    });
    let tools_terminal = state.tools.values().all(|tool| {
        tool.turn_id != turn_id
            || !matches!(
                tool.effect,
                crate::session_journal::ToolEffectState::Prepared
                    | crate::session_journal::ToolEffectState::Running
                    | crate::session_journal::ToolEffectState::Unknown { .. }
            )
    });
    let hooks_terminal = state.hook_phases.values().all(|phase| {
        phase.turn_id != turn_id
            || matches!(
                phase.state,
                crate::session_journal::HookPhaseState::NotStarted { .. }
                    | crate::session_journal::HookPhaseState::NotApplicable
                    | crate::session_journal::HookPhaseState::Consumed { .. }
            )
    });
    let children_terminal = state.children.values().all(|child| {
        child.turn_id != turn_id
            || !matches!(
                child.effect,
                ExternalEffectState::Prepared | ExternalEffectState::Unknown
            )
    });
    let deliveries_terminal = state.deliveries.values().all(|delivery| {
        !matches!(
            &delivery.origin,
            DeliveryOrigin::Turn {
                turn_id: origin_turn
            } if origin_turn == turn_id
        ) || !matches!(
            delivery.effect,
            ExternalEffectState::Prepared | ExternalEffectState::Unknown
        )
    });
    let budgets_terminal = state.budgets.values().all(|budget| {
        budget.used.is_some() || budget.released || !budget_belongs_to_turn(&budget.owner)
    });

    approvals_terminal
        && providers_terminal_or_correlated
        && tools_terminal
        && hooks_terminal
        && children_terminal
        && deliveries_terminal
        && budgets_terminal
}

fn latest_valid_recovery_checkpoint(
    state: &crate::session_journal::ReducedSessionState,
    entries: &[crate::session_journal::JournalEnvelope],
    turn_id: &str,
) -> Option<ValidatedRecoveryCheckpoint> {
    let (checkpoint_index, checkpoint_id, checkpoint) =
        entries
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, entry)| match &entry.event {
                SessionEvent::CheckpointCommitted {
                    checkpoint_id,
                    purpose: crate::session_journal::CheckpointPurpose::Recovery,
                    origin:
                        crate::session_journal::CheckpointOrigin::Turn {
                            turn_id: checkpoint_turn,
                        },
                    state: checkpoint,
                    ..
                } if checkpoint_turn == turn_id => Some((index, checkpoint_id, checkpoint)),
                SessionEvent::ConversationRecoveryCheckpointCommitted {
                    turn_id: checkpoint_turn,
                    checkpoint_id,
                    checkpoint,
                    ..
                } if checkpoint_turn == turn_id => Some((index, checkpoint_id, checkpoint)),
                SessionEvent::ConversationRecoveryCheckpointCommittedV2 {
                    turn_id: checkpoint_turn,
                    checkpoint_id,
                    checkpoint,
                    ..
                } if checkpoint_turn == turn_id => Some((index, checkpoint_id, checkpoint)),
                _ => None,
            })?;
    let checkpoint = RecoveryCheckpoint::from_value(checkpoint).ok()?;
    if checkpoint.message_count != u64::try_from(state.conversation.len()).ok()? {
        return None;
    }
    let conversation = serde_json::Value::Array(state.conversation.clone());
    let digest = crate::session_journal::state_payload_digest(&conversation).ok()?;
    if checkpoint.conversation_digest != digest {
        return None;
    }
    let tail_role = state
        .conversation
        .last()
        .and_then(|message| message.get("role"))
        .and_then(serde_json::Value::as_str);
    let tail_matches_action = match checkpoint.next_action {
        RecoveryNextAction::ProviderDispatch | RecoveryNextAction::ContinueLoop => {
            tail_role == Some("user")
        }
        RecoveryNextAction::ContinueToolRound => {
            recovery_tool_call_authority(&state.conversation).is_some()
        }
        RecoveryNextAction::CommitTurn => {
            recovery_terminal_authority(&state.conversation, checkpoint.terminal_result.as_ref())
        }
    };
    if !tail_matches_action {
        return None;
    }

    let suffix = &entries[checkpoint_index + 1..];
    let correlated_attempt_ids = match checkpoint.next_action {
        RecoveryNextAction::ProviderDispatch => {
            validate_provider_checkpoint_suffix(
                suffix,
                turn_id,
                checkpoint.dispatch_id.as_deref(),
                checkpoint.request_digest.as_deref(),
            )?
            .correlated_attempt_ids
        }
        RecoveryNextAction::ContinueLoop if suffix.is_empty() => BTreeSet::new(),
        RecoveryNextAction::ContinueToolRound
            if validate_tool_round_checkpoint_suffix(suffix, turn_id, &state.conversation) =>
        {
            BTreeSet::new()
        }
        RecoveryNextAction::CommitTurn if suffix.is_empty() => BTreeSet::new(),
        RecoveryNextAction::ContinueLoop
        | RecoveryNextAction::ContinueToolRound
        | RecoveryNextAction::CommitTurn => return None,
    };
    if !matches!(
        checkpoint.next_action,
        RecoveryNextAction::ContinueToolRound
    ) && !turn_descendants_match_checkpoint(state, turn_id, &correlated_attempt_ids)
    {
        return None;
    }

    Some(ValidatedRecoveryCheckpoint {
        checkpoint_id: checkpoint_id.clone(),
        checkpoint,
    })
}

impl RecoveryPlan {
    pub fn from_journal(journal: &SessionJournal) -> Result<Self, JournalError> {
        let authority = journal.committed_authority()?;
        Self::from_reduced_state(authority.state, authority.entries, journal.session_id()?)
    }

    pub(crate) fn recovered_tool_round_authority(
        journal: &SessionJournal,
        turn_id: &str,
    ) -> Result<RecoveredToolRoundAuthority, JournalError> {
        let authority = journal.committed_authority()?;
        let validated =
            latest_valid_recovery_checkpoint(&authority.state, &authority.entries, turn_id)
                .filter(|validated| {
                    matches!(
                        validated.checkpoint.next_action,
                        RecoveryNextAction::ContinueToolRound
                    )
                })
                .ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "recovered approval is not bound to a valid tool-round checkpoint"
                            .to_string(),
                    )
                })?;
        Ok(RecoveredToolRoundAuthority {
            conversation: authority.state.conversation,
            checkpoint: validated.checkpoint,
        })
    }

    fn from_reduced_state(
        state: crate::session_journal::ReducedSessionState,
        entries: Vec<crate::session_journal::JournalEnvelope>,
        fallback_session_id: String,
    ) -> Result<Self, JournalError> {
        let session_id = match state.session_id.clone() {
            Some(session_id) => session_id,
            None => fallback_session_id,
        };
        let active_turns = state
            .turns
            .iter()
            .filter(|(_, turn)| turn.completion.is_none())
            .collect::<Vec<_>>();
        if active_turns.len() > 1 {
            return Err(JournalError::InvalidTransition(format!(
                "multiple active turns in recovery state: {}",
                active_turns
                    .iter()
                    .map(|(turn_id, _)| turn_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let disposition = match active_turns.first() {
            None => RecoveryDisposition::Ready,
            Some((turn_id, turn)) => {
                let turn_id = (*turn_id).clone();
                let validated_checkpoint =
                    latest_valid_recovery_checkpoint(&state, &entries, &turn_id);
                let unresolved_hook_outcome = state.hook_phases.iter().any(|(phase_id, phase)| {
                    if phase.turn_id != turn_id {
                        return false;
                    }
                    match &phase.state {
                        crate::session_journal::HookPhaseState::Finished { .. }
                            if phase.phase == crate::session_journal::ToolHookPhase::PreToolUse =>
                        {
                            !state.tools.values().any(|tool| {
                                tool.pre_hook_phase_id.as_deref() == Some(phase_id.as_str())
                            })
                        }
                        crate::session_journal::HookPhaseState::Prepared
                            if phase.phase
                                == crate::session_journal::ToolHookPhase::PostToolUse =>
                        {
                            phase
                                .tool_execution_id
                                .as_ref()
                                .and_then(|tool_id| state.tools.get(tool_id))
                                .is_none_or(|tool| {
                                    !matches!(
                                        tool.effect,
                                        crate::session_journal::ToolEffectState::Prepared
                                            | crate::session_journal::ToolEffectState::NotStarted
                                    ) && tool.resolution_source.is_none()
                                })
                        }
                        crate::session_journal::HookPhaseState::Prepared
                        | crate::session_journal::HookPhaseState::Started { .. }
                        | crate::session_journal::HookPhaseState::Finished { .. }
                        | crate::session_journal::HookPhaseState::AbandonedUnknown => true,
                        crate::session_journal::HookPhaseState::NotStarted { .. }
                        | crate::session_journal::HookPhaseState::NotApplicable
                        | crate::session_journal::HookPhaseState::Consumed { .. } => false,
                    }
                });
                let unresolved_tools = state
                    .tools
                    .iter()
                    .filter(|(_, tool)| {
                        tool.turn_id == turn_id && tool.effect.requires_reconciliation()
                    })
                    .map(|(tool_execution_id, _)| tool_execution_id.clone())
                    .collect::<Vec<_>>();
                if !unresolved_tools.is_empty() {
                    RecoveryDisposition::ReconciliationRequired {
                        turn_id,
                        tool_execution_ids: unresolved_tools,
                    }
                } else if unresolved_hook_outcome {
                    RecoveryDisposition::Blocked {
                        turn_id,
                        reason: RecoveryBlocker::HookOutcomeUnknown,
                    }
                } else {
                    let pending_approvals = state
                        .approvals
                        .iter()
                        .filter(|(_, approval)| {
                            approval.resolution.is_none()
                                && match &approval.origin {
                                    crate::session_journal::ApprovalOrigin::Turn {
                                        turn_id: origin_turn,
                                    } => origin_turn == &turn_id,
                                    crate::session_journal::ApprovalOrigin::ProviderAttempt {
                                        attempt_id,
                                    } => state
                                        .provider_attempts
                                        .get(attempt_id)
                                        .is_some_and(|attempt| attempt.turn_id == turn_id),
                                    crate::session_journal::ApprovalOrigin::ToolExecution {
                                        tool_execution_id,
                                    } => state
                                        .tools
                                        .get(tool_execution_id)
                                        .is_some_and(|tool| tool.turn_id == turn_id),
                                    crate::session_journal::ApprovalOrigin::Child { child_id } => {
                                        state
                                            .children
                                            .get(child_id)
                                            .is_some_and(|child| child.turn_id == turn_id)
                                    }
                                    crate::session_journal::ApprovalOrigin::Delivery {
                                        delivery_id,
                                    } => {
                                        state.deliveries.get(delivery_id).is_some_and(|delivery| {
                                            matches!(
                                                &delivery.origin,
                                                DeliveryOrigin::Turn { turn_id: origin_turn }
                                                    if origin_turn == &turn_id
                                            )
                                        })
                                    }
                                }
                        })
                        .map(|(approval_id, _)| approval_id.clone())
                        .collect::<Vec<_>>();
                    if !pending_approvals.is_empty() {
                        if validated_checkpoint.as_ref().is_some_and(|validated| {
                            matches!(
                                validated.checkpoint.next_action,
                                RecoveryNextAction::ContinueToolRound
                            )
                        }) {
                            RecoveryDisposition::AwaitApproval {
                                turn_id,
                                approval_ids: pending_approvals,
                            }
                        } else {
                            RecoveryDisposition::Blocked {
                                turn_id,
                                reason: RecoveryBlocker::ContextCheckpointMissing,
                            }
                        }
                    } else if state.provider_attempts.values().any(|attempt| {
                        attempt.turn_id == turn_id
                            && matches!(attempt.effect, ExternalEffectState::Unknown)
                    }) {
                        RecoveryDisposition::Blocked {
                            turn_id,
                            reason: RecoveryBlocker::ProviderOutcomeUnknown,
                        }
                    } else if state.children.values().any(|child| {
                        child.turn_id == turn_id
                            && matches!(child.effect, ExternalEffectState::Unknown)
                    }) {
                        RecoveryDisposition::Blocked {
                            turn_id,
                            reason: RecoveryBlocker::ChildOutcomeUnknown,
                        }
                    } else if state.deliveries.values().any(|delivery| {
                        matches!(delivery.effect, ExternalEffectState::Unknown)
                            && matches!(
                                &delivery.origin,
                                DeliveryOrigin::Turn { turn_id: origin_turn }
                                    if origin_turn == &turn_id
                            )
                    }) {
                        RecoveryDisposition::Blocked {
                            turn_id,
                            reason: RecoveryBlocker::DeliveryOutcomeUnknown,
                        }
                    } else {
                        let turn_start_is_head = matches!(
                            entries.last().map(|entry| &entry.event),
                            Some(SessionEvent::TurnStarted { turn_id: head_id, .. })
                                if head_id == &turn_id
                        );
                        let has_other_turn_state = state
                            .provider_attempts
                            .values()
                            .any(|attempt| attempt.turn_id == turn_id)
                            || state.tools.values().any(|tool| tool.turn_id == turn_id)
                            || state
                                .children
                                .values()
                                .any(|child| child.turn_id == turn_id);
                        if turn_start_is_head && !has_other_turn_state {
                            // TurnStarted does not contain the plan/approval/tool
                            // posture that governed the original turn. Re-entering
                            // from this boundary could silently restore a broader
                            // authority, so only an exact typed checkpoint may
                            // grant continuation.
                            RecoveryDisposition::Blocked {
                                turn_id,
                                reason: RecoveryBlocker::ContextCheckpointMissing,
                            }
                        } else if let Some(validated) = validated_checkpoint {
                            RecoveryDisposition::ContinueCheckpoint {
                                turn_id,
                                user_message: turn.user_message.clone(),
                                checkpoint_id: validated.checkpoint_id,
                                checkpoint: Box::new(validated.checkpoint),
                            }
                        } else {
                            RecoveryDisposition::Blocked {
                                turn_id,
                                reason: RecoveryBlocker::ContextCheckpointMissing,
                            }
                        }
                    }
                }
            }
        };

        Ok(Self {
            session_id,
            journal_sequence: state.last_seq,
            journal_digest: state.last_checksum.clone(),
            state_digest: state.digest()?,
            budget: recovery_budget_snapshot(&state)?,
            disposition,
        })
    }

    #[must_use]
    pub fn cursor(&self) -> RecoveryCursor {
        RecoveryCursor {
            journal_sequence: self.journal_sequence,
            journal_digest: self.journal_digest.clone(),
        }
    }

    /// Build the sanitized recovery projection at an already-observed cursor.
    ///
    /// This is the baseline paired with [`Self::replay_after`]. It never labels
    /// current-head state with an older cursor. A compacted prefix that cannot
    /// be reduced from the retained journal fails as a history gap.
    pub fn from_journal_at(
        journal: &SessionJournal,
        cursor: &RecoveryCursor,
    ) -> Result<Self, RecoveryUnavailableReason> {
        if cursor.journal_digest.is_empty() {
            return Err(RecoveryUnavailableReason::CursorInvalid);
        }
        let authority = journal
            .committed_authority()
            .map_err(|_| RecoveryUnavailableReason::JournalCorrupt)?;
        let session_id = journal
            .session_id()
            .map_err(|_| RecoveryUnavailableReason::JournalCorrupt)?;

        let (state, entries) = match cursor.journal_sequence {
            None => {
                if cursor.journal_digest != crate::session_journal::GENESIS_CHECKSUM {
                    return Err(RecoveryUnavailableReason::CursorDigestMismatch);
                }
                if authority
                    .entries
                    .first()
                    .is_some_and(|entry| entry.seq != 0)
                {
                    return Err(RecoveryUnavailableReason::HistoryGap);
                }
                (
                    crate::session_journal::ReducedSessionState::default(),
                    Vec::new(),
                )
            }
            Some(sequence) => {
                let Some(current_sequence) = authority.state.last_seq else {
                    return Err(RecoveryUnavailableReason::CursorAhead);
                };
                if sequence > current_sequence {
                    return Err(RecoveryUnavailableReason::CursorAhead);
                }
                if sequence == current_sequence {
                    if cursor.journal_digest != authority.state.last_checksum {
                        return Err(RecoveryUnavailableReason::CursorDigestMismatch);
                    }
                    (authority.state, authority.entries)
                } else {
                    let Some(index) = authority
                        .entries
                        .iter()
                        .position(|entry| entry.seq == sequence)
                    else {
                        return Err(RecoveryUnavailableReason::HistoryGap);
                    };
                    if authority.entries[index].checksum != cursor.journal_digest {
                        return Err(RecoveryUnavailableReason::CursorDigestMismatch);
                    }
                    let entries = authority.entries[..=index].to_vec();
                    let state = if authority.entries.first().map(|entry| entry.seq) == Some(0) {
                        crate::session_journal::replay_state(&entries)
                            .map_err(|_| RecoveryUnavailableReason::JournalCorrupt)?
                    } else {
                        let snapshot = authority
                            .base_snapshot
                            .as_ref()
                            .ok_or(RecoveryUnavailableReason::HistoryGap)?;
                        let Some(snapshot_sequence) = snapshot.cursor else {
                            return Err(RecoveryUnavailableReason::HistoryGap);
                        };
                        if sequence < snapshot_sequence
                            || authority.entries.first().map(|entry| entry.seq)
                                != Some(snapshot_sequence)
                        {
                            return Err(RecoveryUnavailableReason::HistoryGap);
                        }
                        if authority.entries[0].checksum != snapshot.cursor_checksum {
                            return Err(RecoveryUnavailableReason::JournalCorrupt);
                        }
                        if index == 0 {
                            snapshot.state.clone()
                        } else {
                            authority.entries[1..=index]
                                .iter()
                                .try_fold(snapshot.state.clone(), crate::session_journal::reduce)
                                .map_err(|_| RecoveryUnavailableReason::JournalCorrupt)?
                        }
                    };
                    (state, entries)
                }
            }
        };

        Self::from_reduced_state(state, entries, session_id)
            .map_err(|_| RecoveryUnavailableReason::JournalCorrupt)
    }

    /// Return content-free journal milestones strictly after a cursor.
    ///
    /// Cursor validation is deliberately separate from the reduced-state
    /// projection: a matching sequence with a different digest is stale or
    /// forged, while a cursor older than the retained compacted suffix is an
    /// explicit history gap. Neither condition is silently promoted to an
    /// empty replay.
    pub fn replay_after(
        journal: &SessionJournal,
        after: &RecoveryCursor,
    ) -> Result<Vec<RecoveryReplayItem>, RecoveryUnavailableReason> {
        if after.journal_digest.is_empty() {
            return Err(RecoveryUnavailableReason::CursorInvalid);
        }
        let authority = journal
            .committed_authority()
            .map_err(|_| RecoveryUnavailableReason::JournalCorrupt)?;
        let entries = authority.entries;

        match after.journal_sequence {
            None => {
                if after.journal_digest != crate::session_journal::GENESIS_CHECKSUM {
                    return Err(RecoveryUnavailableReason::CursorDigestMismatch);
                }
                if entries.first().is_some_and(|entry| entry.seq != 0) {
                    return Err(RecoveryUnavailableReason::HistoryGap);
                }
            }
            Some(sequence) => {
                let current = entries.last().map(|entry| entry.seq);
                if current.is_none_or(|current| sequence > current) {
                    return Err(RecoveryUnavailableReason::CursorAhead);
                }
                match entries.iter().find(|entry| entry.seq == sequence) {
                    Some(entry) if entry.checksum != after.journal_digest => {
                        return Err(RecoveryUnavailableReason::CursorDigestMismatch);
                    }
                    Some(_) => {}
                    None => return Err(RecoveryUnavailableReason::HistoryGap),
                }
            }
        }

        Ok(entries
            .into_iter()
            .filter(|entry| after.journal_sequence.is_none_or(|seq| entry.seq > seq))
            .map(|entry| {
                let kind = replay_kind(&entry.event);
                RecoveryReplayItem {
                    cursor: RecoveryCursor {
                        journal_sequence: Some(entry.seq),
                        journal_digest: entry.checksum,
                    },
                    turn_id: replay_turn_id(&entry.event),
                    kind,
                }
            })
            .collect())
    }

    /// Convert the internal execution decision into the sanitized host view.
    ///
    /// The projection intentionally omits the stored user message and any
    /// effect evidence. Hosts receive only stable identifiers and typed state.
    #[must_use]
    pub fn protocol_projection(&self) -> (RecoveryLifecycle, Option<RecoveryTurnSnapshot>) {
        match &self.disposition {
            RecoveryDisposition::Ready => (RecoveryLifecycle::Ready, None),
            RecoveryDisposition::ContinueTurnStart { turn_id, .. } => (
                RecoveryLifecycle::Ready,
                Some(RecoveryTurnSnapshot {
                    turn_id: turn_id.clone(),
                    msg_id: None,
                    lifecycle: RecoveryLifecycle::Ready,
                    pending_call_id: None,
                    reconcile_reason: None,
                }),
            ),
            RecoveryDisposition::ContinueCheckpoint { turn_id, .. } => (
                RecoveryLifecycle::Ready,
                Some(RecoveryTurnSnapshot {
                    turn_id: turn_id.clone(),
                    msg_id: None,
                    lifecycle: RecoveryLifecycle::Ready,
                    pending_call_id: None,
                    reconcile_reason: None,
                }),
            ),
            RecoveryDisposition::AwaitApproval {
                turn_id,
                approval_ids,
            } => (
                RecoveryLifecycle::AwaitingApproval,
                Some(RecoveryTurnSnapshot {
                    turn_id: turn_id.clone(),
                    msg_id: None,
                    lifecycle: RecoveryLifecycle::AwaitingApproval,
                    pending_call_id: approval_ids.first().cloned(),
                    reconcile_reason: Some(RecoveryReconcileReason::ApprovalExpired),
                }),
            ),
            RecoveryDisposition::ReconciliationRequired {
                turn_id,
                tool_execution_ids,
            } => (
                RecoveryLifecycle::ReconciliationRequired,
                Some(RecoveryTurnSnapshot {
                    turn_id: turn_id.clone(),
                    msg_id: None,
                    lifecycle: RecoveryLifecycle::ReconciliationRequired,
                    pending_call_id: tool_execution_ids.first().cloned(),
                    reconcile_reason: Some(RecoveryReconcileReason::ToolOutcomeUnknown),
                }),
            ),
            RecoveryDisposition::Blocked { turn_id, reason } => {
                let reconcile_reason = match reason {
                    RecoveryBlocker::ProviderOutcomeUnknown => {
                        RecoveryReconcileReason::ProviderOutcomeUnknown
                    }
                    RecoveryBlocker::HookOutcomeUnknown => {
                        RecoveryReconcileReason::EffectRequiresOperator
                    }
                    RecoveryBlocker::ContextCheckpointMissing => {
                        RecoveryReconcileReason::ContextUnrestorable
                    }
                    RecoveryBlocker::ChildOutcomeUnknown
                    | RecoveryBlocker::DeliveryOutcomeUnknown => {
                        RecoveryReconcileReason::EffectRequiresOperator
                    }
                };
                (
                    RecoveryLifecycle::Suspended,
                    Some(RecoveryTurnSnapshot {
                        turn_id: turn_id.clone(),
                        msg_id: None,
                        lifecycle: RecoveryLifecycle::Suspended,
                        pending_call_id: None,
                        reconcile_reason: Some(reconcile_reason),
                    }),
                )
            }
        }
    }
}

fn replay_kind(event: &SessionEvent) -> RecoveryReplayKind {
    match event {
        SessionEvent::TurnStarted { .. } => RecoveryReplayKind::TurnStarted,
        SessionEvent::StreamStarted { .. } => RecoveryReplayKind::StreamStarted,
        SessionEvent::StreamFinished { .. } => RecoveryReplayKind::StreamCommitted,
        SessionEvent::ApprovalRequested { .. } => RecoveryReplayKind::ApprovalRequested,
        SessionEvent::ApprovalResolved { .. } => RecoveryReplayKind::ApprovalResolved,
        SessionEvent::ToolExecutionStarted { .. } => RecoveryReplayKind::ToolStarted,
        SessionEvent::ToolExecutionFinished { .. }
        | SessionEvent::ToolExecutionNotStarted { .. }
        | SessionEvent::ToolExecutionResolved { .. } => RecoveryReplayKind::ToolCommitted,
        SessionEvent::ToolExecutionUnknown { .. } => RecoveryReplayKind::EffectUncertain,
        SessionEvent::TurnCommitted { .. } => RecoveryReplayKind::TurnCompleted,
        SessionEvent::TurnCancelled { .. } => RecoveryReplayKind::TurnCancelled,
        SessionEvent::TurnFailed { .. } => RecoveryReplayKind::TurnFailed,
        _ => RecoveryReplayKind::StateAdvanced,
    }
}

fn replay_turn_id(event: &SessionEvent) -> Option<String> {
    match event {
        SessionEvent::TurnStarted { turn_id, .. }
        | SessionEvent::TurnCommitted { turn_id, .. }
        | SessionEvent::TurnCancelled { turn_id }
        | SessionEvent::TurnFailed { turn_id, .. }
        | SessionEvent::ConversationMessageCommitted { turn_id, .. }
        | SessionEvent::ConversationStateCommitted { turn_id, .. }
        | SessionEvent::ConversationRecoveryCheckpointCommitted { turn_id, .. }
        | SessionEvent::ConversationRecoveryCheckpointCommittedV2 { turn_id, .. }
        | SessionEvent::HookPhasePrepared { turn_id, .. }
        | SessionEvent::ProviderAttemptPrepared { turn_id, .. }
        | SessionEvent::ProviderAttemptPreparedV2 { turn_id, .. }
        | SessionEvent::ToolIntentRecorded { turn_id, .. }
        | SessionEvent::ToolIntentRecordedV2 { turn_id, .. }
        | SessionEvent::ChildPrepared { turn_id, .. } => Some(turn_id.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_journal::{
        BUDGET_AUTHORITY_SCHEMA_VERSION, BudgetAuthorityCursor, BudgetAuthorityState,
        BudgetWallClockAuthority, CheckpointOrigin, CheckpointPurpose, CompletionOutcome,
        DeliveryOrigin, ProviderAttemptPurpose, ProviderStreamEvent, SessionEvent,
        ToolUnknownReason, state_payload_digest,
    };
    use wcore_budget::{BudgetCap, BudgetTracker, ExecutionBudget};

    fn journal() -> (tempfile::TempDir, SessionJournal) {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        (dir, journal)
    }

    fn append_budget_authority(
        journal: &SessionJournal,
        authority_epoch: u64,
        tokens_used: u64,
        cost_used_usd: f64,
    ) -> crate::session_journal::JournalEnvelope {
        let state = journal.state().unwrap();
        let mut provider_tracker = BudgetTracker::new(
            BudgetCap::builder()
                .per_session_tokens(10_000)
                .per_session_usd(10.0)
                .build(),
        );
        provider_tracker
            .charge("budget-session", tokens_used, cost_used_usd)
            .unwrap();
        let execution_root = ExecutionBudget::default().start_root();
        let authority = BudgetAuthorityState {
            schema_version: BUDGET_AUTHORITY_SCHEMA_VERSION,
            authority_epoch,
            prior_cursor: BudgetAuthorityCursor {
                journal_sequence: state.last_seq,
                journal_checksum: state.last_checksum,
            },
            budget_session_id: "budget-session".into(),
            provider_tracker: provider_tracker.snapshot().unwrap(),
            provider_reservations: BTreeMap::new(),
            execution_root: execution_root.snapshot().unwrap(),
            active_turn: None,
            captured_at_unix_millis: authority_epoch,
            wall_clock: BudgetWallClockAuthority::ActiveRuntime,
            conversation_digest: state_payload_digest(&serde_json::Value::Array(
                state.conversation,
            ))
            .unwrap(),
        };
        journal
            .append(SessionEvent::BudgetAuthorityCommitted { authority })
            .unwrap()
    }

    #[test]
    fn empty_journal_is_ready() {
        let (_dir, journal) = journal();
        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(plan.disposition, RecoveryDisposition::Ready));
        assert_eq!(plan.journal_sequence, None);
    }

    #[test]
    fn turn_started_at_head_without_posture_blocks_continuation() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::Blocked {
                turn_id,
                reason: RecoveryBlocker::ContextCheckpointMissing,
            } if turn_id == "turn-1"
        ));
    }

    #[test]
    fn prepared_hook_phase_blocks_recovery_before_checkpoint_fallback() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-hook".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let descriptor_digest = "a".repeat(64);
        let hook_slots = vec![crate::session_journal::HookManifestSlot {
            ordinal: 0,
            slot_id: "rust-0".into(),
            source: crate::session_journal::HookSlotSource::Rust,
            descriptor_digest,
        }];
        let hook_manifest_digest =
            state_payload_digest(&serde_json::to_value(&hook_slots).expect("hook slots serialize"))
                .unwrap();
        journal
            .append(SessionEvent::HookPhasePrepared {
                hook_phase_id: "hook-prepared".into(),
                lifecycle_version: crate::session_journal::HOOK_PHASE_LIFECYCLE_VERSION,
                turn_id: "turn-hook".into(),
                provider_call_id: "call-1".into(),
                ordinal: 0,
                phase: crate::session_journal::ToolHookPhase::PreToolUse,
                tool_execution_id: None,
                input_digest: "b".repeat(64),
                hook_authority_digest: "c".repeat(64),
                hook_manifest_digest,
                hook_slots,
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::Blocked {
                turn_id,
                reason: RecoveryBlocker::HookOutcomeUnknown,
            } if turn_id == "turn-hook"
        ));
    }

    fn append_recovery_checkpoint(
        journal: &SessionJournal,
        turn_id: &str,
        conversation: &[serde_json::Value],
        conversation_digest: Option<String>,
        prepared_messages: Option<Vec<Message>>,
    ) {
        let durable_messages = conversation
            .iter()
            .cloned()
            .map(serde_json::from_value)
            .collect::<Result<Vec<Message>, _>>()
            .unwrap();
        let request = wcore_types::llm::LlmRequest {
            conversation_id: Some("conversation-1".into()),
            messages: prepared_messages.unwrap_or(durable_messages),
            ..Default::default()
        };
        let checkpoint = RecoveryCheckpoint {
            version: RECOVERY_CHECKPOINT_VERSION,
            conversation_id: "conversation-1".into(),
            next_action: RecoveryNextAction::ProviderDispatch,
            conversation_digest: conversation_digest.unwrap_or_else(|| {
                state_payload_digest(&serde_json::Value::Array(conversation.to_vec())).unwrap()
            }),
            message_count: conversation.len() as u64,
            turn_index: 0,
            stream_attempt: 0,
            overflow_retried: false,
            length_wedge_retried: false,
            request_digest: Some(
                crate::session_journal::provider_request_digest(&request).unwrap(),
            ),
            dispatch_id: Some("dispatch-1".into()),
            sealed_prepared_request: Some(crate::recovery_confidential::SealedPreparedRequest {
                envelope_version: 1,
                algorithm: "xchacha20-poly1305".into(),
                ciphertext: "AA".into(),
            }),
            posture: RecoveryPosture {
                plan_active: false,
                pre_plan_allow_list: Vec::new(),
                effective_allow_list: Vec::new(),
                conservatively_open_breakers: Vec::new(),
                authority_digest: "c".repeat(64),
                authority_component_digests: BTreeMap::new(),
                tool_hook_authority_version: TOOL_HOOK_RECOVERY_AUTHORITY_VERSION,
            },
            loop_guard: RecoveryLoopGuardState {
                last_signature: None,
                count: 0,
                threshold: 10,
            },
            failure_guard: RecoveryFailureGuardState {
                count: 0,
                threshold: 10,
            },
            run_usage: TokenUsage::default(),
            terminal_result: None,
        }
        .to_value()
        .unwrap();
        journal
            .append(SessionEvent::CheckpointCommitted {
                checkpoint_id: "checkpoint-1".into(),
                purpose: CheckpointPurpose::Recovery,
                origin: CheckpointOrigin::Turn {
                    turn_id: turn_id.into(),
                },
                state_digest: state_payload_digest(&checkpoint).unwrap(),
                state: checkpoint,
            })
            .unwrap();
    }

    fn user_message(text: &str) -> serde_json::Value {
        serde_json::to_value(Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        ))
        .unwrap()
    }

    fn journal_with_recovery_checkpoint() -> (tempfile::TempDir, SessionJournal, String) {
        let (dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let message = user_message("finish the task");
        journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 0,
                message: message.clone(),
                message_digest: state_payload_digest(&message).unwrap(),
            })
            .unwrap();
        append_recovery_checkpoint(
            &journal,
            "turn-1",
            std::slice::from_ref(&message),
            None,
            None,
        );
        let checkpoint = journal.state().unwrap().checkpoints["checkpoint-1"]
            .state
            .clone();
        let request_digest = RecoveryCheckpoint::from_value(&checkpoint)
            .unwrap()
            .request_digest
            .unwrap();
        (dir, journal, request_digest)
    }

    fn append_correlated_attempt(
        journal: &SessionJournal,
        attempt_id: &str,
        dispatch_id: &str,
        request_digest: &str,
    ) {
        journal
            .append(SessionEvent::ProviderAttemptPreparedV2 {
                attempt_id: attempt_id.into(),
                dispatch_id: dispatch_id.into(),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: request_digest.into(),
            })
            .unwrap();
    }

    fn assert_checkpoint_continuation(plan: &RecoveryPlan) {
        assert!(
            matches!(
                plan.disposition,
                RecoveryDisposition::ContinueCheckpoint {
                    ref turn_id,
                    ref checkpoint_id,
                    ..
                } if turn_id == "turn-1" && checkpoint_id == "checkpoint-1"
            ),
            "unexpected recovery disposition: {:?}",
            plan.disposition,
        );
    }

    #[test]
    fn exact_head_recovery_checkpoint_is_safe_to_continue() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let message = user_message("finish the task");
        journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 0,
                message: message.clone(),
                message_digest: state_payload_digest(&message).unwrap(),
            })
            .unwrap();
        append_recovery_checkpoint(
            &journal,
            "turn-1",
            std::slice::from_ref(&message),
            None,
            None,
        );

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(
            matches!(
                plan.disposition,
                RecoveryDisposition::ContinueCheckpoint {
                    ref turn_id,
                    ref user_message,
                    ref checkpoint_id,
                    ref checkpoint,
                } if turn_id == "turn-1"
                    && user_message == "finish the task"
                    && checkpoint_id == "checkpoint-1"
                    && matches!(checkpoint.next_action, RecoveryNextAction::ProviderDispatch)
            ),
            "unexpected recovery disposition: {:?}",
            plan.disposition,
        );
    }

    #[test]
    fn continue_loop_checkpoint_binds_the_durable_tool_result_tail() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let initial = user_message("finish the task");
        journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 0,
                message: initial.clone(),
                message_digest: state_payload_digest(&initial).unwrap(),
            })
            .unwrap();
        let tool_result = user_message("tool result");
        let conversation = vec![initial, tool_result.clone()];
        let checkpoint = RecoveryCheckpoint {
            version: RECOVERY_CHECKPOINT_VERSION,
            conversation_id: "conversation-1".into(),
            next_action: RecoveryNextAction::ContinueLoop,
            conversation_digest: state_payload_digest(&serde_json::Value::Array(
                conversation.clone(),
            ))
            .unwrap(),
            message_count: conversation.len() as u64,
            turn_index: 1,
            stream_attempt: 0,
            overflow_retried: false,
            length_wedge_retried: false,
            request_digest: None,
            dispatch_id: None,
            sealed_prepared_request: None,
            posture: RecoveryPosture {
                plan_active: false,
                pre_plan_allow_list: Vec::new(),
                effective_allow_list: Vec::new(),
                conservatively_open_breakers: Vec::new(),
                authority_digest: "c".repeat(64),
                authority_component_digests: BTreeMap::new(),
                tool_hook_authority_version: TOOL_HOOK_RECOVERY_AUTHORITY_VERSION,
            },
            loop_guard: RecoveryLoopGuardState {
                last_signature: None,
                count: 0,
                threshold: 10,
            },
            failure_guard: RecoveryFailureGuardState {
                count: 0,
                threshold: 10,
            },
            run_usage: TokenUsage::default(),
            terminal_result: None,
        }
        .to_value()
        .unwrap();
        journal
            .append(SessionEvent::ConversationRecoveryCheckpointCommitted {
                turn_id: "turn-1".into(),
                messages: conversation.clone(),
                messages_digest: state_payload_digest(&serde_json::Value::Array(conversation))
                    .unwrap(),
                checkpoint_id: "checkpoint-1".into(),
                checkpoint_state_digest: state_payload_digest(&checkpoint).unwrap(),
                checkpoint,
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::ContinueCheckpoint {
                checkpoint,
                ..
            } if matches!(checkpoint.next_action, RecoveryNextAction::ContinueLoop)
        ));
    }

    #[test]
    fn tool_round_checkpoint_is_resumable_at_pre_execution_boundary() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let initial = user_message("finish the task");
        let assistant = serde_json::to_value(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "README.md"}),
                extra: None,
            }],
        ))
        .unwrap();
        let hook_message = user_message("hook-injected guidance");
        let conversation = vec![initial, assistant, hook_message];
        let checkpoint = RecoveryCheckpoint {
            version: RECOVERY_CHECKPOINT_VERSION,
            conversation_id: "conversation-1".into(),
            next_action: RecoveryNextAction::ContinueToolRound,
            conversation_digest: state_payload_digest(&serde_json::Value::Array(
                conversation.clone(),
            ))
            .unwrap(),
            message_count: conversation.len() as u64,
            turn_index: 0,
            stream_attempt: 0,
            overflow_retried: false,
            length_wedge_retried: false,
            request_digest: None,
            dispatch_id: None,
            sealed_prepared_request: None,
            posture: RecoveryPosture {
                plan_active: false,
                pre_plan_allow_list: Vec::new(),
                effective_allow_list: Vec::new(),
                conservatively_open_breakers: Vec::new(),
                authority_digest: "c".repeat(64),
                authority_component_digests: BTreeMap::new(),
                tool_hook_authority_version: TOOL_HOOK_RECOVERY_AUTHORITY_VERSION,
            },
            loop_guard: RecoveryLoopGuardState {
                last_signature: None,
                count: 0,
                threshold: 10,
            },
            failure_guard: RecoveryFailureGuardState {
                count: 0,
                threshold: 10,
            },
            run_usage: TokenUsage::default(),
            terminal_result: None,
        }
        .to_value()
        .unwrap();
        journal
            .append(SessionEvent::ConversationRecoveryCheckpointCommitted {
                turn_id: "turn-1".into(),
                messages: conversation.clone(),
                messages_digest: state_payload_digest(&serde_json::Value::Array(conversation))
                    .unwrap(),
                checkpoint_id: "checkpoint-tool-round".into(),
                checkpoint_state_digest: state_payload_digest(&checkpoint).unwrap(),
                checkpoint,
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::ContinueCheckpoint {
                checkpoint,
                ..
            } if matches!(checkpoint.next_action, RecoveryNextAction::ContinueToolRound)
        ));
    }

    #[test]
    fn terminal_checkpoint_at_head_is_recognized_without_dispatch_identity() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let assistant = serde_json::to_value(Message::new(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
        ))
        .unwrap();
        journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 0,
                message: assistant.clone(),
                message_digest: state_payload_digest(&assistant).unwrap(),
            })
            .unwrap();
        let checkpoint = RecoveryCheckpoint {
            version: RECOVERY_CHECKPOINT_VERSION,
            conversation_id: "conversation-1".into(),
            next_action: RecoveryNextAction::CommitTurn,
            conversation_digest: state_payload_digest(&serde_json::Value::Array(vec![assistant]))
                .unwrap(),
            message_count: 1,
            turn_index: 0,
            stream_attempt: 0,
            overflow_retried: false,
            length_wedge_retried: false,
            request_digest: None,
            dispatch_id: None,
            sealed_prepared_request: None,
            posture: RecoveryPosture {
                plan_active: false,
                pre_plan_allow_list: Vec::new(),
                effective_allow_list: Vec::new(),
                conservatively_open_breakers: Vec::new(),
                authority_digest: "c".repeat(64),
                authority_component_digests: BTreeMap::new(),
                tool_hook_authority_version: TOOL_HOOK_RECOVERY_AUTHORITY_VERSION,
            },
            loop_guard: RecoveryLoopGuardState {
                last_signature: None,
                count: 0,
                threshold: 10,
            },
            failure_guard: RecoveryFailureGuardState {
                count: 0,
                threshold: 10,
            },
            run_usage: TokenUsage::default(),
            terminal_result: Some(RecoveryTerminalResult {
                completion: RecoveryTerminalCompletion::Committed,
                text: "done".into(),
                stop_reason: "end_turn".into(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
                usage_delta: TokenUsage::default(),
                turns: 1,
                active_window_percent: None,
                agent_run_id: None,
            }),
        }
        .to_value()
        .unwrap();
        journal
            .append(SessionEvent::CheckpointCommitted {
                checkpoint_id: "checkpoint-1".into(),
                purpose: CheckpointPurpose::Recovery,
                origin: CheckpointOrigin::Turn {
                    turn_id: "turn-1".into(),
                },
                state_digest: state_payload_digest(&checkpoint).unwrap(),
                state: checkpoint,
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::ContinueCheckpoint {
                checkpoint,
                ..
            } if matches!(checkpoint.next_action, RecoveryNextAction::CommitTurn)
        ));
    }

    #[test]
    fn terminal_checkpoint_accepts_digest_bound_external_edit_after_answer() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let assistant = serde_json::to_value(Message::new(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
        ))
        .unwrap();
        let edit_notice = user_message("User edited README.md while I was thinking");
        let conversation = vec![assistant, edit_notice];
        for (message_index, message) in conversation.iter().enumerate() {
            journal
                .append(SessionEvent::ConversationMessageCommitted {
                    turn_id: "turn-1".into(),
                    message_index: u64::try_from(message_index).unwrap(),
                    message: message.clone(),
                    message_digest: state_payload_digest(message).unwrap(),
                })
                .unwrap();
        }
        let checkpoint = RecoveryCheckpoint {
            version: RECOVERY_CHECKPOINT_VERSION,
            conversation_id: "conversation-1".into(),
            next_action: RecoveryNextAction::CommitTurn,
            conversation_digest: state_payload_digest(&serde_json::Value::Array(
                conversation.clone(),
            ))
            .unwrap(),
            message_count: conversation.len() as u64,
            turn_index: 0,
            stream_attempt: 0,
            overflow_retried: false,
            length_wedge_retried: false,
            request_digest: None,
            dispatch_id: None,
            sealed_prepared_request: None,
            posture: RecoveryPosture {
                plan_active: false,
                pre_plan_allow_list: Vec::new(),
                effective_allow_list: Vec::new(),
                conservatively_open_breakers: Vec::new(),
                authority_digest: "c".repeat(64),
                authority_component_digests: BTreeMap::new(),
                tool_hook_authority_version: TOOL_HOOK_RECOVERY_AUTHORITY_VERSION,
            },
            loop_guard: RecoveryLoopGuardState {
                last_signature: None,
                count: 0,
                threshold: 10,
            },
            failure_guard: RecoveryFailureGuardState {
                count: 0,
                threshold: 10,
            },
            run_usage: TokenUsage::default(),
            terminal_result: Some(RecoveryTerminalResult {
                completion: RecoveryTerminalCompletion::Committed,
                text: "done".into(),
                stop_reason: "end_turn".into(),
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
                usage_delta: TokenUsage::default(),
                turns: 1,
                active_window_percent: None,
                agent_run_id: None,
            }),
        }
        .to_value()
        .unwrap();
        journal
            .append(SessionEvent::ConversationRecoveryCheckpointCommitted {
                turn_id: "turn-1".into(),
                messages: conversation.clone(),
                messages_digest: state_payload_digest(&serde_json::Value::Array(conversation))
                    .unwrap(),
                checkpoint_id: "checkpoint-with-edit".into(),
                checkpoint_state_digest: state_payload_digest(&checkpoint).unwrap(),
                checkpoint,
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::ContinueCheckpoint {
                checkpoint,
                ..
            } if matches!(checkpoint.next_action, RecoveryNextAction::CommitTurn)
        ));
    }

    #[test]
    fn correlated_started_provider_suffix_is_publicly_suspended() {
        let (_dir, journal, request_digest) = journal_with_recovery_checkpoint();
        append_correlated_attempt(&journal, "attempt-1", "dispatch-1", &request_digest);
        journal
            .append(SessionEvent::ProviderAttemptStarted {
                attempt_id: "attempt-1".into(),
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::Blocked {
                ref turn_id,
                reason: RecoveryBlocker::ProviderOutcomeUnknown,
            } if turn_id == "turn-1"
        ));
        let (lifecycle, pending_turn) = plan.protocol_projection();
        assert_eq!(
            lifecycle,
            wcore_protocol::events::RecoveryLifecycle::Suspended
        );
        assert_eq!(
            pending_turn
                .as_ref()
                .and_then(|pending| pending.reconcile_reason),
            Some(wcore_protocol::events::RecoveryReconcileReason::ProviderOutcomeUnknown)
        );
    }

    #[test]
    fn correlated_terminal_provider_suffix_reaches_typed_recovery() {
        let (_dir, journal, request_digest) = journal_with_recovery_checkpoint();
        append_correlated_attempt(&journal, "attempt-1", "dispatch-1", &request_digest);
        journal
            .append(SessionEvent::ProviderAttemptStarted {
                attempt_id: "attempt-1".into(),
            })
            .unwrap();
        journal
            .append(SessionEvent::ProviderAttemptFinishedV2 {
                attempt_id: "attempt-1".into(),
                dispatch_id: "dispatch-1".into(),
                outcome: CompletionOutcome::Failed {
                    error: "transport".into(),
                },
                response_digest: None,
            })
            .unwrap();

        assert_checkpoint_continuation(&RecoveryPlan::from_journal(&journal).unwrap());
    }

    #[test]
    fn correlated_durable_success_suffix_reaches_typed_recovery() {
        let (_dir, journal, request_digest) = journal_with_recovery_checkpoint();
        append_correlated_attempt(&journal, "attempt-1", "dispatch-1", &request_digest);
        journal
            .append(SessionEvent::ProviderAttemptStarted {
                attempt_id: "attempt-1".into(),
            })
            .unwrap();
        journal
            .append(SessionEvent::StreamStarted {
                stream_id: "stream-1".into(),
                attempt_id: "attempt-1".into(),
            })
            .unwrap();
        let events = vec![ProviderStreamEvent::Done {
            stop_reason: serde_json::json!("end_turn"),
            finish_reason: serde_json::to_value(FinishReason::Stop).unwrap(),
            usage: serde_json::json!({
                "input_tokens": 8,
                "output_tokens": 3,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0
            }),
        }];
        journal
            .append(SessionEvent::StreamBatchCommitted {
                stream_id: "stream-1".into(),
                ordinal: 0,
                events: events.clone(),
            })
            .unwrap();
        journal
            .append(SessionEvent::StreamFinished {
                stream_id: "stream-1".into(),
            })
            .unwrap();
        journal
            .append(SessionEvent::ProviderAttemptFinishedV2 {
                attempt_id: "attempt-1".into(),
                dispatch_id: "dispatch-1".into(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some(
                    crate::provider_recovery::provider_response_digest(&events).unwrap(),
                ),
            })
            .unwrap();

        assert_checkpoint_continuation(&RecoveryPlan::from_journal(&journal).unwrap());
    }

    #[test]
    fn mismatched_provider_authority_suffix_fails_closed() {
        let (_dir, wrong_digest, _request_digest) = journal_with_recovery_checkpoint();
        append_correlated_attempt(&wrong_digest, "attempt-1", "dispatch-1", &"d".repeat(64));
        assert!(matches!(
            RecoveryPlan::from_journal(&wrong_digest)
                .unwrap()
                .disposition,
            RecoveryDisposition::Blocked { .. }
        ));

        let (_dir, wrong_purpose, request_digest) = journal_with_recovery_checkpoint();
        wrong_purpose
            .append(SessionEvent::ProviderAttemptPreparedV2 {
                attempt_id: "attempt-1".into(),
                dispatch_id: "dispatch-1".into(),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Compaction,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest,
            })
            .unwrap();
        assert!(matches!(
            RecoveryPlan::from_journal(&wrong_purpose)
                .unwrap()
                .disposition,
            RecoveryDisposition::Blocked { .. }
        ));
    }

    #[test]
    fn legacy_or_foreign_provider_suffix_fails_closed() {
        let (_dir, legacy, request_digest) = journal_with_recovery_checkpoint();
        legacy
            .append(SessionEvent::ProviderAttemptPrepared {
                attempt_id: "attempt-legacy".into(),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: request_digest.clone(),
            })
            .unwrap();
        assert!(matches!(
            RecoveryPlan::from_journal(&legacy).unwrap().disposition,
            RecoveryDisposition::Blocked { .. }
        ));

        let (_dir, foreign, request_digest) = journal_with_recovery_checkpoint();
        append_correlated_attempt(&foreign, "attempt-1", "dispatch-1", &request_digest);
        append_correlated_attempt(&foreign, "attempt-2", "dispatch-2", &request_digest);
        assert!(matches!(
            RecoveryPlan::from_journal(&foreign).unwrap().disposition,
            RecoveryDisposition::Blocked { .. }
        ));
    }

    #[test]
    fn unrelated_unknown_before_checkpoint_remains_blocked() {
        let (dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let message = user_message("finish the task");
        journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 0,
                message: message.clone(),
                message_digest: state_payload_digest(&message).unwrap(),
            })
            .unwrap();
        let request = wcore_types::llm::LlmRequest {
            conversation_id: Some("conversation-1".into()),
            messages: vec![serde_json::from_value(message.clone()).unwrap()],
            ..Default::default()
        };
        let request_digest = crate::session_journal::provider_request_digest(&request).unwrap();
        append_correlated_attempt(
            &journal,
            "attempt-before",
            "dispatch-before",
            &request_digest,
        );
        journal
            .append(SessionEvent::ProviderAttemptStarted {
                attempt_id: "attempt-before".into(),
            })
            .unwrap();
        append_recovery_checkpoint(&journal, "turn-1", &[message], None, None);

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::Blocked {
                reason: RecoveryBlocker::ProviderOutcomeUnknown,
                ..
            }
        ));
        drop(dir);
    }

    #[test]
    fn unrelated_effect_or_checkpoint_suffix_fails_closed() {
        let (_dir, conversation, _request_digest) = journal_with_recovery_checkpoint();
        let extra = user_message("unrelated mutation");
        conversation
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 1,
                message: extra.clone(),
                message_digest: state_payload_digest(&extra).unwrap(),
            })
            .unwrap();
        assert!(matches!(
            RecoveryPlan::from_journal(&conversation)
                .unwrap()
                .disposition,
            RecoveryDisposition::Blocked { .. }
        ));

        let (_dir, child, _request_digest) = journal_with_recovery_checkpoint();
        child
            .append(SessionEvent::ChildPrepared {
                child_id: "child-1".into(),
                turn_id: "turn-1".into(),
                request: serde_json::json!({"task": "unrelated"}),
            })
            .unwrap();
        assert!(matches!(
            RecoveryPlan::from_journal(&child).unwrap().disposition,
            RecoveryDisposition::Blocked { .. }
        ));

        let (_dir, delivery, _request_digest) = journal_with_recovery_checkpoint();
        delivery
            .append(SessionEvent::DeliveryPrepared {
                delivery_id: "delivery-1".into(),
                origin: DeliveryOrigin::Turn {
                    turn_id: "turn-1".into(),
                },
                destination: "fixture".into(),
                payload: serde_json::json!({"body": "unrelated"}),
            })
            .unwrap();
        assert!(matches!(
            RecoveryPlan::from_journal(&delivery).unwrap().disposition,
            RecoveryDisposition::Blocked { .. }
        ));

        let (_dir, checkpoint, _request_digest) = journal_with_recovery_checkpoint();
        let state = serde_json::json!({"unrelated": true});
        checkpoint
            .append(SessionEvent::CheckpointCommitted {
                checkpoint_id: "checkpoint-other".into(),
                purpose: CheckpointPurpose::UserRequested,
                origin: CheckpointOrigin::Turn {
                    turn_id: "turn-1".into(),
                },
                state_digest: state_payload_digest(&state).unwrap(),
                state,
            })
            .unwrap();
        assert!(matches!(
            RecoveryPlan::from_journal(&checkpoint).unwrap().disposition,
            RecoveryDisposition::Blocked { .. }
        ));
    }

    #[test]
    fn stale_or_mismatched_recovery_checkpoint_fails_closed() {
        for corrupt_digest in [false, true] {
            let (_dir, journal) = journal();
            journal
                .append(SessionEvent::TurnStarted {
                    turn_id: "turn-1".into(),
                    user_message: "finish the task".into(),
                })
                .unwrap();
            let message = user_message("finish the task");
            journal
                .append(SessionEvent::ConversationMessageCommitted {
                    turn_id: "turn-1".into(),
                    message_index: 0,
                    message: message.clone(),
                    message_digest: state_payload_digest(&message).unwrap(),
                })
                .unwrap();
            append_recovery_checkpoint(
                &journal,
                "turn-1",
                std::slice::from_ref(&message),
                corrupt_digest.then(|| "d".repeat(64)),
                None,
            );
            if !corrupt_digest {
                journal
                    .append(SessionEvent::ProviderAttemptPrepared {
                        attempt_id: "attempt-after-checkpoint".into(),
                        turn_id: "turn-1".into(),
                        purpose: ProviderAttemptPurpose::Conversation,
                        provider: "fixture".into(),
                        model: "fixture-model".into(),
                        request_digest: "request-digest".into(),
                    })
                    .unwrap();
            }

            let plan = RecoveryPlan::from_journal(&journal).unwrap();
            assert!(matches!(
                plan.disposition,
                RecoveryDisposition::Blocked {
                    turn_id,
                    reason: RecoveryBlocker::ContextCheckpointMissing,
                } if turn_id == "turn-1"
            ));
        }
    }

    #[test]
    fn opened_prepared_request_with_changed_durable_content_fails_closed() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        let message = user_message("finish the task");
        journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 0,
                message: message.clone(),
                message_digest: state_payload_digest(&message).unwrap(),
            })
            .unwrap();
        append_recovery_checkpoint(
            &journal,
            "turn-1",
            std::slice::from_ref(&message),
            None,
            None,
        );
        let checkpoint = RecoveryCheckpoint::from_value(
            &journal.state().unwrap().checkpoints["checkpoint-1"].state,
        )
        .unwrap();
        let changed_request = wcore_types::llm::LlmRequest {
            conversation_id: Some("conversation-1".into()),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "run a different task".into(),
                }],
            )],
            ..Default::default()
        };
        let changed_snapshot =
            crate::session_journal::prepared_provider_request_snapshot(&changed_request).unwrap();
        assert!(
            checkpoint
                .validate_opened_prepared_request_conversation(
                    &changed_snapshot,
                    std::slice::from_ref(&message),
                )
                .is_err()
        );
    }

    #[test]
    fn started_provider_is_never_automatically_reissued() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        journal
            .append(SessionEvent::ProviderAttemptPrepared {
                attempt_id: "attempt-1".into(),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: "request-digest".into(),
            })
            .unwrap();
        journal
            .append(SessionEvent::ProviderAttemptStarted {
                attempt_id: "attempt-1".into(),
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::Blocked {
                turn_id,
                reason: RecoveryBlocker::ProviderOutcomeUnknown,
            } if turn_id == "turn-1"
        ));
    }

    #[test]
    fn unknown_tool_requires_reconciliation_before_continue() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "write it".into(),
            })
            .unwrap();
        let scope = crate::journal_effects::JournalEffectCoordinator::new(journal.clone())
            .for_turn("turn-1");
        let prepared = scope
            .prepare_tool_with_contract(
                "provider-call-1",
                0,
                "OpaqueTool",
                serde_json::json!({"value": 1}),
                serde_json::json!({"value": 1}),
                wcore_types::tool::ToolEffectContract::default(),
            )
            .unwrap();
        let started = prepared.start().unwrap();
        let tool_execution_id = started.id().to_owned();
        let _ = started
            .unknown(
                ToolUnknownReason::Interrupted,
                serde_json::json!({"recovery": "test"}),
            )
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::ReconciliationRequired {
                turn_id,
                tool_execution_ids,
            } if turn_id == "turn-1" && tool_execution_ids == vec![tool_execution_id]
        ));
    }

    #[test]
    fn prepared_provider_requires_a_checkpoint_instead_of_guessing() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "finish the task".into(),
            })
            .unwrap();
        journal
            .append(SessionEvent::ProviderAttemptPrepared {
                attempt_id: "attempt-1".into(),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: "request-digest".into(),
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        assert!(matches!(
            plan.disposition,
            RecoveryDisposition::Blocked {
                turn_id,
                reason: RecoveryBlocker::ContextCheckpointMissing,
            } if turn_id == "turn-1"
        ));
    }

    #[test]
    fn host_projection_contains_no_recovered_prompt_or_effect_payload() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "secret prompt contents".into(),
            })
            .unwrap();

        let plan = RecoveryPlan::from_journal(&journal).unwrap();
        let (lifecycle, pending_turn) = plan.protocol_projection();
        assert_eq!(lifecycle, RecoveryLifecycle::Suspended);
        assert_eq!(pending_turn.unwrap().turn_id, "turn-1");
        let debug = format!("{:?}", plan.protocol_projection());
        assert!(!debug.contains("secret prompt contents"));
    }

    #[test]
    fn recovery_cursor_binds_sequence_and_digest() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "resume".into(),
            })
            .unwrap();
        let plan = RecoveryPlan::from_journal(&journal).unwrap();

        assert_eq!(
            plan.cursor(),
            RecoveryCursor {
                journal_sequence: plan.journal_sequence,
                journal_digest: plan.journal_digest,
            }
        );
    }

    #[test]
    fn snapshot_at_cursor_is_a_truthful_baseline_for_non_empty_replay() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "secret prompt".into(),
            })
            .unwrap();
        let message = serde_json::json!({"role": "user", "content": "secret prompt"});
        let message_digest = state_payload_digest(&message).unwrap();
        let committed = journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "turn-1".into(),
                message_index: 0,
                message,
                message_digest,
            })
            .unwrap();
        let baseline_cursor = RecoveryCursor {
            journal_sequence: Some(committed.seq),
            journal_digest: committed.checksum,
        };
        journal.compact().unwrap();
        journal
            .append(SessionEvent::TurnCancelled {
                turn_id: "turn-1".into(),
            })
            .unwrap();

        let snapshot = RecoveryPlan::from_journal_at(&journal, &baseline_cursor).unwrap();
        let head = RecoveryPlan::from_journal(&journal).unwrap();
        let replay = RecoveryPlan::replay_after(&journal, &baseline_cursor).unwrap();

        assert_eq!(snapshot.cursor(), baseline_cursor);
        assert_ne!(snapshot.state_digest, head.state_digest);
        assert_eq!(
            snapshot.protocol_projection(),
            (
                RecoveryLifecycle::Suspended,
                Some(RecoveryTurnSnapshot {
                    turn_id: "turn-1".into(),
                    msg_id: None,
                    lifecycle: RecoveryLifecycle::Suspended,
                    pending_call_id: None,
                    reconcile_reason: Some(RecoveryReconcileReason::ContextUnrestorable),
                })
            )
        );
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].kind, RecoveryReplayKind::TurnCancelled);
        assert!(
            replay[0].cursor.journal_sequence > snapshot.cursor().journal_sequence,
            "replay must advance strictly beyond its snapshot baseline"
        );
    }

    #[test]
    fn baseline_snapshot_with_full_seq_zero_log_accepts_a_genesis_cursor() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "resume".into(),
            })
            .unwrap();
        journal.compact().unwrap();
        let genesis = RecoveryCursor {
            journal_sequence: None,
            journal_digest: crate::session_journal::GENESIS_CHECKSUM.into(),
        };

        let baseline = RecoveryPlan::from_journal_at(&journal, &genesis).unwrap();
        let replay = RecoveryPlan::replay_after(&journal, &genesis).unwrap();

        assert_eq!(baseline.cursor(), genesis);
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].cursor.journal_sequence, Some(0));
    }

    #[test]
    fn compacted_non_genesis_prefix_rejects_a_genesis_cursor() {
        let (_dir, journal) = journal();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "resume".into(),
            })
            .unwrap();
        journal
            .append(SessionEvent::TurnCancelled {
                turn_id: "turn-1".into(),
            })
            .unwrap();
        journal.compact().unwrap();
        let genesis = RecoveryCursor {
            journal_sequence: None,
            journal_digest: crate::session_journal::GENESIS_CHECKSUM.into(),
        };

        assert!(matches!(
            RecoveryPlan::from_journal_at(&journal, &genesis),
            Err(RecoveryUnavailableReason::HistoryGap)
        ));
        assert!(matches!(
            RecoveryPlan::replay_after(&journal, &genesis),
            Err(RecoveryUnavailableReason::HistoryGap)
        ));
    }

    #[test]
    fn snapshot_at_cursor_keeps_budget_at_the_same_historical_boundary() {
        let (_dir, journal) = journal();
        let session = serde_json::json!({
            "id": "session",
            "schema_version": 1,
            "messages": [],
        });
        journal
            .append(SessionEvent::SessionImported {
                source_schema_version: 1,
                session_digest: state_payload_digest(&session).unwrap(),
                session,
            })
            .unwrap();
        let baseline = append_budget_authority(&journal, 1, 100, 1.25);
        let baseline_cursor = RecoveryCursor {
            journal_sequence: Some(baseline.seq),
            journal_digest: baseline.checksum,
        };
        append_budget_authority(&journal, 2, 275, 2.75);

        let snapshot = RecoveryPlan::from_journal_at(&journal, &baseline_cursor).unwrap();
        let head = RecoveryPlan::from_journal(&journal).unwrap();
        let replay = RecoveryPlan::replay_after(&journal, &baseline_cursor).unwrap();

        assert_eq!(snapshot.budget.tokens_used, 100);
        assert_eq!(snapshot.budget.cost_used_usd, 1.25);
        assert_eq!(head.budget.tokens_used, 275);
        assert_eq!(head.budget.cost_used_usd, 2.75);
        assert_ne!(snapshot.budget, head.budget);
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].kind, RecoveryReplayKind::StateAdvanced);
    }

    #[test]
    fn snapshot_at_cursor_rejects_stale_and_ahead_authority() {
        let (_dir, journal) = journal();
        let entry = journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "resume".into(),
            })
            .unwrap();

        assert!(matches!(
            RecoveryPlan::from_journal_at(
                &journal,
                &RecoveryCursor {
                    journal_sequence: Some(entry.seq),
                    journal_digest: "wrong".into(),
                },
            ),
            Err(RecoveryUnavailableReason::CursorDigestMismatch)
        ));
        assert!(matches!(
            RecoveryPlan::from_journal_at(
                &journal,
                &RecoveryCursor {
                    journal_sequence: Some(entry.seq + 1),
                    journal_digest: entry.checksum,
                },
            ),
            Err(RecoveryUnavailableReason::CursorAhead)
        ));
    }

    #[test]
    fn replay_is_ordered_content_free_and_duplicate_safe() {
        let (_dir, journal) = journal();
        let started = journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "secret prompt".into(),
            })
            .unwrap();
        let terminal = journal
            .append(SessionEvent::TurnCancelled {
                turn_id: "turn-1".into(),
            })
            .unwrap();

        let replay = RecoveryPlan::replay_after(
            &journal,
            &RecoveryCursor {
                journal_sequence: None,
                journal_digest: crate::session_journal::GENESIS_CHECKSUM.into(),
            },
        )
        .unwrap();
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].kind, RecoveryReplayKind::TurnStarted);
        assert_eq!(replay[0].cursor.journal_sequence, Some(started.seq));
        assert_eq!(replay[1].kind, RecoveryReplayKind::TurnCancelled);
        assert_eq!(replay[1].cursor.journal_sequence, Some(terminal.seq));
        assert!(!format!("{replay:?}").contains("secret prompt"));

        let duplicate = RecoveryPlan::replay_after(
            &journal,
            &RecoveryCursor {
                journal_sequence: Some(terminal.seq),
                journal_digest: terminal.checksum,
            },
        )
        .unwrap();
        assert!(duplicate.is_empty());
    }

    #[test]
    fn replay_keeps_journal_sequences_contiguous_for_private_transitions() {
        let (_dir, journal) = journal();
        let started = journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "secret prompt".into(),
            })
            .unwrap();
        let prepared = journal
            .append(SessionEvent::ProviderAttemptPrepared {
                attempt_id: "attempt-1".into(),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "secret-provider".into(),
                model: "secret-model".into(),
                request_digest: "secret-request-digest".into(),
            })
            .unwrap();

        let replay = RecoveryPlan::replay_after(
            &journal,
            &RecoveryCursor {
                journal_sequence: None,
                journal_digest: crate::session_journal::GENESIS_CHECKSUM.into(),
            },
        )
        .unwrap();

        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].cursor.journal_sequence, Some(started.seq));
        assert_eq!(replay[0].kind, RecoveryReplayKind::TurnStarted);
        assert_eq!(replay[1].cursor.journal_sequence, Some(prepared.seq));
        assert_eq!(replay[1].kind, RecoveryReplayKind::StateAdvanced);
        let serialized = serde_json::to_string(&replay).unwrap();
        assert!(!serialized.contains("secret prompt"));
        assert!(!serialized.contains("secret-provider"));
        assert!(!serialized.contains("secret-model"));
        assert!(!serialized.contains("secret-request-digest"));
    }

    #[test]
    fn replay_rejects_stale_digest_and_ahead_cursor() {
        let (_dir, journal) = journal();
        let entry = journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "resume".into(),
            })
            .unwrap();

        assert_eq!(
            RecoveryPlan::replay_after(
                &journal,
                &RecoveryCursor {
                    journal_sequence: Some(entry.seq),
                    journal_digest: "wrong".into(),
                },
            ),
            Err(RecoveryUnavailableReason::CursorDigestMismatch)
        );
        assert_eq!(
            RecoveryPlan::replay_after(
                &journal,
                &RecoveryCursor {
                    journal_sequence: Some(entry.seq + 1),
                    journal_digest: entry.checksum,
                },
            ),
            Err(RecoveryUnavailableReason::CursorAhead)
        );
    }
}
