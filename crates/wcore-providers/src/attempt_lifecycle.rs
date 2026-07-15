//! Durable, provider-neutral lifecycle hooks for physical inference attempts.
//!
//! The agent installs one task-local scope around a logical provider call.
//! Retry and fallback layers inherit that scope, while provider wrappers may
//! override only the actual provider/model identity before delegating. The
//! HTTP send boundary calls `prepare` before policy admission and `started`
//! only after admission, immediately before the operating system can dispatch
//! bytes. `finished` records a proved no-send outcome, the first header, or a
//! pre-header transport failure.

use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uuid::Uuid;

use crate::ProviderError;

/// Why the model call is being made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttemptPurpose {
    Conversation,
    Compaction,
}

/// Caller-supplied linkage shared by every physical retry/fallback attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderAttemptContext {
    pub turn_id: String,
    pub purpose: ProviderAttemptPurpose,
    pub request_digest: String,
    pub provider: String,
    pub model: String,
}

/// Stable identity and complete metadata for one physical send.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhysicalProviderAttempt {
    pub attempt_id: String,
    pub turn_id: String,
    pub purpose: ProviderAttemptPurpose,
    pub request_digest: String,
    pub provider: String,
    pub model: String,
}

impl PhysicalProviderAttempt {
    fn new(context: &ProviderAttemptContext) -> Self {
        Self {
            attempt_id: Uuid::now_v7().to_string(),
            turn_id: context.turn_id.clone(),
            purpose: context.purpose,
            request_digest: context.request_digest.clone(),
            provider: context.provider.clone(),
            model: context.model.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderAttemptNotStartedReason {
    EgressDenied { reason: String },
    BeforeDispatchFailed { error: String },
}

/// The result known at the physical send boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderAttemptHeaderOutcome {
    /// Policy or durable-admission failure proved that network dispatch never
    /// started. This terminalizes a prepared identity without pretending the
    /// provider effect became unknown.
    NotStarted {
        reason: ProviderAttemptNotStartedReason,
    },
    HeadersReceived {
        status: u16,
    },
    FailedBeforeHeaders {
        failure_code: String,
    },
}

/// A lifecycle sink rejected a durable transition.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ProviderAttemptLifecycleError {
    message: String,
}

impl ProviderAttemptLifecycleError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Object-safe sink implemented by the owning session journal.
#[async_trait]
pub trait ProviderAttemptLifecycle: Send + Sync {
    /// Durably reserve the attempt identity before any send can occur.
    async fn prepare(
        &self,
        attempt: &PhysicalProviderAttempt,
    ) -> Result<(), ProviderAttemptLifecycleError>;

    /// Durably mark the attempt started immediately before the physical send.
    async fn started(
        &self,
        attempt: &PhysicalProviderAttempt,
    ) -> Result<(), ProviderAttemptLifecycleError>;

    /// Durably record the proved no-send, first-header, or pre-header outcome.
    async fn finished(
        &self,
        attempt: &PhysicalProviderAttempt,
        outcome: &ProviderAttemptHeaderOutcome,
    ) -> Result<(), ProviderAttemptLifecycleError>;
}

#[derive(Clone)]
struct ActiveAttemptScope {
    lifecycle: Arc<dyn ProviderAttemptLifecycle>,
    context: ProviderAttemptContext,
    accepted_attempt_id: Arc<Mutex<Option<String>>>,
}

tokio::task_local! {
    static ACTIVE_ATTEMPT_SCOPE: ActiveAttemptScope;
}

/// Result of a logical provider call, including the physical attempt whose
/// successful headers opened the returned stream.
pub struct ProviderAttemptScopeResult<T> {
    pub output: T,
    pub accepted_attempt_id: Option<String>,
}

/// Install durable physical-attempt tracking around one logical provider call.
pub async fn scope_provider_attempt_lifecycle<F>(
    context: ProviderAttemptContext,
    lifecycle: Arc<dyn ProviderAttemptLifecycle>,
    future: F,
) -> ProviderAttemptScopeResult<F::Output>
where
    F: Future,
{
    let accepted_attempt_id = Arc::new(Mutex::new(None));
    let scope = ActiveAttemptScope {
        lifecycle,
        context,
        accepted_attempt_id: Arc::clone(&accepted_attempt_id),
    };
    let output = ACTIVE_ATTEMPT_SCOPE.scope(scope, future).await;
    let accepted_attempt_id = accepted_attempt_id
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    ProviderAttemptScopeResult {
        output,
        accepted_attempt_id,
    }
}

/// Override actual provider/model identity while retaining caller linkage and
/// the same lifecycle sink. A no-scope call remains a zero-cost delegation.
pub async fn scope_provider_attempt_identity<F>(
    provider: impl Into<String>,
    model: impl Into<String>,
    future: F,
) -> F::Output
where
    F: Future,
{
    let Some(mut scope) = ACTIVE_ATTEMPT_SCOPE.try_with(Clone::clone).ok() else {
        return future.await;
    };
    scope.context.provider = provider.into();
    scope.context.model = model.into();
    ACTIVE_ATTEMPT_SCOPE.scope(scope, future).await
}

/// Durably reserve one physical attempt before egress policy admission.
pub(crate) async fn begin_physical_attempt()
-> Result<Option<PhysicalProviderAttempt>, ProviderError> {
    let Some(scope) = ACTIVE_ATTEMPT_SCOPE.try_with(Clone::clone).ok() else {
        return Ok(None);
    };
    let attempt = PhysicalProviderAttempt::new(&scope.context);
    scope
        .lifecycle
        .prepare(&attempt)
        .await
        .map_err(|error| ProviderError::NotAttempted {
            reason: format!("provider attempt prepare was not durable: {error}"),
        })?;
    Ok(Some(attempt))
}

/// Durably mark an admitted attempt immediately before network dispatch.
pub(crate) async fn start_physical_attempt(
    attempt: Option<&PhysicalProviderAttempt>,
) -> Result<(), ProviderError> {
    let Some(attempt) = attempt else {
        return Ok(());
    };
    let scope =
        ACTIVE_ATTEMPT_SCOPE
            .try_with(Clone::clone)
            .map_err(|_| ProviderError::NotAttempted {
                reason: "provider attempt lifecycle scope disappeared before dispatch".into(),
            })?;
    scope
        .lifecycle
        .started(attempt)
        .await
        .map_err(|error| ProviderError::NotAttempted {
            reason: format!("provider attempt start was not durable: {error}"),
        })
}

/// Finish one physical attempt before its response or error escapes the send
/// boundary. A finish failure is terminal: repeating an already-dispatched
/// request would risk duplicating an external effect.
pub(crate) async fn finish_physical_attempt(
    attempt: Option<&PhysicalProviderAttempt>,
    outcome: ProviderAttemptHeaderOutcome,
) -> Result<(), ProviderError> {
    let Some(attempt) = attempt else {
        return Ok(());
    };
    let scope = ACTIVE_ATTEMPT_SCOPE
        .try_with(Clone::clone)
        .map_err(|_| ProviderError::Parse("provider attempt lifecycle scope disappeared".into()))?;
    scope
        .lifecycle
        .finished(attempt, &outcome)
        .await
        .map_err(|error| {
            ProviderError::Parse(format!(
                "provider attempt outcome was not durable after dispatch: {error}"
            ))
        })?;
    if matches!(
        outcome,
        ProviderAttemptHeaderOutcome::HeadersReceived { status: 200..=299 }
    ) {
        *scope
            .accepted_attempt_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(attempt.attempt_id.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio::sync::mpsc;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::resilient::{CircuitConfig, NoOpCircuitReporter, ResilientProvider};
    use crate::retry::{builder_send_with_retry, scope_max_retries};
    use crate::{LlmProvider, ProviderChain};

    #[derive(Default)]
    struct RecordingLifecycle {
        prepared: Mutex<Vec<PhysicalProviderAttempt>>,
        started: Mutex<Vec<PhysicalProviderAttempt>>,
        finished: Mutex<Vec<(PhysicalProviderAttempt, ProviderAttemptHeaderOutcome)>>,
        fail_prepare: AtomicBool,
        fail_start: AtomicBool,
    }

    #[async_trait]
    impl ProviderAttemptLifecycle for RecordingLifecycle {
        async fn prepare(
            &self,
            attempt: &PhysicalProviderAttempt,
        ) -> Result<(), ProviderAttemptLifecycleError> {
            self.prepared.lock().unwrap().push(attempt.clone());
            if self.fail_prepare.load(Ordering::SeqCst) {
                return Err(ProviderAttemptLifecycleError::new("prepare rejected"));
            }
            Ok(())
        }

        async fn started(
            &self,
            attempt: &PhysicalProviderAttempt,
        ) -> Result<(), ProviderAttemptLifecycleError> {
            self.started.lock().unwrap().push(attempt.clone());
            if self.fail_start.load(Ordering::SeqCst) {
                return Err(ProviderAttemptLifecycleError::new("start rejected"));
            }
            Ok(())
        }

        async fn finished(
            &self,
            attempt: &PhysicalProviderAttempt,
            outcome: &ProviderAttemptHeaderOutcome,
        ) -> Result<(), ProviderAttemptLifecycleError> {
            self.finished
                .lock()
                .unwrap()
                .push((attempt.clone(), outcome.clone()));
            Ok(())
        }
    }

    fn context() -> ProviderAttemptContext {
        ProviderAttemptContext {
            turn_id: "turn-1".into(),
            purpose: ProviderAttemptPurpose::Conversation,
            request_digest: "sha256:test".into(),
            provider: "configured".into(),
            model: "configured-model".into(),
        }
    }

    fn request(model: &str) -> LlmRequest {
        LlmRequest {
            model: model.into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 128,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: vec![],
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
        }
    }

    struct HttpStatusProvider {
        url: String,
    }

    struct DenyPolicy;

    #[async_trait]
    impl wcore_egress::EgressPolicy for DenyPolicy {
        async fn check(&self, _: &reqwest::Request) -> wcore_egress::EgressDecision {
            wcore_egress::EgressDecision::Deny {
                reason: "fixture denial".into(),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for HttpStatusProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let client = wcore_egress::EgressClient::new()
                .with_policy(Arc::new(wcore_egress::AllowAllPolicy));
            let response = builder_send_with_retry(client.post(&self.url)).await?;
            let status = response.status().as_u16();
            if !response.status().is_success() {
                return Err(ProviderError::Api {
                    status,
                    message: "fixture response".into(),
                });
            }
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }
    }

    async fn status_server(status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn pre_send_lifecycle_failure_prevents_physical_invocation() {
        let server = status_server(200).await;
        let lifecycle = Arc::new(RecordingLifecycle::default());
        lifecycle.fail_start.store(true, Ordering::SeqCst);
        let lifecycle_object: Arc<dyn ProviderAttemptLifecycle> = lifecycle.clone();
        let client =
            wcore_egress::EgressClient::new().with_policy(Arc::new(wcore_egress::AllowAllPolicy));

        let result = scope_provider_attempt_lifecycle(
            context(),
            lifecycle_object,
            builder_send_with_retry(client.post(server.uri())),
        )
        .await;

        assert!(matches!(
            result.output,
            Err(ProviderError::NotAttempted { .. })
        ));
        assert!(result.accepted_attempt_id.is_none());
        assert_eq!(lifecycle.prepared.lock().unwrap().len(), 1);
        assert_eq!(lifecycle.started.lock().unwrap().len(), 1);
        let finished = lifecycle.finished.lock().unwrap();
        assert_eq!(finished.len(), 1);
        assert!(matches!(
            &finished[0].1,
            ProviderAttemptHeaderOutcome::NotStarted { reason }
                if matches!(reason, ProviderAttemptNotStartedReason::BeforeDispatchFailed { .. })
        ));
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded requests")
                .is_empty(),
            "a rejected durable start must prevent the network send"
        );
    }

    #[tokio::test]
    async fn policy_denial_terminalizes_the_prepared_attempt_without_starting_it() {
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let lifecycle_object: Arc<dyn ProviderAttemptLifecycle> = lifecycle.clone();
        let client = wcore_egress::EgressClient::new().with_policy(Arc::new(DenyPolicy));

        let result = scope_provider_attempt_lifecycle(
            context(),
            lifecycle_object,
            builder_send_with_retry(client.post("https://denied.invalid/")),
        )
        .await;

        assert!(matches!(result.output, Err(ProviderError::Egress(_))));
        assert!(result.accepted_attempt_id.is_none());
        assert_eq!(lifecycle.prepared.lock().unwrap().len(), 1);
        assert!(lifecycle.started.lock().unwrap().is_empty());
        let finished = lifecycle.finished.lock().unwrap();
        assert_eq!(finished.len(), 1);
        assert!(matches!(
            &finished[0].1,
            ProviderAttemptHeaderOutcome::NotStarted { reason }
                if matches!(reason, ProviderAttemptNotStartedReason::EgressDenied { reason }
                    if reason == "fixture denial")
        ));
    }

    #[tokio::test]
    async fn physical_retries_receive_distinct_stable_attempt_ids() {
        let server = status_server(503).await;
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let lifecycle_object: Arc<dyn ProviderAttemptLifecycle> = lifecycle.clone();
        let client =
            wcore_egress::EgressClient::new().with_policy(Arc::new(wcore_egress::AllowAllPolicy));

        let result = scope_provider_attempt_lifecycle(
            context(),
            lifecycle_object,
            scope_max_retries(1, builder_send_with_retry(client.post(server.uri()))),
        )
        .await;

        assert_eq!(result.output.unwrap().status().as_u16(), 503);
        assert!(result.accepted_attempt_id.is_none());
        let prepared = lifecycle.prepared.lock().unwrap();
        let started = lifecycle.started.lock().unwrap();
        let finished = lifecycle.finished.lock().unwrap();
        assert_eq!(prepared.len(), 2);
        assert_eq!(started.len(), 2);
        assert_eq!(finished.len(), 2);
        let ids = prepared
            .iter()
            .map(|attempt| attempt.attempt_id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 2, "each physical retry needs a unique id");
        assert_eq!(
            prepared
                .iter()
                .map(|attempt| &attempt.attempt_id)
                .collect::<Vec<_>>(),
            started
                .iter()
                .map(|attempt| &attempt.attempt_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            prepared
                .iter()
                .map(|attempt| &attempt.attempt_id)
                .collect::<Vec<_>>(),
            finished
                .iter()
                .map(|(attempt, _)| &attempt.attempt_id)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn transport_failure_is_finished_before_the_error_escapes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture port");
        let address = listener.local_addr().expect("fixture address");
        drop(listener);
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let lifecycle_object: Arc<dyn ProviderAttemptLifecycle> = lifecycle.clone();
        let client =
            wcore_egress::EgressClient::new().with_policy(Arc::new(wcore_egress::AllowAllPolicy));

        let result = scope_provider_attempt_lifecycle(
            context(),
            lifecycle_object,
            scope_max_retries(
                0,
                builder_send_with_retry(client.post(format!("http://{address}/"))),
            ),
        )
        .await;

        assert!(matches!(result.output, Err(ProviderError::Connection(_))));
        let finished = lifecycle.finished.lock().unwrap();
        assert_eq!(finished.len(), 1);
        assert!(matches!(
            &finished[0].1,
            ProviderAttemptHeaderOutcome::FailedBeforeHeaders { failure_code }
                if failure_code == "connection"
        ));
    }

    #[tokio::test]
    async fn provider_chain_overrides_each_physical_slot_identity() {
        let primary_server = status_server(503).await;
        let fallback_server = status_server(200).await;
        let chain = ProviderChain::new(vec![
            (
                "primary",
                Arc::new(HttpStatusProvider {
                    url: primary_server.uri(),
                }) as Arc<dyn LlmProvider>,
            ),
            (
                "fallback",
                Arc::new(HttpStatusProvider {
                    url: fallback_server.uri(),
                }) as Arc<dyn LlmProvider>,
            ),
        ]);
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let lifecycle_object: Arc<dyn ProviderAttemptLifecycle> = lifecycle.clone();

        let result = scope_provider_attempt_lifecycle(
            context(),
            lifecycle_object,
            scope_max_retries(0, chain.stream(&request("requested-model"))),
        )
        .await;

        assert!(result.output.is_ok());
        let prepared = lifecycle.prepared.lock().unwrap();
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[0].provider, "primary");
        assert_eq!(prepared[0].model, "requested-model");
        assert_eq!(prepared[1].provider, "fallback");
        assert_eq!(prepared[1].model, "requested-model");
        assert_eq!(
            result.accepted_attempt_id.as_deref(),
            Some(prepared[1].attempt_id.as_str())
        );
    }

    #[tokio::test]
    async fn resilient_fallback_uses_actual_pricing_provider_and_model() {
        let primary_server = status_server(503).await;
        let fallback_server = status_server(200).await;
        let resilient = ResilientProvider::new_with_fallback_identities(
            "primary-provider",
            Arc::new(HttpStatusProvider {
                url: primary_server.uri(),
            }),
            vec![(
                "fallback-label".into(),
                "fallback-provider".into(),
                "fallback-model".into(),
                Arc::new(HttpStatusProvider {
                    url: fallback_server.uri(),
                }),
            )],
            CircuitConfig::default(),
            Arc::new(NoOpCircuitReporter),
        );
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let lifecycle_object: Arc<dyn ProviderAttemptLifecycle> = lifecycle.clone();

        let result = scope_provider_attempt_lifecycle(
            context(),
            lifecycle_object,
            scope_max_retries(0, resilient.stream(&request("primary-model"))),
        )
        .await;

        assert!(result.output.is_ok());
        let prepared = lifecycle.prepared.lock().unwrap();
        assert_eq!(prepared.len(), 2);
        assert_eq!(prepared[0].provider, "primary-provider");
        assert_eq!(prepared[0].model, "primary-model");
        assert_eq!(prepared[1].provider, "fallback-provider");
        assert_eq!(prepared[1].model, "fallback-model");
        assert_eq!(
            result.accepted_attempt_id.as_deref(),
            Some(prepared[1].attempt_id.as_str())
        );
    }

    #[tokio::test]
    async fn open_circuit_does_not_report_a_false_physical_start() {
        let server = status_server(503).await;
        let resilient = ResilientProvider::new(
            "primary-provider",
            Arc::new(HttpStatusProvider { url: server.uri() }),
            vec![],
            CircuitConfig {
                fail_threshold: 1,
                window: Duration::from_secs(30),
                cooldown: Duration::from_secs(60),
            },
            Arc::new(NoOpCircuitReporter),
        );
        let lifecycle = Arc::new(RecordingLifecycle::default());
        let lifecycle_object: Arc<dyn ProviderAttemptLifecycle> = lifecycle.clone();

        let result = scope_provider_attempt_lifecycle(context(), lifecycle_object, async {
            let first = scope_max_retries(0, resilient.stream(&request("model"))).await;
            let after_first = lifecycle.started.lock().unwrap().len();
            let second = resilient.stream(&request("model")).await;
            let after_second = lifecycle.started.lock().unwrap().len();
            (first, second, after_first, after_second)
        })
        .await;

        let (first, second, after_first, after_second) = result.output;
        assert!(matches!(first, Err(ProviderError::Api { status: 503, .. })));
        assert!(matches!(second, Err(ProviderError::NotAttempted { .. })));
        assert_eq!(after_first, 1);
        assert_eq!(after_second, after_first);
    }
}
