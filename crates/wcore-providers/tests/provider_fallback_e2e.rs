//! E5 scenario 2 — ProviderChain fallback.
//!
//! Build a ProviderChain of [failing_provider, succeeding_provider].
//! Call `.stream()`. Assert:
//!   - The call succeeds (slot 1 answered).
//!   - Slot 0 was attempted exactly once (error captured, not propagated).
//!   - Slot 1 was attempted exactly once.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_providers::chain::ProviderChain;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

struct CountedProvider {
    result: fn() -> Result<mpsc::Receiver<LlmEvent>, ProviderError>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for CountedProvider {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self.result)()
    }
}

fn ok_provider(calls: Arc<AtomicUsize>) -> Arc<dyn LlmProvider> {
    Arc::new(CountedProvider {
        result: || {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        },
        calls,
    })
}

fn connection_err_provider(calls: Arc<AtomicUsize>) -> Arc<dyn LlmProvider> {
    Arc::new(CountedProvider {
        result: || Err(ProviderError::Connection("mock provider down".into())),
        calls,
    })
}

fn dummy_request() -> LlmRequest {
    LlmRequest {
        model: "test-model".into(),
        system: String::new(),
        messages: vec![],
        tools: vec![],
        max_tokens: 1,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chain_falls_back_to_slot1_when_slot0_fails() {
    let slot0_calls = Arc::new(AtomicUsize::new(0));
    let slot1_calls = Arc::new(AtomicUsize::new(0));

    let chain = ProviderChain::new(vec![
        ("failing", connection_err_provider(slot0_calls.clone())),
        ("succeeding", ok_provider(slot1_calls.clone())),
    ]);

    // Must succeed via slot 1.
    chain
        .stream(&dummy_request())
        .await
        .expect("chain should succeed via slot 1");

    // Slot 0 tried once, error captured — not propagated.
    assert_eq!(
        slot0_calls.load(Ordering::SeqCst),
        1,
        "slot 0 must be attempted exactly once"
    );

    // Slot 1 picked up the request.
    assert_eq!(
        slot1_calls.load(Ordering::SeqCst),
        1,
        "slot 1 must be attempted exactly once"
    );
}

#[tokio::test]
async fn chain_exhausted_returns_error_not_panic() {
    let chain = ProviderChain::new(vec![
        ("p1", connection_err_provider(Arc::new(AtomicUsize::new(0)))),
        ("p2", connection_err_provider(Arc::new(AtomicUsize::new(0)))),
    ]);

    let err = chain
        .stream(&dummy_request())
        .await
        .expect_err("both fail — must return Err");

    match err {
        ProviderError::Connection(msg) => {
            assert!(
                msg.contains("2 provider(s)"),
                "error must mention attempt count; got: {msg}"
            );
        }
        other => panic!("expected Connection error, got {other:?}"),
    }
}
