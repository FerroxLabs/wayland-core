use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::Notify;
use wcore_egress::{AllowAllPolicy, EgressClient, EgressDecision, EgressPolicy, reqwest};
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_types::message::{FinishReason, TokenUsage};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

#[derive(Clone)]
enum ProviderScript {
    Events(Vec<LlmEvent>),
    Delayed {
        release: Arc<Notify>,
        events: Vec<LlmEvent>,
    },
    Hang {
        inner_closed: Arc<AtomicBool>,
        closed: Arc<Notify>,
    },
}

struct PhysicalScriptProvider {
    url: String,
    script: ProviderScript,
}

#[async_trait]
impl LlmProvider for PhysicalScriptProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let response = scope_max_retries(0, builder_send_with_retry(client.get(&self.url))).await?;
        if !response.status().is_success() {
            return Err(ProviderError::Api {
                status: response.status().as_u16(),
                message: "fixture response".into(),
            });
        }

        let (tx, rx) = mpsc::channel(8);
        match self.script.clone() {
            ProviderScript::Events(events) => {
                tokio::spawn(async move {
                    for event in events {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ProviderScript::Delayed { release, events } => {
                tokio::spawn(async move {
                    release.notified().await;
                    for event in events {
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                });
            }
            ProviderScript::Hang {
                inner_closed,
                closed,
            } => {
                tokio::spawn(async move {
                    tx.closed().await;
                    inner_closed.store(true, Ordering::SeqCst);
                    closed.notify_one();
                });
            }
        }
        Ok(rx)
    }

    fn alias_key(&self) -> &str {
        "fixture"
    }
}

struct JournalFixture {
    _dir: TempDir,
    path: PathBuf,
    journal: SessionJournal,
}

impl JournalFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.journal");
        let journal = SessionJournal::open(&path, "session").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".into(),
                user_message: "hello".into(),
            })
            .unwrap();
        Self {
            _dir: dir,
            path,
            journal,
        }
    }
}

async fn success_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

fn request(model: &str) -> LlmRequest {
    LlmRequest {
        model: model.into(),
        system: "system".into(),
        max_tokens: 128,
        ..LlmRequest::default()
    }
}

fn done() -> LlmEvent {
    LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::Stop,
        usage: TokenUsage {
            input_tokens: 5,
            output_tokens: 2,
            ..TokenUsage::default()
        },
    }
}

fn journaled(
    server: &MockServer,
    fixture: &JournalFixture,
    purpose: LifecyclePurpose,
    script: ProviderScript,
) -> JournaledLlmProvider {
    JournaledLlmProvider::new(
        Arc::new(PhysicalScriptProvider {
            url: server.uri(),
            script,
        }),
        fixture.journal.clone(),
        "turn",
        purpose,
        "fixture",
        "fallback-model",
    )
}

fn only_attempt(
    fixture: &JournalFixture,
) -> (String, crate::session_journal::ProviderAttemptState) {
    let state = fixture.journal.state().unwrap();
    assert_eq!(state.provider_attempts.len(), 1);
    let (attempt_id, attempt) = state.provider_attempts.into_iter().next().unwrap();
    (attempt_id, attempt)
}

async fn wait_until_completed(fixture: &JournalFixture) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let state = fixture.journal.state().unwrap();
            if state.provider_attempts.values().all(|attempt| {
                matches!(
                    &attempt.effect,
                    crate::session_journal::ExternalEffectState::Completed { .. }
                )
            }) && !state.provider_attempts.is_empty()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider attempt did not terminalize");
}

#[test]
fn authority_marker_survives_provider_error_wrapping() {
    let wrapped = ProviderError::Parse(authority_message("disk sync failed")).to_string();

    assert!(is_journal_authority_error(&wrapped));
    assert!(!ProviderError::Parse(wrapped).is_retryable());
    assert!(!is_journal_authority_error(
        "ordinary provider stream error"
    ));
}

#[tokio::test]
async fn physical_attempt_and_stream_are_durable_before_visibility() {
    let server = success_server().await;
    let fixture = JournalFixture::new();
    let provider = journaled(
        &server,
        &fixture,
        LifecyclePurpose::Conversation,
        ProviderScript::Events(vec![LlmEvent::TextDelta("hello".into()), done()]),
    );

    let mut rx = provider.stream(&request("model-a")).await.unwrap();
    assert!(matches!(rx.recv().await, Some(LlmEvent::TextDelta(text)) if text == "hello"));

    let entries = SessionJournal::replay(&fixture.path).unwrap();
    let prepared = entries
        .iter()
        .position(|entry| matches!(&entry.event, SessionEvent::ProviderAttemptPrepared { .. }))
        .unwrap();
    let started = entries
        .iter()
        .position(|entry| matches!(&entry.event, SessionEvent::ProviderAttemptStarted { .. }))
        .unwrap();
    let stream_started = entries
        .iter()
        .position(|entry| matches!(&entry.event, SessionEvent::StreamStarted { .. }))
        .unwrap();
    let first_batch = entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.event,
                SessionEvent::StreamBatchCommitted { ordinal: 0, .. }
            )
        })
        .unwrap();
    assert!(prepared < started && started < stream_started && stream_started < first_batch);

    assert!(matches!(rx.recv().await, Some(LlmEvent::Done { .. })));
    let entries = SessionJournal::replay(&fixture.path).unwrap();
    let done_batch = entries
        .iter()
        .rposition(|entry| matches!(&entry.event, SessionEvent::StreamBatchCommitted { .. }))
        .unwrap();
    let stream_finished = entries
        .iter()
        .position(|entry| matches!(&entry.event, SessionEvent::StreamFinished { .. }))
        .unwrap();
    let attempt_finished = entries
        .iter()
        .position(|entry| {
            matches!(
                &entry.event,
                SessionEvent::ProviderAttemptFinished {
                    outcome: CompletionOutcome::Succeeded,
                    ..
                }
            )
        })
        .unwrap();
    assert!(done_batch < stream_finished && stream_finished < attempt_finished);

    let state = fixture.journal.state().unwrap();
    let attempt = state.provider_attempts.values().next().unwrap();
    let stream = state.streams.values().next().unwrap();
    let response = stream.batches.iter().flatten().cloned().collect::<Vec<_>>();
    assert_eq!(
        attempt.response_digest,
        Some(response_digest(&response).unwrap())
    );
}

#[tokio::test]
async fn stream_without_accepted_physical_identity_fails_closed() {
    struct BareProvider;

    #[async_trait]
    impl LlmProvider for BareProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
    }

    let fixture = JournalFixture::new();
    let provider = JournaledLlmProvider::new(
        Arc::new(BareProvider),
        fixture.journal.clone(),
        "turn",
        LifecyclePurpose::Conversation,
        "fixture",
        "model",
    );
    let error = provider.stream(&request("model")).await.unwrap_err();

    assert!(is_journal_authority_error(&error.to_string()));
    assert!(!error.is_retryable());
    assert!(
        fixture
            .journal
            .state()
            .unwrap()
            .provider_attempts
            .is_empty()
    );
}

#[tokio::test]
async fn egress_denial_terminalizes_prepared_identity_as_not_started() {
    #[derive(Debug)]
    struct DenyPolicy;

    #[async_trait]
    impl EgressPolicy for DenyPolicy {
        async fn check(&self, _request: &reqwest::Request) -> EgressDecision {
            EgressDecision::Deny {
                reason: "fixture policy denied egress".into(),
            }
        }
    }

    struct DeniedProvider;

    #[async_trait]
    impl LlmProvider for DeniedProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let client = EgressClient::new().with_policy(Arc::new(DenyPolicy));
            builder_send_with_retry(client.get("https://denied.invalid/"))
                .await
                .map(|_| {
                    let (_tx, rx) = mpsc::channel(1);
                    rx
                })
        }
    }

    let fixture = JournalFixture::new();
    let provider = JournaledLlmProvider::new(
        Arc::new(DeniedProvider),
        fixture.journal.clone(),
        "turn",
        LifecyclePurpose::Conversation,
        "fixture",
        "model",
    );
    let error = provider.stream(&request("model")).await.unwrap_err();

    assert!(!error.is_retryable());
    let (_, attempt) = only_attempt(&fixture);
    assert!(matches!(
        attempt.effect,
        crate::session_journal::ExternalEffectState::NotStarted
    ));
    assert!(matches!(
        attempt.not_started_reason,
        Some(JournalNotStartedReason::EgressDenied { ref policy })
            if policy == "fixture policy denied egress"
    ));
    assert!(fixture.journal.state().unwrap().streams.is_empty());
}

#[tokio::test]
async fn journal_append_failure_surfaces_terminal_authority_error() {
    let server = success_server().await;
    let fixture = JournalFixture::new();
    let release = Arc::new(Notify::new());
    let provider = journaled(
        &server,
        &fixture,
        LifecyclePurpose::Conversation,
        ProviderScript::Delayed {
            release: Arc::clone(&release),
            events: vec![LlmEvent::TextDelta("late".into())],
        },
    );
    let mut rx = provider.stream(&request("model")).await.unwrap();
    let (attempt_id, _) = only_attempt(&fixture);
    fixture
        .journal
        .append(SessionEvent::ProviderAttemptFinished {
            attempt_id,
            outcome: CompletionOutcome::Failed {
                error: "forced competing terminal event".into(),
            },
            response_digest: None,
        })
        .unwrap();

    release.notify_one();
    let message = match rx.recv().await {
        Some(LlmEvent::Error(message)) => message,
        other => panic!("expected authority error, got {other:?}"),
    };
    assert!(is_journal_authority_error(&message));
    assert!(!ProviderError::Parse(message).is_retryable());
}

#[tokio::test]
async fn event_conversion_failure_terminalizes_and_surfaces_authority() {
    let server = success_server().await;
    let fixture = JournalFixture::new();
    let provider = journaled(
        &server,
        &fixture,
        LifecyclePurpose::Conversation,
        ProviderScript::Events(vec![LlmEvent::ProviderMeta {
            routed_model: Some("model".into()),
            model_window: Some(10),
            context_pressure: Some(f32::NAN),
            tokens_counted: Some(5),
        }]),
    );
    let mut rx = provider.stream(&request("model")).await.unwrap();

    let message = match rx.recv().await {
        Some(LlmEvent::Error(message)) => message,
        other => panic!("expected authority error, got {other:?}"),
    };
    assert!(is_journal_authority_error(&message));
    assert!(!ProviderError::Parse(message).is_retryable());
    let (_, attempt) = only_attempt(&fixture);
    assert!(matches!(
        attempt.effect,
        crate::session_journal::ExternalEffectState::Completed {
            outcome: CompletionOutcome::Failed { .. }
        }
    ));
}

#[tokio::test]
async fn provider_error_and_truncation_terminalize_as_failed() {
    let server = success_server().await;

    let errored = JournalFixture::new();
    let provider = journaled(
        &server,
        &errored,
        LifecyclePurpose::Conversation,
        ProviderScript::Events(vec![LlmEvent::Error("provider failed".into())]),
    );
    let mut rx = provider.stream(&request("model")).await.unwrap();
    assert!(
        matches!(rx.recv().await, Some(LlmEvent::Error(message)) if message == "provider failed")
    );
    let (_, attempt) = only_attempt(&errored);
    assert!(matches!(
        attempt.effect,
        crate::session_journal::ExternalEffectState::Completed {
            outcome: CompletionOutcome::Failed { ref error }
        } if error == "provider failed"
    ));

    let truncated = JournalFixture::new();
    let provider = journaled(
        &server,
        &truncated,
        LifecyclePurpose::Conversation,
        ProviderScript::Events(vec![LlmEvent::TextDelta("partial".into())]),
    );
    let mut rx = provider.stream(&request("model")).await.unwrap();
    assert!(matches!(rx.recv().await, Some(LlmEvent::TextDelta(text)) if text == "partial"));
    assert!(rx.recv().await.is_none());
    let (_, attempt) = only_attempt(&truncated);
    assert!(matches!(
        attempt.effect,
        crate::session_journal::ExternalEffectState::Completed {
            outcome: CompletionOutcome::Failed { ref error }
        } if error.contains("closed before a Done event")
    ));
    assert!(attempt.response_digest.is_some());
}

#[tokio::test]
async fn receiver_drop_cancels_attempt_and_hung_inner_stream() {
    let server = success_server().await;
    let fixture = JournalFixture::new();
    let inner_closed = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(Notify::new());
    let provider = journaled(
        &server,
        &fixture,
        LifecyclePurpose::Conversation,
        ProviderScript::Hang {
            inner_closed: Arc::clone(&inner_closed),
            closed: Arc::clone(&closed),
        },
    );
    let rx = provider.stream(&request("model")).await.unwrap();

    drop(rx);
    tokio::time::timeout(Duration::from_secs(2), closed.notified())
        .await
        .expect("hung inner provider was not cancelled");
    wait_until_completed(&fixture).await;
    assert!(inner_closed.load(Ordering::SeqCst));
    let (_, attempt) = only_attempt(&fixture);
    assert!(matches!(
        attempt.effect,
        crate::session_journal::ExternalEffectState::Completed {
            outcome: CompletionOutcome::Cancelled
        }
    ));
}

#[tokio::test]
async fn request_digest_and_purpose_are_exact_for_conversation_and_compaction() {
    let server = success_server().await;
    let fixture = JournalFixture::new();
    let conversation_request = request("model-a");
    let mut compaction_request = request("model-a");
    compaction_request.system = "compact this exact state".into();
    let conversation_digest = provider_request_digest(&conversation_request).unwrap();
    let compaction_digest = provider_request_digest(&compaction_request).unwrap();
    assert_ne!(conversation_digest, compaction_digest);

    let provider = journaled(
        &server,
        &fixture,
        LifecyclePurpose::Conversation,
        ProviderScript::Events(vec![done()]),
    );
    let mut rx = provider.stream(&conversation_request).await.unwrap();
    assert!(matches!(rx.recv().await, Some(LlmEvent::Done { .. })));

    let provider = journaled(
        &server,
        &fixture,
        LifecyclePurpose::Compaction,
        ProviderScript::Events(vec![done()]),
    );
    let mut rx = provider.stream(&compaction_request).await.unwrap();
    assert!(matches!(rx.recv().await, Some(LlmEvent::Done { .. })));

    let state = fixture.journal.state().unwrap();
    assert_eq!(state.provider_attempts.len(), 2);
    assert!(state.provider_attempts.values().any(|attempt| {
        attempt.purpose == ProviderAttemptPurpose::Conversation
            && attempt.request_digest == conversation_digest
    }));
    assert!(state.provider_attempts.values().any(|attempt| {
        attempt.purpose == ProviderAttemptPurpose::Compaction
            && attempt.request_digest == compaction_digest
    }));
}
