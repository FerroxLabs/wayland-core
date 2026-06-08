//! F-003 integration test: init_history text lands in the engine system prompt.
//!
//! Verifies the fix for TRIAGE.md F-003 (CRIT): the app ships Constitution +
//! skills index + persona via the `init_history` ProtocolCommand, but the
//! engine was previously a silent `eprintln!`-drop. `AgentEngine::inject_history`
//! now routes the text into `self.system_prompt`; this test proves the injected
//! content is visible in the `LlmRequest.system` field the provider actually sees.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

// ---------------------------------------------------------------------------
// CapturingProvider — records every LlmRequest it receives
// ---------------------------------------------------------------------------

struct CapturingProvider {
    captured: Mutex<Vec<LlmRequest>>,
}

impl CapturingProvider {
    fn new() -> Self {
        Self {
            captured: Mutex::new(Vec::new()),
        }
    }

    fn last_system(&self) -> Option<String> {
        self.captured
            .lock()
            .unwrap()
            .last()
            .map(|r| r.system.clone())
    }
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.captured.lock().unwrap().push(request.clone());

        let events = vec![
            LlmEvent::TextDelta("ok".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
            },
        ];
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            for e in events {
                let _ = tx.send(e).await;
            }
        });
        Ok(rx)
    }
}

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

// ---------------------------------------------------------------------------
// F-003: inject_history text visible in LlmRequest.system on first turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inject_history_lands_in_system_prompt() {
    let provider = Arc::new(CapturingProvider::new());
    let provider_ref = provider.clone();

    let config = test_config(); // includes base system_prompt "You are a test assistant."
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);

    // Simulate the app's init_history frame (Constitution + persona).
    let injected =
        "## Constitution\nYou are a Wayland assistant with specialized skills.".to_string();
    engine.inject_history(injected.clone());

    // Run a turn — the provider captures the LlmRequest.
    engine
        .run("hello", "msg-1")
        .await
        .expect("turn should succeed");

    let system = provider_ref
        .last_system()
        .expect("provider should have been called");
    assert!(
        system.contains("Constitution"),
        "injected Constitution must appear in system prompt; got: {system}"
    );
    assert!(
        system.contains("Wayland assistant"),
        "injected persona text must appear in system prompt; got: {system}"
    );
    // The engine's own system prompt must still be present (inject prepends, doesn't replace).
    assert!(
        system.contains("test assistant"),
        "engine's original system prompt must still be present; got: {system}"
    );
}

#[tokio::test]
async fn inject_history_into_empty_system_prompt() {
    let provider = Arc::new(CapturingProvider::new());
    let provider_ref = provider.clone();

    let mut config = test_config();
    config.system_prompt = None; // engine starts with empty system prompt

    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);

    engine.inject_history("Only injected text.".to_string());
    engine
        .run("hi", "msg-2")
        .await
        .expect("turn should succeed");

    let system = provider_ref.last_system().expect("provider called");
    assert_eq!(
        system, "Only injected text.",
        "empty system_prompt: injected text should be the only content; got: {system}"
    );
}

#[tokio::test]
async fn inject_history_twice_accumulates() {
    let provider = Arc::new(CapturingProvider::new());
    let provider_ref = provider.clone();

    let mut config = test_config();
    config.system_prompt = None;

    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);

    engine.inject_history("Frame A".to_string());
    engine.inject_history("Frame B".to_string());
    engine
        .run("hi", "msg-3")
        .await
        .expect("turn should succeed");

    let system = provider_ref.last_system().expect("provider called");
    assert!(
        system.contains("Frame A"),
        "first inject must be present; got: {system}"
    );
    assert!(
        system.contains("Frame B"),
        "second inject must be present; got: {system}"
    );
}
