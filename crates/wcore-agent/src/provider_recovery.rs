//! Pure validation and reconstruction of crash-durable provider responses.
//!
//! This module never calls a provider and never mutates engine state. It turns
//! only a V2 dispatch-correlated, reducer-accepted response into the typed
//! values that Stage B can apply idempotently.

use std::collections::BTreeSet;

use serde::Deserialize;
use thiserror::Error;
use wcore_types::llm::FluxSearchResult;
use wcore_types::message::{ContentBlock, FinishReason, StopReason, TokenUsage};

use crate::session_journal::{
    CompletionOutcome, ExternalEffectState, JournalError, ProviderAttemptPurpose,
    ProviderStreamEvent, ReducedSessionState, state_payload_digest,
};

#[derive(Debug, Error)]
pub enum ProviderRecoveryError {
    #[error("unknown provider attempt {0}")]
    AttemptMissing(String),
    #[error("provider attempt {0} uses the legacy uncorrelated journal shape")]
    LegacyAttempt(String),
    #[error("provider attempt {attempt_id} belongs to dispatch {actual}, not {expected}")]
    DispatchMismatch {
        attempt_id: String,
        expected: String,
        actual: String,
    },
    #[error("provider attempt {0} does not have an authoritative successful outcome")]
    AttemptNotSuccessful(String),
    #[error("provider attempt {0} does not have exactly one stream")]
    StreamCardinality(String),
    #[error("provider stream is invalid: {0}")]
    InvalidStream(String),
    #[error("provider response digest is missing or does not match the durable stream")]
    DigestMismatch,
    #[error("provider response field {field} is invalid: {detail}")]
    Decode { field: &'static str, detail: String },
    #[error("provider attempt {attempt_id} belongs to turn {actual}, not recovery turn {expected}")]
    TurnMismatch {
        attempt_id: String,
        expected: String,
        actual: String,
    },
    #[error(
        "provider attempt {attempt_id} purpose {actual:?} does not match recovery purpose {expected:?}"
    )]
    PurposeMismatch {
        attempt_id: String,
        expected: ProviderAttemptPurpose,
        actual: ProviderAttemptPurpose,
    },
    #[error(
        "provider attempt {attempt_id} request digest {actual} does not match recovery digest {expected}"
    )]
    RequestDigestMismatch {
        attempt_id: String,
        expected: String,
        actual: String,
    },
    #[error("provider dispatch {dispatch_id} has multiple successful attempts: {attempt_ids:?}")]
    AmbiguousSuccess {
        dispatch_id: String,
        attempt_ids: Vec<String>,
    },
    #[error(
        "provider dispatch {dispatch_id} has durable success {successful_attempt_id} alongside unknown attempts {unknown_attempt_ids:?}"
    )]
    SuccessWithUnknownAttempts {
        dispatch_id: String,
        successful_attempt_id: String,
        unknown_attempt_ids: Vec<String>,
    },
    #[error(
        "provider dispatch {dispatch_id} has both durable failures {failed_attempt_ids:?} and cancellations {cancelled_attempt_ids:?}"
    )]
    MixedTerminalOutcomes {
        dispatch_id: String,
        failed_attempt_ids: Vec<String>,
        cancelled_attempt_ids: Vec<String>,
    },
    #[error(transparent)]
    Journal(#[from] JournalError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredProviderFailure {
    pub attempt_id: String,
    pub error: String,
}

/// Fail-closed disposition for every V2 physical attempt bound to one logical
/// provider dispatch. Only `SafeNoSend` permits issuing the request, while
/// `ApplyDurableSuccess` reuses already-durable output without another call.
#[derive(Debug, Clone)]
pub enum ProviderDispatchRecoveryDisposition {
    SafeNoSend {
        dispatch_id: String,
        turn_id: String,
        request_digest: String,
        attempt_ids: Vec<String>,
    },
    StartedUnknown {
        dispatch_id: String,
        turn_id: String,
        request_digest: String,
        unknown_attempt_ids: Vec<String>,
    },
    ApplyDurableSuccess {
        round: Box<RecoveredProviderRound>,
        other_attempt_ids: Vec<String>,
    },
    DurableFailure {
        dispatch_id: String,
        turn_id: String,
        request_digest: String,
        failures: Vec<RecoveredProviderFailure>,
    },
    DurableCancelled {
        dispatch_id: String,
        turn_id: String,
        request_digest: String,
        cancelled_attempt_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecoveredProviderMetadata {
    pub routed_model: Option<String>,
    pub model_window: Option<u64>,
    pub context_pressure: Option<f32>,
    pub tokens_counted: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RecoveredProviderRound {
    pub dispatch_id: String,
    pub attempt_id: String,
    pub stream_id: String,
    pub turn_id: String,
    pub provider: String,
    pub model: String,
    pub request_digest: String,
    pub response_digest: String,
    pub assistant_text: String,
    pub thinking_text: String,
    pub tool_calls: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
    pub citations: Vec<String>,
    pub search_results: Vec<FluxSearchResult>,
    pub provider_metadata: RecoveredProviderMetadata,
}

/// Project the complete physical-attempt set for one logical provider
/// dispatch without calling a provider or mutating engine state.
///
/// Dispatch, turn, purpose and request identity must all match. A single
/// durable V2 success is authoritative only when no other matching attempt
/// remains started-unknown. Multiple successes and mixed failure/cancellation
/// terminal sets are rejected because the reduced state has no ordering
/// authority with which to choose one.
pub fn plan_provider_dispatch_recovery(
    state: &ReducedSessionState,
    dispatch_id: &str,
    turn_id: &str,
    expected_purpose: ProviderAttemptPurpose,
    request_digest: &str,
) -> Result<ProviderDispatchRecoveryDisposition, ProviderRecoveryError> {
    let attempts = state
        .provider_attempts
        .iter()
        .filter(|(_, attempt)| attempt.dispatch_id.as_deref() == Some(dispatch_id))
        .collect::<Vec<_>>();
    if attempts.is_empty() {
        return Ok(ProviderDispatchRecoveryDisposition::SafeNoSend {
            dispatch_id: dispatch_id.to_owned(),
            turn_id: turn_id.to_owned(),
            request_digest: request_digest.to_owned(),
            attempt_ids: Vec::new(),
        });
    }

    let mut safe_no_send = Vec::new();
    let mut unknown = Vec::new();
    let mut succeeded = Vec::new();
    let mut failures = Vec::new();
    let mut cancelled = Vec::new();

    for (attempt_id, attempt) in attempts {
        if attempt.turn_id != turn_id {
            return Err(ProviderRecoveryError::TurnMismatch {
                attempt_id: attempt_id.clone(),
                expected: turn_id.to_owned(),
                actual: attempt.turn_id.clone(),
            });
        }
        if attempt.purpose != expected_purpose {
            return Err(ProviderRecoveryError::PurposeMismatch {
                attempt_id: attempt_id.clone(),
                expected: expected_purpose,
                actual: attempt.purpose,
            });
        }
        if attempt.request_digest != request_digest {
            return Err(ProviderRecoveryError::RequestDigestMismatch {
                attempt_id: attempt_id.clone(),
                expected: request_digest.to_owned(),
                actual: attempt.request_digest.clone(),
            });
        }

        match &attempt.effect {
            ExternalEffectState::Prepared | ExternalEffectState::NotStarted => {
                safe_no_send.push(attempt_id.clone());
            }
            ExternalEffectState::Unknown => unknown.push(attempt_id.clone()),
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Succeeded,
            } => succeeded.push(attempt_id.clone()),
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Failed { error },
            } => failures.push(RecoveredProviderFailure {
                attempt_id: attempt_id.clone(),
                error: error.clone(),
            }),
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Cancelled,
            } => cancelled.push(attempt_id.clone()),
        }
    }

    if succeeded.len() > 1 {
        return Err(ProviderRecoveryError::AmbiguousSuccess {
            dispatch_id: dispatch_id.to_owned(),
            attempt_ids: succeeded,
        });
    }
    if let Some(successful_attempt_id) = succeeded.pop() {
        if !unknown.is_empty() {
            return Err(ProviderRecoveryError::SuccessWithUnknownAttempts {
                dispatch_id: dispatch_id.to_owned(),
                successful_attempt_id,
                unknown_attempt_ids: unknown,
            });
        }
        let round = recover_provider_round(state, dispatch_id, &successful_attempt_id)?;
        let mut other_attempt_ids = safe_no_send;
        other_attempt_ids.extend(failures.into_iter().map(|failure| failure.attempt_id));
        other_attempt_ids.extend(cancelled);
        other_attempt_ids.sort();
        return Ok(ProviderDispatchRecoveryDisposition::ApplyDurableSuccess {
            round: Box::new(round),
            other_attempt_ids,
        });
    }

    if !unknown.is_empty() {
        return Ok(ProviderDispatchRecoveryDisposition::StartedUnknown {
            dispatch_id: dispatch_id.to_owned(),
            turn_id: turn_id.to_owned(),
            request_digest: request_digest.to_owned(),
            unknown_attempt_ids: unknown,
        });
    }
    if !failures.is_empty() && !cancelled.is_empty() {
        return Err(ProviderRecoveryError::MixedTerminalOutcomes {
            dispatch_id: dispatch_id.to_owned(),
            failed_attempt_ids: failures
                .iter()
                .map(|failure| failure.attempt_id.clone())
                .collect(),
            cancelled_attempt_ids: cancelled,
        });
    }
    if !failures.is_empty() {
        return Ok(ProviderDispatchRecoveryDisposition::DurableFailure {
            dispatch_id: dispatch_id.to_owned(),
            turn_id: turn_id.to_owned(),
            request_digest: request_digest.to_owned(),
            failures,
        });
    }
    if !cancelled.is_empty() {
        return Ok(ProviderDispatchRecoveryDisposition::DurableCancelled {
            dispatch_id: dispatch_id.to_owned(),
            turn_id: turn_id.to_owned(),
            request_digest: request_digest.to_owned(),
            cancelled_attempt_ids: cancelled,
        });
    }

    Ok(ProviderDispatchRecoveryDisposition::SafeNoSend {
        dispatch_id: dispatch_id.to_owned(),
        turn_id: turn_id.to_owned(),
        request_digest: request_digest.to_owned(),
        attempt_ids: safe_no_send,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictTokenUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictProviderMetadata {
    routed_model: Option<String>,
    model_window: Option<u64>,
    context_pressure: Option<f32>,
    tokens_counted: Option<u64>,
}

/// Compute the canonical digest used by both the live journal writer and
/// recovery validation.
pub fn provider_response_digest(events: &[ProviderStreamEvent]) -> Result<String, JournalError> {
    let value = serde_json::to_value(events).map_err(|source| JournalError::Json {
        context: "encoding provider response digest",
        source,
    })?;
    state_payload_digest(&value)
}

pub(crate) fn validate_finished_provider_events(
    events: &[ProviderStreamEvent],
) -> Result<(), JournalError> {
    let done_positions = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event, ProviderStreamEvent::Done { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    if done_positions != [events.len().saturating_sub(1)] {
        return Err(JournalError::InvalidTransition(
            "recovery-correlated provider stream must contain exactly one final Done event"
                .to_owned(),
        ));
    }
    if events
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::Error { .. }))
    {
        return Err(JournalError::InvalidTransition(
            "successful recovery-correlated provider stream contains an Error event".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_appended_provider_events(
    existing: &[ProviderStreamEvent],
    appended: &[ProviderStreamEvent],
) -> Result<(), JournalError> {
    if existing.iter().any(is_terminal_event) {
        return Err(JournalError::InvalidTransition(
            "recovery-correlated provider stream already contains a terminal event".to_owned(),
        ));
    }
    let terminal_positions = appended
        .iter()
        .enumerate()
        .filter_map(|(index, event)| is_terminal_event(event).then_some(index))
        .collect::<Vec<_>>();
    if terminal_positions.len() > 1
        || terminal_positions
            .first()
            .is_some_and(|position| *position + 1 != appended.len())
    {
        return Err(JournalError::InvalidTransition(
            "recovery-correlated provider batch has multiple or non-final terminal events"
                .to_owned(),
        ));
    }
    Ok(())
}

fn is_terminal_event(event: &ProviderStreamEvent) -> bool {
    matches!(
        event,
        ProviderStreamEvent::Done { .. } | ProviderStreamEvent::Error { .. }
    )
}

/// Validate and fold one successful, dispatch-correlated provider response.
/// Legacy attempts deliberately return [`ProviderRecoveryError::LegacyAttempt`].
pub fn recover_provider_round(
    state: &ReducedSessionState,
    dispatch_id: &str,
    attempt_id: &str,
) -> Result<RecoveredProviderRound, ProviderRecoveryError> {
    let attempt = state
        .provider_attempts
        .get(attempt_id)
        .ok_or_else(|| ProviderRecoveryError::AttemptMissing(attempt_id.to_owned()))?;
    let Some(actual_dispatch_id) = attempt.dispatch_id.as_deref() else {
        return Err(ProviderRecoveryError::LegacyAttempt(attempt_id.to_owned()));
    };
    if actual_dispatch_id != dispatch_id {
        return Err(ProviderRecoveryError::DispatchMismatch {
            attempt_id: attempt_id.to_owned(),
            expected: dispatch_id.to_owned(),
            actual: actual_dispatch_id.to_owned(),
        });
    }
    if !matches!(
        &attempt.effect,
        ExternalEffectState::Completed {
            outcome: CompletionOutcome::Succeeded
        }
    ) {
        return Err(ProviderRecoveryError::AttemptNotSuccessful(
            attempt_id.to_owned(),
        ));
    }

    let streams = state
        .streams
        .iter()
        .filter(|(_, stream)| stream.attempt_id == attempt_id)
        .collect::<Vec<_>>();
    let [(stream_id, stream)] = streams.as_slice() else {
        return Err(ProviderRecoveryError::StreamCardinality(
            attempt_id.to_owned(),
        ));
    };
    if !stream.finished {
        return Err(ProviderRecoveryError::InvalidStream(
            "stream is not finished".to_owned(),
        ));
    }
    let events = stream.batches.iter().flatten().cloned().collect::<Vec<_>>();
    validate_finished_provider_events(&events)
        .map_err(|error| ProviderRecoveryError::InvalidStream(error.to_string()))?;
    let response_digest = provider_response_digest(&events)?;
    if attempt.response_digest.as_deref() != Some(response_digest.as_str()) {
        return Err(ProviderRecoveryError::DigestMismatch);
    }

    let mut assistant_text = String::new();
    let mut thinking_text = String::new();
    let mut tool_calls = Vec::new();
    let mut tool_call_ids = BTreeSet::new();
    let mut citations = Vec::new();
    let mut search_results = Vec::new();
    let mut provider_metadata = RecoveredProviderMetadata::default();
    let mut done = None;

    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { text } => assistant_text.push_str(&text),
            ProviderStreamEvent::ToolUse {
                id,
                name,
                input,
                extra,
            } => {
                if id.trim().is_empty()
                    || name.trim().is_empty()
                    || !tool_call_ids.insert(id.clone())
                {
                    return Err(ProviderRecoveryError::InvalidStream(
                        "tool calls require unique non-empty ids and non-empty names".to_owned(),
                    ));
                }
                tool_calls.push(ContentBlock::ToolUse {
                    id,
                    name,
                    input,
                    extra,
                });
            }
            ProviderStreamEvent::ThinkingDelta { text } => thinking_text.push_str(&text),
            ProviderStreamEvent::ThinkingSubject { .. } => {}
            ProviderStreamEvent::Done {
                stop_reason,
                finish_reason,
                usage,
            } => {
                done = Some(decode_done(stop_reason, finish_reason, usage)?);
            }
            ProviderStreamEvent::Error { .. } => {
                return Err(ProviderRecoveryError::InvalidStream(
                    "successful response contains an Error event".to_owned(),
                ));
            }
            ProviderStreamEvent::Citations { urls } => citations = urls,
            ProviderStreamEvent::SearchResults { results } => {
                search_results = results
                    .into_iter()
                    .map(|result| {
                        serde_json::from_value(result).map_err(|error| {
                            ProviderRecoveryError::Decode {
                                field: "search_results",
                                detail: error.to_string(),
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
            ProviderStreamEvent::ProviderMeta { metadata } => {
                let decoded: StrictProviderMetadata =
                    serde_json::from_value(metadata).map_err(|error| {
                        ProviderRecoveryError::Decode {
                            field: "provider_metadata",
                            detail: error.to_string(),
                        }
                    })?;
                if decoded
                    .context_pressure
                    .is_some_and(|value| !(0.0..=1.0).contains(&value))
                {
                    return Err(ProviderRecoveryError::Decode {
                        field: "provider_metadata.context_pressure",
                        detail: "value must be finite and within 0.0..=1.0".to_owned(),
                    });
                }
                provider_metadata = RecoveredProviderMetadata {
                    routed_model: decoded.routed_model,
                    model_window: decoded.model_window,
                    context_pressure: decoded.context_pressure,
                    tokens_counted: decoded.tokens_counted,
                };
            }
        }
    }

    let (stop_reason, finish_reason, usage) = done.ok_or_else(|| {
        ProviderRecoveryError::InvalidStream("finished stream has no Done event".to_owned())
    })?;
    Ok(RecoveredProviderRound {
        dispatch_id: dispatch_id.to_owned(),
        attempt_id: attempt_id.to_owned(),
        stream_id: (*stream_id).clone(),
        turn_id: attempt.turn_id.clone(),
        provider: attempt.provider.clone(),
        model: attempt.model.clone(),
        request_digest: attempt.request_digest.clone(),
        response_digest,
        assistant_text,
        thinking_text,
        tool_calls,
        stop_reason,
        finish_reason,
        usage,
        citations,
        search_results,
        provider_metadata,
    })
}

fn decode_done(
    stop_reason: serde_json::Value,
    finish_reason: serde_json::Value,
    usage: serde_json::Value,
) -> Result<(StopReason, FinishReason, TokenUsage), ProviderRecoveryError> {
    let stop_reason = match stop_reason.as_str() {
        Some("end_turn") => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("max_turns") => {
            return Err(ProviderRecoveryError::Decode {
                field: "stop_reason",
                detail: "max_turns is engine-generated and cannot come from a provider".to_owned(),
            });
        }
        _ => {
            return Err(ProviderRecoveryError::Decode {
                field: "stop_reason",
                detail: "unknown provider stop reason".to_owned(),
            });
        }
    };
    let finish_reason: FinishReason =
        serde_json::from_value(finish_reason).map_err(|error| ProviderRecoveryError::Decode {
            field: "finish_reason",
            detail: error.to_string(),
        })?;
    if finish_reason != FinishReason::from_stop_reason(stop_reason) {
        return Err(ProviderRecoveryError::Decode {
            field: "finish_reason",
            detail: "finish reason is incompatible with the provider stop reason".to_owned(),
        });
    }
    let usage: StrictTokenUsage =
        serde_json::from_value(usage).map_err(|error| ProviderRecoveryError::Decode {
            field: "usage",
            detail: error.to_string(),
        })?;
    Ok((
        stop_reason,
        finish_reason,
        TokenUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            cache_read_tokens: usage.cache_read_tokens,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_journal::{ProviderAttemptPurpose, ProviderAttemptState, StreamState};

    fn durable_events() -> Vec<ProviderStreamEvent> {
        vec![
            ProviderStreamEvent::TextDelta {
                text: "hello ".into(),
            },
            ProviderStreamEvent::TextDelta {
                text: "world".into(),
            },
            ProviderStreamEvent::ToolUse {
                id: "call-1".into(),
                name: "Read".into(),
                input: serde_json::json!({"path": "README.md"}),
                extra: None,
            },
            ProviderStreamEvent::Done {
                stop_reason: serde_json::json!("tool_use"),
                finish_reason: serde_json::to_value(FinishReason::Stop).unwrap(),
                usage: serde_json::json!({
                    "input_tokens": 8,
                    "output_tokens": 3,
                    "cache_creation_tokens": 1,
                    "cache_read_tokens": 2
                }),
            },
        ]
    }

    fn recovered_state(dispatch_id: Option<&str>) -> ReducedSessionState {
        let events = durable_events();
        let digest = provider_response_digest(&events).unwrap();
        let mut state = ReducedSessionState::default();
        state.provider_attempts.insert(
            "attempt-1".into(),
            ProviderAttemptState {
                dispatch_id: dispatch_id.map(str::to_owned),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: "request-digest".into(),
                response_digest: Some(digest),
                not_started_reason: None,
                effect: ExternalEffectState::Completed {
                    outcome: CompletionOutcome::Succeeded,
                },
            },
        );
        state.streams.insert(
            "stream-1".into(),
            StreamState {
                attempt_id: "attempt-1".into(),
                next_ordinal: 1,
                batches: vec![events],
                finished: true,
            },
        );
        state
    }

    fn insert_attempt(
        state: &mut ReducedSessionState,
        attempt_id: &str,
        effect: ExternalEffectState,
    ) {
        state.provider_attempts.insert(
            attempt_id.into(),
            ProviderAttemptState {
                dispatch_id: Some("dispatch-1".into()),
                turn_id: "turn-1".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: "request-digest".into(),
                response_digest: None,
                not_started_reason: None,
                effect,
            },
        );
    }

    fn insert_successful_attempt(
        state: &mut ReducedSessionState,
        attempt_id: &str,
        stream_id: &str,
    ) {
        let events = durable_events();
        let response_digest = provider_response_digest(&events).unwrap();
        insert_attempt(
            state,
            attempt_id,
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Succeeded,
            },
        );
        state
            .provider_attempts
            .get_mut(attempt_id)
            .unwrap()
            .response_digest = Some(response_digest);
        state.streams.insert(
            stream_id.into(),
            StreamState {
                attempt_id: attempt_id.into(),
                next_ordinal: 1,
                batches: vec![events],
                finished: true,
            },
        );
    }

    #[test]
    fn folds_exact_correlated_response_into_typed_round() {
        let recovered = recover_provider_round(
            &recovered_state(Some("dispatch-1")),
            "dispatch-1",
            "attempt-1",
        )
        .unwrap();

        assert_eq!(recovered.assistant_text, "hello world");
        assert_eq!(recovered.stop_reason, StopReason::ToolUse);
        assert_eq!(recovered.finish_reason, FinishReason::Stop);
        assert_eq!(recovered.usage.input_tokens, 8);
        assert_eq!(recovered.tool_calls.len(), 1);
        assert_eq!(recovered.dispatch_id, "dispatch-1");
    }

    #[test]
    fn rejects_legacy_attempt_even_when_its_response_is_complete() {
        assert!(matches!(
            recover_provider_round(&recovered_state(None), "dispatch-1", "attempt-1"),
            Err(ProviderRecoveryError::LegacyAttempt(attempt)) if attempt == "attempt-1"
        ));
    }

    #[test]
    fn rejects_digest_drift() {
        let mut state = recovered_state(Some("dispatch-1"));
        state
            .provider_attempts
            .get_mut("attempt-1")
            .unwrap()
            .response_digest = Some("sha256:wrong".into());

        assert!(matches!(
            recover_provider_round(&state, "dispatch-1", "attempt-1"),
            Err(ProviderRecoveryError::DigestMismatch)
        ));
    }

    #[test]
    fn dispatch_projection_marks_only_prepared_and_not_started_attempts_safe_to_send() {
        let mut state = ReducedSessionState::default();
        insert_attempt(
            &mut state,
            "attempt-prepared",
            ExternalEffectState::Prepared,
        );
        insert_attempt(
            &mut state,
            "attempt-not-started",
            ExternalEffectState::NotStarted,
        );

        let disposition = plan_provider_dispatch_recovery(
            &state,
            "dispatch-1",
            "turn-1",
            ProviderAttemptPurpose::Conversation,
            "request-digest",
        )
        .unwrap();

        assert!(matches!(
            disposition,
            ProviderDispatchRecoveryDisposition::SafeNoSend { attempt_ids, .. }
                if attempt_ids == vec!["attempt-not-started", "attempt-prepared"]
        ));
    }

    #[test]
    fn dispatch_projection_treats_checkpoint_before_first_attempt_as_safe_no_send() {
        let disposition = plan_provider_dispatch_recovery(
            &ReducedSessionState::default(),
            "dispatch-1",
            "turn-1",
            ProviderAttemptPurpose::Conversation,
            "request-digest",
        )
        .unwrap();

        assert!(matches!(
            disposition,
            ProviderDispatchRecoveryDisposition::SafeNoSend { attempt_ids, .. }
                if attempt_ids.is_empty()
        ));
    }

    #[test]
    fn dispatch_projection_never_reissues_a_started_unknown_attempt() {
        let mut state = ReducedSessionState::default();
        insert_attempt(&mut state, "attempt-unknown", ExternalEffectState::Unknown);
        insert_attempt(
            &mut state,
            "attempt-prepared",
            ExternalEffectState::Prepared,
        );

        let disposition = plan_provider_dispatch_recovery(
            &state,
            "dispatch-1",
            "turn-1",
            ProviderAttemptPurpose::Conversation,
            "request-digest",
        )
        .unwrap();

        assert!(matches!(
            disposition,
            ProviderDispatchRecoveryDisposition::StartedUnknown {
                unknown_attempt_ids,
                ..
            } if unknown_attempt_ids == vec!["attempt-unknown"]
        ));
    }

    #[test]
    fn dispatch_projection_selects_one_durable_success_without_another_call() {
        let mut state = ReducedSessionState::default();
        insert_attempt(
            &mut state,
            "attempt-failed",
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Failed {
                    error: "retryable".into(),
                },
            },
        );
        insert_successful_attempt(&mut state, "attempt-success", "stream-success");

        let disposition = plan_provider_dispatch_recovery(
            &state,
            "dispatch-1",
            "turn-1",
            ProviderAttemptPurpose::Conversation,
            "request-digest",
        )
        .unwrap();

        assert!(matches!(
            disposition,
            ProviderDispatchRecoveryDisposition::ApplyDurableSuccess {
                round,
                other_attempt_ids,
            } if round.attempt_id == "attempt-success"
                && round.assistant_text == "hello world"
                && other_attempt_ids == vec!["attempt-failed"]
        ));
    }

    #[test]
    fn dispatch_projection_distinguishes_durable_failure_and_cancel() {
        let mut failed = ReducedSessionState::default();
        insert_attempt(
            &mut failed,
            "attempt-failed",
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Failed {
                    error: "transport".into(),
                },
            },
        );
        let failure = plan_provider_dispatch_recovery(
            &failed,
            "dispatch-1",
            "turn-1",
            ProviderAttemptPurpose::Conversation,
            "request-digest",
        )
        .unwrap();
        assert!(matches!(
            failure,
            ProviderDispatchRecoveryDisposition::DurableFailure { failures, .. }
                if failures == vec![RecoveredProviderFailure {
                    attempt_id: "attempt-failed".into(),
                    error: "transport".into(),
                }]
        ));

        let mut cancelled = ReducedSessionState::default();
        insert_attempt(
            &mut cancelled,
            "attempt-cancelled",
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Cancelled,
            },
        );
        let cancellation = plan_provider_dispatch_recovery(
            &cancelled,
            "dispatch-1",
            "turn-1",
            ProviderAttemptPurpose::Conversation,
            "request-digest",
        )
        .unwrap();
        assert!(matches!(
            cancellation,
            ProviderDispatchRecoveryDisposition::DurableCancelled {
                cancelled_attempt_ids,
                ..
            } if cancelled_attempt_ids == vec!["attempt-cancelled"]
        ));
    }

    #[test]
    fn dispatch_projection_rejects_turn_purpose_and_request_binding_drift() {
        let mut wrong_turn = ReducedSessionState::default();
        insert_attempt(&mut wrong_turn, "attempt-1", ExternalEffectState::Prepared);
        wrong_turn
            .provider_attempts
            .get_mut("attempt-1")
            .unwrap()
            .turn_id = "turn-other".into();
        assert!(matches!(
            plan_provider_dispatch_recovery(
                &wrong_turn,
                "dispatch-1",
                "turn-1",
                ProviderAttemptPurpose::Conversation,
                "request-digest"
            ),
            Err(ProviderRecoveryError::TurnMismatch { attempt_id, .. })
                if attempt_id == "attempt-1"
        ));

        let mut compaction_attempt = ReducedSessionState::default();
        insert_attempt(
            &mut compaction_attempt,
            "attempt-1",
            ExternalEffectState::Prepared,
        );
        compaction_attempt
            .provider_attempts
            .get_mut("attempt-1")
            .unwrap()
            .purpose = ProviderAttemptPurpose::Compaction;
        assert!(matches!(
            plan_provider_dispatch_recovery(
                &compaction_attempt,
                "dispatch-1",
                "turn-1",
                ProviderAttemptPurpose::Conversation,
                "request-digest"
            ),
            Err(ProviderRecoveryError::PurposeMismatch {
                attempt_id,
                expected: ProviderAttemptPurpose::Conversation,
                actual: ProviderAttemptPurpose::Compaction,
            }) if attempt_id == "attempt-1"
        ));

        let mut conversation_attempt = ReducedSessionState::default();
        insert_attempt(
            &mut conversation_attempt,
            "attempt-1",
            ExternalEffectState::Prepared,
        );
        assert!(matches!(
            plan_provider_dispatch_recovery(
                &conversation_attempt,
                "dispatch-1",
                "turn-1",
                ProviderAttemptPurpose::Compaction,
                "request-digest"
            ),
            Err(ProviderRecoveryError::PurposeMismatch {
                attempt_id,
                expected: ProviderAttemptPurpose::Compaction,
                actual: ProviderAttemptPurpose::Conversation,
            }) if attempt_id == "attempt-1"
        ));

        let mut wrong_request = ReducedSessionState::default();
        insert_attempt(
            &mut wrong_request,
            "attempt-1",
            ExternalEffectState::Prepared,
        );
        wrong_request
            .provider_attempts
            .get_mut("attempt-1")
            .unwrap()
            .request_digest = "different-request".into();
        assert!(matches!(
            plan_provider_dispatch_recovery(
                &wrong_request,
                "dispatch-1",
                "turn-1",
                ProviderAttemptPurpose::Conversation,
                "request-digest"
            ),
            Err(ProviderRecoveryError::RequestDigestMismatch { attempt_id, .. })
                if attempt_id == "attempt-1"
        ));
    }

    #[test]
    fn dispatch_projection_rejects_multiple_successes_or_success_with_unknown() {
        let mut multiple = ReducedSessionState::default();
        insert_successful_attempt(&mut multiple, "attempt-1", "stream-1");
        insert_successful_attempt(&mut multiple, "attempt-2", "stream-2");
        assert!(matches!(
            plan_provider_dispatch_recovery(
                &multiple,
                "dispatch-1",
                "turn-1",
                ProviderAttemptPurpose::Conversation,
                "request-digest"
            ),
            Err(ProviderRecoveryError::AmbiguousSuccess { attempt_ids, .. })
                if attempt_ids == vec!["attempt-1", "attempt-2"]
        ));

        let mut uncertain = recovered_state(Some("dispatch-1"));
        insert_attempt(
            &mut uncertain,
            "attempt-unknown",
            ExternalEffectState::Unknown,
        );
        assert!(matches!(
            plan_provider_dispatch_recovery(
                &uncertain,
                "dispatch-1",
                "turn-1",
                ProviderAttemptPurpose::Conversation,
                "request-digest"
            ),
            Err(ProviderRecoveryError::SuccessWithUnknownAttempts {
                unknown_attempt_ids,
                ..
            }) if unknown_attempt_ids == vec!["attempt-unknown"]
        ));
    }

    #[test]
    fn dispatch_projection_rejects_unordered_mixed_terminal_outcomes() {
        let mut state = ReducedSessionState::default();
        insert_attempt(
            &mut state,
            "attempt-failed",
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Failed {
                    error: "transport".into(),
                },
            },
        );
        insert_attempt(
            &mut state,
            "attempt-cancelled",
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Cancelled,
            },
        );

        assert!(matches!(
            plan_provider_dispatch_recovery(
                &state,
                "dispatch-1",
                "turn-1",
                ProviderAttemptPurpose::Conversation,
                "request-digest"
            ),
            Err(ProviderRecoveryError::MixedTerminalOutcomes { .. })
        ));
    }
}
