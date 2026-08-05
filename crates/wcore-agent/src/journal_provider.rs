//! Durable provider-stream adapter for journal-authoritative sessions.
//!
//! Physical retry and fallback attempts are recorded by `wcore-providers` at
//! their actual send boundaries. This adapter owns the session-side lifecycle
//! sink and prevents provider-neutral stream events from becoming visible to
//! the engine before the corresponding journal batch is durable.
//!
//! The first correctness cut commits one event per batch. A bounded group
//! commit may replace that policy later, but only if it retains
//! durable-before-visible ordering and bounded streaming latency.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_providers::attempt_lifecycle::{
    PhysicalProviderAttempt, ProviderAttemptContext, ProviderAttemptHeaderOutcome,
    ProviderAttemptLifecycle, ProviderAttemptLifecycleError,
    ProviderAttemptNotStartedReason as LifecycleNotStartedReason,
    ProviderAttemptPurpose as LifecyclePurpose, scope_provider_attempt_lifecycle,
};
use wcore_providers::{LlmProvider, ModelInfo, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::StopReason;

use crate::provider_recovery::provider_response_digest;
use crate::session_journal::{
    CompletionOutcome, ProviderAttemptNotStartedReason as JournalNotStartedReason,
    ProviderAttemptPurpose, ProviderStreamEvent, SessionEvent, SessionJournal,
    provider_request_digest,
};

pub const JOURNAL_AUTHORITY_ERROR_PREFIX: &str = "wayland journal authority failure: ";

#[must_use]
pub fn is_journal_authority_error(message: &str) -> bool {
    message.contains(JOURNAL_AUTHORITY_ERROR_PREFIX)
}

/// Wraps one configured provider with the journal authority for an active turn.
///
/// The provider and model strings are fallbacks for provider implementations
/// that do not install a more precise retry/fallback identity scope.
#[derive(Clone)]
pub struct JournaledLlmProvider {
    inner: Arc<dyn LlmProvider>,
    journal: SessionJournal,
    turn_id: String,
    purpose: LifecyclePurpose,
    provider: String,
    model: String,
    dispatch_id: Option<String>,
}

impl JournaledLlmProvider {
    pub fn new(
        inner: Arc<dyn LlmProvider>,
        journal: SessionJournal,
        turn_id: impl Into<String>,
        purpose: LifecyclePurpose,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            journal,
            turn_id: turn_id.into(),
            purpose,
            provider: provider.into(),
            model: model.into(),
            dispatch_id: None,
        }
    }

    /// Bind every physical retry/fallback attempt to the logical provider
    /// dispatch authorized by a durable recovery checkpoint.
    #[must_use]
    pub fn with_dispatch_id(mut self, dispatch_id: impl Into<String>) -> Self {
        self.dispatch_id = Some(dispatch_id.into());
        self
    }
}

#[derive(Clone)]
struct JournalAttemptLifecycle {
    journal: SessionJournal,
}

impl JournalAttemptLifecycle {
    async fn append(&self, event: SessionEvent) -> Result<(), ProviderAttemptLifecycleError> {
        append_event(&self.journal, event)
            .await
            .map_err(|error| ProviderAttemptLifecycleError::new(error.to_string()))
    }
}

#[async_trait]
impl ProviderAttemptLifecycle for JournalAttemptLifecycle {
    async fn prepare(
        &self,
        attempt: &PhysicalProviderAttempt,
    ) -> Result<(), ProviderAttemptLifecycleError> {
        let event = attempt.dispatch_id.as_ref().map_or_else(
            || SessionEvent::ProviderAttemptPrepared {
                attempt_id: attempt.attempt_id.clone(),
                turn_id: attempt.turn_id.clone(),
                purpose: journal_purpose(attempt.purpose),
                provider: attempt.provider.clone(),
                model: attempt.model.clone(),
                request_digest: attempt.request_digest.clone(),
            },
            |dispatch_id| SessionEvent::ProviderAttemptPreparedV2 {
                attempt_id: attempt.attempt_id.clone(),
                dispatch_id: dispatch_id.clone(),
                turn_id: attempt.turn_id.clone(),
                purpose: journal_purpose(attempt.purpose),
                provider: attempt.provider.clone(),
                model: attempt.model.clone(),
                request_digest: attempt.request_digest.clone(),
            },
        );
        self.append(event).await
    }

    async fn started(
        &self,
        attempt: &PhysicalProviderAttempt,
    ) -> Result<(), ProviderAttemptLifecycleError> {
        self.append(SessionEvent::ProviderAttemptStarted {
            attempt_id: attempt.attempt_id.clone(),
        })
        .await
    }

    async fn finished(
        &self,
        attempt: &PhysicalProviderAttempt,
        outcome: &ProviderAttemptHeaderOutcome,
    ) -> Result<(), ProviderAttemptLifecycleError> {
        let error = match outcome {
            ProviderAttemptHeaderOutcome::HeadersReceived { status: 200..=299 } => return Ok(()),
            ProviderAttemptHeaderOutcome::NotStarted { reason } => {
                let event = attempt.dispatch_id.as_ref().map_or_else(
                    || SessionEvent::ProviderAttemptNotStarted {
                        attempt_id: attempt.attempt_id.clone(),
                        reason: journal_not_started_reason(reason),
                    },
                    |dispatch_id| SessionEvent::ProviderAttemptNotStartedV2 {
                        attempt_id: attempt.attempt_id.clone(),
                        dispatch_id: dispatch_id.clone(),
                        reason: journal_not_started_reason(reason),
                    },
                );
                return self.append(event).await;
            }
            ProviderAttemptHeaderOutcome::HeadersReceived { status } => {
                format!("provider returned HTTP {status}")
            }
            ProviderAttemptHeaderOutcome::FailedBeforeHeaders { failure_code } => {
                format!("provider failed before response headers: {failure_code}")
            }
        };
        let outcome = CompletionOutcome::Failed { error };
        let event = match attempt.dispatch_id.as_ref() {
            None => SessionEvent::ProviderAttemptFinished {
                attempt_id: attempt.attempt_id.clone(),
                outcome,
                response_digest: None,
            },
            Some(dispatch_id) => SessionEvent::ProviderAttemptFinishedV2 {
                attempt_id: attempt.attempt_id.clone(),
                dispatch_id: dispatch_id.clone(),
                outcome,
                response_digest: None,
            },
        };
        self.append(event).await
    }
}

#[async_trait]
impl LlmProvider for JournaledLlmProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let request_digest =
            provider_request_digest(request).map_err(|error| ProviderError::NotAttempted {
                reason: format!("provider request digest could not be computed: {error}"),
            })?;
        let provider = if self.provider.is_empty() {
            self.inner.alias_key().to_owned()
        } else {
            self.provider.clone()
        };
        let model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };
        let scope = scope_provider_attempt_lifecycle(
            ProviderAttemptContext {
                dispatch_id: self.dispatch_id.clone(),
                turn_id: self.turn_id.clone(),
                purpose: self.purpose,
                request_digest,
                provider,
                model,
            },
            Arc::new(JournalAttemptLifecycle {
                journal: self.journal.clone(),
            }),
            self.inner.stream(request),
        )
        .await;
        let inner_rx = scope.output?;
        let Some(attempt_id) = scope.accepted_attempt_id else {
            drop(inner_rx);
            return Err(ProviderError::Parse(authority_message(
                "provider returned a stream without a durable accepted physical-attempt \
                     identity; the physical outcome may be unknown",
            )));
        };

        let stream_id = format!("provider-stream:{attempt_id}");
        if let Err(error) = append_event(
            &self.journal,
            SessionEvent::StreamStarted {
                stream_id: stream_id.clone(),
                attempt_id: attempt_id.clone(),
            },
        )
        .await
        {
            let _ = finish_attempt(
                &self.journal,
                &attempt_id,
                self.dispatch_id.as_deref(),
                CompletionOutcome::Failed {
                    error: format!("provider stream authority could not start: {error}"),
                },
                None,
            )
            .await;
            return Err(error);
        }

        let (tx, rx) = mpsc::channel(64);
        let journal = self.journal.clone();
        let dispatch_id = self.dispatch_id.clone();
        tokio::spawn(async move {
            forward_durable_stream(journal, attempt_id, dispatch_id, stream_id, inner_rx, tx).await;
        });
        Ok(rx)
    }

    fn alias_key(&self) -> &str {
        self.inner.alias_key()
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        self.inner.list_models().await
    }
}

async fn forward_durable_stream(
    journal: SessionJournal,
    attempt_id: String,
    dispatch_id: Option<String>,
    stream_id: String,
    mut inner_rx: mpsc::Receiver<LlmEvent>,
    tx: mpsc::Sender<LlmEvent>,
) {
    let mut ordinal = 0_u64;
    let mut response = Vec::new();

    loop {
        let event = tokio::select! {
            _ = tx.closed() => {
                // The local consumer going away proves only that Wayland
                // stopped reading. Once the physical attempt was accepted it
                // does not prove the provider stopped generating, nor reveal
                // its final usage/outcome. Leave the attempt StartedUnknown
                // so recovery requires explicit reconciliation.
                return;
            }
            event = inner_rx.recv() => event,
        };
        let Some(event) = event else {
            break;
        };
        let journal_event = match provider_stream_event(&event) {
            Ok(event) => event,
            Err(error) => {
                let _ = finish_attempt(
                    &journal,
                    &attempt_id,
                    dispatch_id.as_deref(),
                    CompletionOutcome::Failed {
                        error: error.to_string(),
                    },
                    if response.is_empty() {
                        None
                    } else {
                        provider_response_digest(&response).ok()
                    },
                )
                .await;
                surface_authority_error(&tx, &error).await;
                return;
            }
        };
        if let Err(error) = append_event(
            &journal,
            SessionEvent::StreamBatchCommitted {
                stream_id: stream_id.clone(),
                ordinal,
                events: vec![journal_event.clone()],
            },
        )
        .await
        {
            let _ = finish_attempt(
                &journal,
                &attempt_id,
                dispatch_id.as_deref(),
                CompletionOutcome::Failed {
                    error: "provider stream batch could not be made durable".to_owned(),
                },
                partial_response_digest(&response).unwrap_or(None),
            )
            .await;
            surface_authority_error(&tx, &error).await;
            return;
        }
        response.push(journal_event);
        ordinal = match ordinal.checked_add(1) {
            Some(next) => next,
            None => {
                let _ = finish_attempt(
                    &journal,
                    &attempt_id,
                    dispatch_id.as_deref(),
                    CompletionOutcome::Failed {
                        error: "provider stream batch ordinal exhausted".to_owned(),
                    },
                    partial_response_digest(&response).unwrap_or(None),
                )
                .await;
                surface_authority_error(
                    &tx,
                    &ProviderError::Parse(authority_message(
                        "provider stream batch ordinal exhausted",
                    )),
                )
                .await;
                return;
            }
        };

        match &event {
            LlmEvent::Done { .. } => {
                match finish_success(
                    &journal,
                    &attempt_id,
                    dispatch_id.as_deref(),
                    &stream_id,
                    &response,
                )
                .await
                {
                    Ok(()) => {
                        let _ = tx.send(event).await;
                    }
                    Err(error) => surface_authority_error(&tx, &error).await,
                }
                return;
            }
            LlmEvent::Error(message) => {
                let response_digest = match partial_response_digest(&response) {
                    Ok(digest) => digest,
                    Err(error) => {
                        let _ = finish_attempt(
                            &journal,
                            &attempt_id,
                            dispatch_id.as_deref(),
                            CompletionOutcome::Failed {
                                error: error.to_string(),
                            },
                            None,
                        )
                        .await;
                        surface_authority_error(&tx, &error).await;
                        return;
                    }
                };
                match finish_attempt(
                    &journal,
                    &attempt_id,
                    dispatch_id.as_deref(),
                    CompletionOutcome::Failed {
                        error: message.clone(),
                    },
                    response_digest,
                )
                .await
                {
                    Ok(()) => {
                        let _ = tx.send(event).await;
                    }
                    Err(error) => surface_authority_error(&tx, &error).await,
                }
                return;
            }
            _ => {
                if tx.send(event).await.is_err() {
                    // As above, a failed local delivery is not a terminal
                    // receipt from the provider. Keep the accepted attempt
                    // unknown rather than fabricating cancellation authority.
                    return;
                }
            }
        }
    }

    let response_digest = match partial_response_digest(&response) {
        Ok(digest) => digest,
        Err(error) => {
            let _ = finish_attempt(
                &journal,
                &attempt_id,
                dispatch_id.as_deref(),
                CompletionOutcome::Failed {
                    error: error.to_string(),
                },
                None,
            )
            .await;
            surface_authority_error(&tx, &error).await;
            return;
        }
    };
    if let Err(error) = finish_attempt(
        &journal,
        &attempt_id,
        dispatch_id.as_deref(),
        CompletionOutcome::Failed {
            error: "provider stream closed before a Done event".to_owned(),
        },
        response_digest,
    )
    .await
    {
        surface_authority_error(&tx, &error).await;
    }
}

async fn finish_success(
    journal: &SessionJournal,
    attempt_id: &str,
    dispatch_id: Option<&str>,
    stream_id: &str,
    response: &[ProviderStreamEvent],
) -> Result<(), ProviderError> {
    let digest = provider_response_digest(response)
        .map_err(|error| ProviderError::Parse(authority_message(error.to_string())))?;
    append_event(
        journal,
        SessionEvent::StreamFinished {
            stream_id: stream_id.to_owned(),
        },
    )
    .await?;
    finish_attempt(
        journal,
        attempt_id,
        dispatch_id,
        CompletionOutcome::Succeeded,
        Some(digest),
    )
    .await
}

async fn finish_attempt(
    journal: &SessionJournal,
    attempt_id: &str,
    dispatch_id: Option<&str>,
    outcome: CompletionOutcome,
    response_digest: Option<String>,
) -> Result<(), ProviderError> {
    let event = match dispatch_id {
        None => SessionEvent::ProviderAttemptFinished {
            attempt_id: attempt_id.to_owned(),
            outcome,
            response_digest,
        },
        Some(dispatch_id) => SessionEvent::ProviderAttemptFinishedV2 {
            attempt_id: attempt_id.to_owned(),
            dispatch_id: dispatch_id.to_owned(),
            outcome,
            response_digest,
        },
    };
    append_event(journal, event).await
}

async fn append_event(journal: &SessionJournal, event: SessionEvent) -> Result<(), ProviderError> {
    let journal = journal.clone();
    tokio::task::spawn_blocking(move || journal.append(event))
        .await
        .map_err(|error| {
            ProviderError::Parse(authority_message(format!(
                "provider journal append task failed: {error}"
            )))
        })?
        .map(|_| ())
        .map_err(|error| {
            ProviderError::Parse(authority_message(format!(
                "provider journal append failed: {error}"
            )))
        })
}

fn journal_purpose(purpose: LifecyclePurpose) -> ProviderAttemptPurpose {
    match purpose {
        LifecyclePurpose::Conversation => ProviderAttemptPurpose::Conversation,
        LifecyclePurpose::Compaction => ProviderAttemptPurpose::Compaction,
    }
}

fn journal_not_started_reason(reason: &LifecycleNotStartedReason) -> JournalNotStartedReason {
    match reason {
        LifecycleNotStartedReason::EgressDenied { reason } => {
            JournalNotStartedReason::EgressDenied {
                policy: reason.clone(),
            }
        }
        LifecycleNotStartedReason::BeforeDispatchFailed { error } => {
            JournalNotStartedReason::BeforeDispatchFailed {
                error: error.clone(),
            }
        }
    }
}

fn partial_response_digest(
    events: &[ProviderStreamEvent],
) -> Result<Option<String>, ProviderError> {
    if events.is_empty() {
        Ok(None)
    } else {
        provider_response_digest(events)
            .map(Some)
            .map_err(|error| ProviderError::Parse(authority_message(error.to_string())))
    }
}

fn authority_message(message: impl AsRef<str>) -> String {
    let message = message.as_ref();
    if is_journal_authority_error(message) {
        message.to_owned()
    } else {
        format!("{JOURNAL_AUTHORITY_ERROR_PREFIX}{message}")
    }
}

async fn surface_authority_error(tx: &mpsc::Sender<LlmEvent>, error: &ProviderError) {
    let _ = tx
        .send(LlmEvent::Error(authority_message(error.to_string())))
        .await;
}

fn provider_stream_event(event: &LlmEvent) -> Result<ProviderStreamEvent, ProviderError> {
    Ok(match event {
        LlmEvent::TextDelta(text) => ProviderStreamEvent::TextDelta { text: text.clone() },
        LlmEvent::ToolUse {
            id,
            name,
            input,
            extra,
        } => ProviderStreamEvent::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
            extra: extra.clone(),
        },
        LlmEvent::ThinkingDelta(text) => ProviderStreamEvent::ThinkingDelta { text: text.clone() },
        LlmEvent::ThinkingSubject(subject) => ProviderStreamEvent::ThinkingSubject {
            subject: subject.clone(),
        },
        LlmEvent::Done {
            stop_reason,
            finish_reason,
            usage,
        } => ProviderStreamEvent::Done {
            stop_reason: serde_json::Value::String(
                match stop_reason {
                    StopReason::EndTurn => "end_turn",
                    StopReason::ToolUse => "tool_use",
                    StopReason::MaxTokens => "max_tokens",
                    StopReason::MaxTurns => "max_turns",
                }
                .to_owned(),
            ),
            finish_reason: serde_json::to_value(finish_reason).map_err(|error| {
                ProviderError::Parse(authority_message(format!(
                    "provider finish reason could not be journaled: {error}"
                )))
            })?,
            usage: serde_json::to_value(usage).map_err(|error| {
                ProviderError::Parse(authority_message(format!(
                    "provider usage could not be journaled: {error}"
                )))
            })?,
        },
        LlmEvent::Error(message) => ProviderStreamEvent::Error {
            message: message.clone(),
        },
        LlmEvent::Citations(urls) => ProviderStreamEvent::Citations { urls: urls.clone() },
        LlmEvent::SearchResults(results) => ProviderStreamEvent::SearchResults {
            results: results
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    ProviderError::Parse(authority_message(format!(
                        "provider search results could not be journaled: {error}"
                    )))
                })?,
        },
        LlmEvent::ProviderMeta {
            routed_model,
            model_window,
            context_pressure,
            tokens_counted,
        } => {
            if context_pressure.is_some_and(|pressure| !pressure.is_finite()) {
                return Err(ProviderError::Parse(authority_message(
                    "provider metadata could not be journaled: context pressure is not finite",
                )));
            }
            ProviderStreamEvent::ProviderMeta {
                metadata: serde_json::to_value(ProviderMetadata {
                    routed_model,
                    model_window,
                    context_pressure,
                    tokens_counted,
                })
                .map_err(|error| {
                    ProviderError::Parse(authority_message(format!(
                        "provider metadata could not be journaled: {error}"
                    )))
                })?,
            }
        }
    })
}

#[derive(serde::Serialize)]
struct ProviderMetadata<'a> {
    routed_model: &'a Option<String>,
    model_window: &'a Option<u64>,
    context_pressure: &'a Option<f32>,
    tokens_counted: &'a Option<u64>,
}

#[cfg(test)]
mod tests;
