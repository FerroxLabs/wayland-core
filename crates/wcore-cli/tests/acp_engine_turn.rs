//! T-A8 / T-B2 — the "prompt streams real engine output" proof for the ACP
//! engine bridge.
//!
//! These tests build a real `EngineTurnEngine` whose engine is backed by a
//! `MockLlm`-bound `AnthropicProvider` (injected via
//! `EngineTurnEngine::with_provider`, the hermetic test seam), then drive a
//! turn through the SAME `EngineSession` the production ACP/A2A paths use and
//! assert the projected `MessageEvent` stream.
//!
//! Marked `#[ignore]` so they stay out of the default lane — they spin a live
//! socket + a full engine bootstrap. Run in the controlled foreground pass:
//! `cargo test -p wcore-cli --test acp_engine_turn -- --ignored`.

#[path = "support/mod.rs"]
mod support;

use std::sync::Arc;

use support::mock_llm::MockLlm;

use wcore_acp::protocol::MessageEvent;
use wcore_acp::turn::{TurnEngine, TurnRequest};
use wcore_cli::acp_engine::EngineTurnEngine;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{CliArgs, Config};
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;

use futures::stream::StreamExt;

/// Resolve a minimal Config. A dummy api key lets resolution succeed; the
/// real provider is never built because we inject one.
fn test_config() -> Config {
    Config::resolve(&CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("sk-ant-harness-not-real-key".to_string()),
        base_url: None,
        model: Some("claude-mock".to_string()),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: true,
        project_dir: None,
    })
    .expect("resolve a default config")
}

/// Build the Anthropic provider pointed at the mock server's base_url.
fn provider_against(base_url: &str) -> Arc<dyn LlmProvider> {
    Arc::new(
        AnthropicProvider::new(
            "sk-ant-harness-not-real-key",
            base_url,
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false),
    )
}

/// T-A8: a prompt over the ACP bridge streams the mock's text then a clean
/// terminal `Done`. This is the literal "process_message streams real engine
/// output" proof.
#[tokio::test]
#[ignore = "spins a live socket + full engine bootstrap; run in the foreground pass"]
async fn acp_turn_streams_text_then_done() {
    let mock = MockLlm::new().text("Hello from the mock!");
    let server = mock.start().await;
    let provider = provider_against(&server.uri());

    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let engine = EngineTurnEngine::with_provider(test_config(), cwd, provider);

    let stream = engine
        .run_turn(TurnRequest {
            session_id: "11111111-2222-3333-4444-aaaaaaaaaaaa".to_string(),
            text: "say hi".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("run_turn establishes a stream");
    let frames: Vec<MessageEvent> = stream.collect().await;

    // At least one TextDelta carrying the mock's text.
    let text: String = frames
        .iter()
        .filter_map(|e| match e {
            MessageEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("Hello from the mock!"),
        "expected the mock text in the stream; frames: {frames:?}"
    );

    // Exactly one terminal frame, last, and it is a clean Done (end_turn).
    let terminals = frames
        .iter()
        .filter(|e| matches!(e, MessageEvent::Done { .. } | MessageEvent::Error { .. }))
        .count();
    assert_eq!(
        terminals, 1,
        "exactly one terminal frame; frames: {frames:?}"
    );
    match frames.last().expect("a terminal frame") {
        MessageEvent::Done { stop_reason } => assert_eq!(stop_reason, "end_turn"),
        other => panic!("expected a clean Done last, got {other:?}"),
    }
}

/// T-B2: the engine-backed A2A handler routes a task through the SAME bridge
/// and returns the engine's reply text (NOT an `ack:` echo).
#[tokio::test]
#[ignore = "spins a live socket + full engine bootstrap; run in the foreground pass"]
async fn a2a_on_message_routes_task_to_engine() {
    use wcore_acp::a2a::{A2aHandler, A2aMessage};
    use wcore_cli::acp_engine::EngineA2aHandler;

    let mock = MockLlm::new().text("pong from engine");
    let server = mock.start().await;
    let provider = provider_against(&server.uri());

    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    // The engine-backed A2A handler wraps an EngineTurnEngine; use the
    // provider-injecting variant so the turn is hermetic.
    let handler = EngineA2aHandler::with_engine(EngineTurnEngine::with_provider(
        test_config(),
        cwd,
        provider,
    ));

    let reply = handler
        .on_message(A2aMessage {
            from: "peer".to_string(),
            to: "wayland-core".to_string(),
            text: "ping".to_string(),
            attachments: vec![],
            correlation_id: Some("corr-1".to_string()),
        })
        .await
        .expect("engine-backed on_message succeeds");

    assert!(
        reply.text.contains("pong from engine"),
        "reply must carry the engine output, not an echo; got {:?}",
        reply.text
    );
    assert_eq!(reply.to, "peer", "reply addressed back to the sender");
    assert_eq!(reply.correlation_id, Some("corr-1".to_string()));
}
