//! wayland#1133 (c) — the failover notice must name WHY the primary was
//! skipped, not only that it was.
//!
//! The shipped sentence named the failed provider, the serving provider and a
//! list of things to check. It did NOT name the fault class, so a user whose
//! endpoint was refused got "Check 'anthropic' — its `base_url`, credentials
//! and quota" where, with no chain configured at all, they would have got
//! "Connection refused (os error 111)". A pointer replaced the diagnosis.
//!
//! The class is not a guess: `ResilientProvider` classifies the primary's
//! failure with `retry::provider_failure_code` and stores it in
//! `primary_failure_code` in the SAME function that later calls
//! `report_failover`. This test drives the real `ResilientProvider` over a
//! real `ProtocolCircuitReporter` and asserts the class survives the trip to
//! the sink.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::output::OutputSink;
use wcore_agent::resilient_reporter::ProtocolCircuitReporter;
use wcore_providers::{
    CandidateCapabilities, CircuitConfig, FailoverCandidateMetadata, FailoverRoutingPolicy,
    LlmProvider, PricingEvidence, ProviderError, ResilientProvider,
};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::FinishReason;

#[derive(Default)]
struct InfoRecorder {
    infos: Mutex<Vec<String>>,
}

impl OutputSink for InfoRecorder {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, message: &str) {
        self.infos.lock().unwrap().push(message.to_string());
    }
    fn emit_provider_circuit_event(&self, _: &str, _: Option<&str>, _: &str, _: Option<&str>) {}
}

/// A primary that fails the way the ticket's user's did: the port is closed.
struct FailingPrimary {
    error: fn() -> ProviderError,
}

#[async_trait]
impl LlmProvider for FailingPrimary {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        Err((self.error)())
    }
}

/// A fallback that answers, so the chain reaches the SELECTED branch — the
/// only one that renders a failover notice.
struct HealthyFallback;

#[async_trait]
impl LlmProvider for HealthyFallback {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let (_tx, rx) = mpsc::channel(1);
        Ok(rx)
    }
}

fn candidate() -> FailoverCandidateMetadata {
    FailoverCandidateMetadata {
        label: "openai:gpt-5".into(),
        provider: "openai".into(),
        model: "gpt-5".into(),
        organization: None,
        region: None,
        capabilities: CandidateCapabilities {
            tools: true,
            vision: true,
            structured_output: true,
            context_window: Some(400_000),
        },
        pricing: PricingEvidence {
            source: "bundled".into(),
            age_seconds: Some(0),
            stale: false,
            priced: true,
            estimated_microcents: Some(10),
        },
    }
}

/// Run one failover through the real wrapper + the real reporter and return
/// every `emit_info` line the sink saw.
async fn notices_for(error: fn() -> ProviderError) -> Vec<String> {
    let sink = Arc::new(InfoRecorder::default());
    let reporter = Arc::new(ProtocolCircuitReporter::new(
        sink.clone() as Arc<dyn OutputSink>
    ));
    let provider = ResilientProvider::new_with_policy(
        "anthropic",
        Arc::new(FailingPrimary { error }) as Arc<dyn LlmProvider>,
        vec![(
            candidate(),
            Arc::new(HealthyFallback) as Arc<dyn LlmProvider>,
        )],
        CircuitConfig::default(),
        reporter,
        FailoverRoutingPolicy::default(),
    );
    let request = LlmRequest {
        model: "claude-sonnet-4-6".into(),
        max_tokens: 256,
        ..Default::default()
    };
    provider
        .stream(&request)
        .await
        .expect("the healthy fallback must serve the turn");
    // Cloned out: the caller asserts on the lines, never under the lock.
    sink.infos.lock().unwrap().clone()
}

fn refused() -> ProviderError {
    ProviderError::Connection(
        "error sending request for url (http://127.0.0.1:9/v1/messages): client error \
         (Connect): tcp connect error: Connection refused (os error 111)"
            .into(),
    )
}

fn throttled() -> ProviderError {
    ProviderError::RateLimited {
        retry_after_ms: 1_500,
    }
}

/// The notice must carry the fault CLASS of the primary, which is the fact a
/// user loses by configuring a chain: with no chain they read "Connection
/// refused (os error 111)"; with one they read a list of things to check.
#[tokio::test]
async fn the_failover_notice_names_the_primary_failure_class() {
    let said = notices_for(refused).await;
    let notice = said
        .iter()
        .find(|line| line.contains("provider failover"))
        .unwrap_or_else(|| panic!("no failover notice was emitted at all: {said:?}"));

    // Instrument liveness: the notice IS present and IS the #1133 sentence, so
    // the assertion below is an absence in the sentence, not a dead sink.
    for fact in ["anthropic", "claude-sonnet-4-6", "openai", "gpt-5"] {
        assert!(
            notice.contains(fact),
            "instrument dead: the notice does not even name `{fact}`: {notice}"
        );
    }

    assert!(
        notice.contains("connection_refused"),
        "the notice does not say WHY the primary was skipped. The class was already \
         in hand — `ResilientProvider` classified it into `primary_failure_code` in \
         the same function that reported this failover — and without it the user \
         gets a pointer ('check base_url, credentials and quota') where an \
         unchained run would have given them the diagnosis: {notice}"
    );
}

/// CAN-FAIL half of the same instrument: the class must be READ from the
/// failure, not printed as a constant. A throttled primary is a different
/// class and must render differently — and it must not render the refused
/// one. Without this, a notice that hardcoded `connection_refused` would pass
/// the test above.
#[tokio::test]
async fn a_different_primary_fault_renders_a_different_class() {
    let said = notices_for(throttled).await;
    let notice = said
        .iter()
        .find(|line| line.contains("provider failover"))
        .unwrap_or_else(|| panic!("no failover notice was emitted at all: {said:?}"));

    assert!(
        notice.contains("http_429"),
        "a rate-limited primary must be named as such: {notice}"
    );
    assert!(
        !notice.contains("connection_refused"),
        "the notice printed a class the primary never produced: {notice}"
    );
}
