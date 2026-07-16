use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};
use wcore_agent::session_journal::{
    ActiveTurnBudgetAuthority, ApprovalDecision, ApprovalOrigin, ApprovalResolution,
    BUDGET_AUTHORITY_SCHEMA_VERSION, BudgetAmount, BudgetAuthorityCursor, BudgetAuthorityState,
    BudgetOwner, BudgetPurpose, BudgetUnit, BudgetWallClockAuthority, CheckpointOrigin,
    CheckpointPurpose, ChildNotStartedReason, CompletionOutcome, DeliveryCompletion,
    DeliveryEvidence, DeliveryNotStartedReason, DeliveryOrigin, DeliveryStage,
    DeliveryUnknownReason, ExternalEffectState, GENESIS_CHECKSUM, JournalEnvelope, JournalError,
    LEGACY_BUDGET_AUTHORITY_SCHEMA_VERSION, ProviderAttemptNotStartedReason,
    ProviderAttemptPurpose, ProviderStreamEvent, ReducedSessionState,
    SESSION_JOURNAL_SCHEMA_VERSION, SessionEvent, SessionJournal, SessionSnapshot, StoredToolInput,
    ToolEffectState, ToolNotStartedReason, TurnState, decode_prepared_provider_request_snapshot,
    load_snapshot, prepared_provider_request_snapshot, provider_request_digest,
    replay_from_snapshot, replay_state, state_payload_digest, verify_chain, write_snapshot,
};
use wcore_budget::{BudgetCap, BudgetTracker, ExecutionBudget};
use wcore_types::cache_tier::CacheTier;
use wcore_types::llm::{LlmRequest, RoutingHint, ThinkingConfig};
use wcore_types::message::{ContentBlock, Message, MessageCacheHint, Role};
use wcore_types::tool::{ToolDef, ToolEffectContract};

fn turn_started(turn_id: &str) -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: turn_id.into(),
        user_message: "hello".into(),
    }
}

fn turn_committed(turn_id: &str) -> SessionEvent {
    SessionEvent::TurnCommitted {
        turn_id: turn_id.into(),
        assistant_message: "done".into(),
    }
}

fn provider_prepared(attempt_id: &str, turn_id: &str) -> SessionEvent {
    SessionEvent::ProviderAttemptPrepared {
        attempt_id: attempt_id.into(),
        turn_id: turn_id.into(),
        purpose: ProviderAttemptPurpose::Conversation,
        provider: "x".into(),
        model: "m".into(),
        request_digest: "r".into(),
    }
}

fn tool_intent(
    tool_execution_id: &str,
    provider_call_id: &str,
    turn_id: &str,
    ordinal: u64,
    tool: &str,
    requested_input: serde_json::Value,
    effective_input: serde_json::Value,
) -> SessionEvent {
    let requested_input_digest = state_payload_digest(&requested_input).unwrap();
    let effective_input_digest = state_payload_digest(&effective_input).unwrap();
    SessionEvent::ToolIntentRecordedV2 {
        tool_execution_id: tool_execution_id.into(),
        idempotency_key: format!("fixture-key-{tool_execution_id}"),
        retry_of: None,
        provider_call_id: provider_call_id.into(),
        turn_id: turn_id.into(),
        ordinal,
        tool: tool.into(),
        requested_input: StoredToolInput::redacted(requested_input_digest.clone()),
        requested_input_digest,
        effective_input: StoredToolInput::redacted(effective_input_digest.clone()),
        effective_input_digest,
        effect_contract: ToolEffectContract::default(),
        effect_receipt: None,
        pre_hook_phase_id: None,
    }
}

fn text_batch(stream_id: &str, ordinal: u64, text: &str) -> SessionEvent {
    SessionEvent::StreamBatchCommitted {
        stream_id: stream_id.into(),
        ordinal,
        events: vec![ProviderStreamEvent::TextDelta { text: text.into() }],
    }
}

fn imported(session: serde_json::Value) -> SessionEvent {
    SessionEvent::SessionImported {
        source_schema_version: u32::try_from(session["schema_version"].as_u64().unwrap_or(0))
            .unwrap(),
        session_digest: state_payload_digest(&session).unwrap(),
        session,
    }
}

fn message_committed(
    turn_id: &str,
    message_index: u64,
    message: serde_json::Value,
) -> SessionEvent {
    SessionEvent::ConversationMessageCommitted {
        turn_id: turn_id.into(),
        message_index,
        message_digest: state_payload_digest(&message).unwrap(),
        message,
    }
}

fn conversation_state_committed(turn_id: &str, messages: Vec<serde_json::Value>) -> SessionEvent {
    let messages_digest =
        state_payload_digest(&serde_json::Value::Array(messages.clone())).unwrap();
    SessionEvent::ConversationStateCommitted {
        turn_id: turn_id.into(),
        messages,
        messages_digest,
    }
}

fn append_events(path: &Path, events: Vec<SessionEvent>) -> Vec<JournalEnvelope> {
    let journal = SessionJournal::open(path, "s1").unwrap();
    events
        .into_iter()
        .map(|event| journal.append(event).unwrap())
        .collect()
}

fn frame(body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len()).unwrap();
    let mut frame = Vec::new();
    frame.extend_from_slice(b"WJ01");
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&(!length).to_be_bytes());
    frame.extend_from_slice(body);
    frame.extend_from_slice(&Sha256::digest(body));
    frame
}

include!("session_journal_test/foundation_cases.rs");
include!("session_journal_test/effect_and_conversation_cases.rs");
include!("session_journal_test/budget_authority_cases.rs");
