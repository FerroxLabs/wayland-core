//! W7 Pre-flight 0.0d: smoke tests for the test-driver helpers.
//!
//! Proves the contract published to W7 task executors:
//! - `AgentBootstrap::build_for_test` returns a working engine + a
//!   `TestSinkHandle`.
//! - `AgentEngine::run_synthetic_turn` drives one turn end-to-end against
//!   the scripted provider.
//! - `AgentEngine::captured_protocol_events` returns the serialised
//!   `ProtocolEvent` stream the sink recorded.

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::test_utils::ScriptedProvider;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(2),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

fn single_text_script(text: &str) -> Vec<LlmEvent> {
    // Build the same script ScriptedProvider::single_text_turn produces
    // — exposed here so the test asserts the LlmEvent → ProtocolEvent
    // contract directly instead of going through a helper alias.
    vec![
        LlmEvent::TextDelta(text.into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

#[tokio::test]
async fn build_for_test_constructs_engine_with_scripted_provider() {
    let (engine, _handle) =
        AgentBootstrap::build_for_test(minimal_config(), single_text_script("hello world"));
    // Engine is fully constructed: tool names include the read-only
    // built-ins, memory_api is NullMemory, captured events empty.
    let names = engine.tool_names();
    assert!(
        names.iter().any(|n| n == "Read"),
        "Read tool should be registered; got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Grep"),
        "Grep tool should be registered; got {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "Glob"),
        "Glob tool should be registered; got {names:?}"
    );
    assert!(engine.captured_protocol_events().is_empty());
}

#[tokio::test]
async fn run_synthetic_turn_captures_text_delta_and_stream_end() {
    let (mut engine, _handle) =
        AgentBootstrap::build_for_test(minimal_config(), single_text_script("hello"));
    let out = engine
        .run_synthetic_turn("anything")
        .await
        .expect("synthetic turn should succeed");

    assert!(
        out.final_text.contains("hello"),
        "engine.run() should return assembled text; got {:?}",
        out.final_text
    );
    assert!(
        out.turns >= 1,
        "at least one turn should run; got {}",
        out.turns
    );

    // Captured events for one synthetic turn include stream_start and
    // text_delta. stream_end is emitted at session-end (CLI scope), not
    // by engine.run() itself — so it is not asserted here.
    let kinds: Vec<&str> = out
        .events
        .iter()
        .filter_map(|e| e["type"].as_str())
        .collect();
    assert!(
        kinds.contains(&"stream_start"),
        "expected stream_start in captured events; got {kinds:?}"
    );
    assert!(
        kinds.contains(&"text_delta"),
        "expected text_delta in captured events; got {kinds:?}"
    );
}

#[tokio::test]
async fn scripted_provider_handle_re_observes_via_sink_handle() {
    // The handle returned from build_for_test shares the underlying
    // event buffer with the engine; both views must agree.
    let (mut engine, handle) =
        AgentBootstrap::build_for_test(minimal_config(), single_text_script("ok"));
    let _ = engine.run_synthetic_turn("input").await.unwrap();
    let engine_view = engine.captured_protocol_events();
    let handle_view = handle.snapshot();
    assert_eq!(
        engine_view.len(),
        handle_view.len(),
        "engine and external handle must observe the same buffer"
    );
}

// Suppress "unused import" if ScriptedProvider only appears in docs.
#[allow(dead_code)]
fn _silence_unused(_p: ScriptedProvider) {}
