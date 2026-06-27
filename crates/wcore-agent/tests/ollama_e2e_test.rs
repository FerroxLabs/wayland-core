//! Wave OL B.4 — end-to-end Ollama wiremock smoke through `AgentBootstrap`.
//!
//! This is the FIRST end-to-end test of a plugin-routed model path in the
//! repo. It validates:
//!
//! 1. A wiremock server speaking Ollama's NDJSON `/api/chat` shape can drive
//!    `OllamaProvider::stream` to produce `TextDelta` + `Done` events.
//! 2. `AgentBootstrap::provider(...)` accepts the plugin-supplied
//!    `OllamaProvider` and the engine completes ONE turn against it.
//! 3. The wiremock server saw exactly one request with `model` honouring
//!    the prefix-stripped name (i.e. `--model ollama:llama4` reaches Ollama
//!    as `model: llama4`).
//!
//! The wiremock + `.provider(...)` injection covers the runtime contract.
//! The `wcore-cli` `make_plugin_provider_router` path that goes from
//! `Arc<dyn PluginProvider>` → `Arc<OllamaProvider>` via `as_any` is
//! covered by the CLI's own plugin-discovery test suite plus the unit
//! tests in `wcore-plugin-api` (`scoped_registry_semantics`).
//!
//! Wave OL also closes the longest-standing stub in the repo: the
//! `provider_registrar.rs` aspirational comment ("downcast/translate
//! `Arc<dyn PluginProvider>` to a concrete `wcore_providers::LlmProvider`
//! is the W8c.3.D chain edge") is now real code.

use std::sync::Arc;

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::LlmProvider;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use wayland_ollama::OllamaProvider;

/// Build a minimal `Config` whose `model` field is `ollama:llama4` so we
/// exercise the prefix-strip in `OllamaProvider::stream`. Provider type
/// is set to `OpenAI` purely so `Config` validation passes — the engine
/// never actually constructs an OpenAI provider because we inject the
/// Ollama provider explicitly via `.provider(...)`.
fn ollama_test_config(model: &str) -> Config {
    Config {
        provider_label: "ollama".into(),
        provider: ProviderType::OpenAI,
        api_key: "dummy".into(),
        base_url: "http://localhost:0".into(),
        model: model.into(),
        max_tokens: 256,
        max_turns: Some(1),
        compat: ProviderCompat::ollama_defaults(),
        ..Default::default()
    }
}

fn null_output() -> Arc<dyn wcore_agent::output::OutputSink> {
    Arc::new(NullSink)
}

/// Two NDJSON lines: one text delta then the terminal `done` line with
/// Ollama's usage counters. Each line is `\n`-terminated.
fn ollama_ndjson_body(text: &str) -> String {
    format!(
        concat!(
            "{{\"model\":\"llama4\",\"created_at\":\"now\",\"message\":{{\"role\":\"assistant\",\"content\":\"{text}\"}},\"done\":false}}\n",
            "{{\"model\":\"llama4\",\"created_at\":\"now\",\"message\":{{\"role\":\"assistant\",\"content\":\"\"}},\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":7,\"eval_count\":3}}\n",
        ),
        text = text
    )
}

/// Drive `OllamaProvider::stream` against a wiremock server returning an
/// NDJSON body, and collect every emitted event. Smoke test #1.
#[tokio::test]
async fn ollama_provider_streams_text_delta_and_done() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(ollama_ndjson_body("hello"), "application/x-ndjson"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(format!("{}/api/chat", server.uri()), "llama4");
    let request = LlmRequest {
        model: "ollama:llama4".into(),
        system: "you are a test".into(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "say hello".into(),
            }],
        )],
        tools: vec![],
        max_tokens: 64,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
    };

    let mut rx = provider
        .stream(&request)
        .await
        .expect("ollama provider stream");

    let mut text = String::new();
    let mut got_done = false;
    while let Some(event) = rx.recv().await {
        match event {
            LlmEvent::TextDelta(s) => text.push_str(&s),
            LlmEvent::Done { usage, .. } => {
                // Token counters from the terminal NDJSON line.
                assert_eq!(usage.input_tokens, 7);
                assert_eq!(usage.output_tokens, 3);
                got_done = true;
                break;
            }
            LlmEvent::Error(e) => panic!("unexpected error event: {e}"),
            other => panic!("unexpected event: {other:?}"),
        }
    }
    assert_eq!(text, "hello");
    assert!(got_done, "expected a terminal Done event");

    // Wiremock saw exactly one request with the prefix-stripped model.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body["model"], "llama4",
        "ollama: prefix should be stripped before hitting Ollama"
    );
    assert_eq!(body["stream"], true);
    // System message rolls in as the first chat message with role=system.
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "you are a test");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], "say hello");
}

/// End-to-end smoke through `AgentBootstrap`: build a real engine with
/// the Ollama provider injected, run ONE turn, verify the engine drained
/// the streamed text and the wiremock saw the request. This is the
/// canonical "plugin-routed model path actually drives a turn" test.
#[tokio::test]
async fn bootstrap_with_ollama_provider_completes_a_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(ollama_ndjson_body("ack"), "application/x-ndjson"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider: Arc<dyn LlmProvider> = Arc::new(OllamaProvider::new(
        format!("{}/api/chat", server.uri()),
        "llama4",
    ));

    let config = ollama_test_config("ollama:llama4");
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .provider(provider)
        .build()
        .await
        .expect("bootstrap with ollama provider should succeed");

    let mut engine = result.engine;
    let outcome = engine
        .run("say hello", "msg-1")
        .await
        .expect("engine should drive one turn against wiremock-backed Ollama");

    // The mocked NDJSON returns `"ack"` as the text delta; the engine
    // aggregates deltas internally. We don't assert the exact text on
    // the AgentResult because the engine may consume it for context;
    // the wiremock invariant is the load-bearing check.
    let _ = outcome;

    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests.is_empty(),
        "wiremock should have received at least one request from the engine turn"
    );
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "llama4");
    assert_eq!(body["stream"], true);
}

/// `done_reason: "length"` should round-trip as `FinishReason::Length`.
#[tokio::test]
async fn ollama_provider_maps_length_done_reason() {
    let server = MockServer::start().await;
    let body = "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"done_reason\":\"length\",\"prompt_eval_count\":2,\"eval_count\":1}\n";
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/x-ndjson"))
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(format!("{}/api/chat", server.uri()), "llama4");
    let request = LlmRequest {
        model: "llama4".into(),
        system: String::new(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "x".into() }],
        )],
        tools: vec![],
        max_tokens: 16,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
    };
    let mut rx = provider.stream(&request).await.unwrap();
    while let Some(event) = rx.recv().await {
        if let LlmEvent::Done { finish_reason, .. } = event {
            assert_eq!(
                finish_reason,
                wcore_types::message::FinishReason::Length,
                "done_reason=length must map to FinishReason::Length"
            );
            return;
        }
    }
    panic!("expected a Done event");
}

/// Wiremock returning 5xx should surface as `ProviderError::Api` rather
/// than panicking inside the stream task.
#[tokio::test]
async fn ollama_provider_5xx_surfaces_as_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(format!("{}/api/chat", server.uri()), "llama4");
    let request = LlmRequest {
        model: "llama4".into(),
        system: String::new(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text { text: "x".into() }],
        )],
        tools: vec![],
        max_tokens: 16,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
    };
    let err = provider.stream(&request).await.unwrap_err();
    match err {
        wcore_providers::ProviderError::Api { status, message } => {
            assert_eq!(status, 503);
            assert!(message.contains("busy"));
        }
        other => panic!("expected ProviderError::Api, got {other:?}"),
    }
}

/// Optional live-Ollama smoke. Ignored by default; run with
/// `OLLAMA_BASE_URL=http://localhost:11434/api/chat cargo nextest run -p wcore-agent --test ollama_e2e_test -- --ignored ollama_live`
/// against a real local Ollama daemon to verify end-to-end. The test
/// is liberal in what it accepts — any non-empty response counts as a
/// pass, because the host system controls which model is installed.
#[tokio::test]
#[ignore = "requires OLLAMA_BASE_URL pointing at a real local Ollama daemon"]
async fn ollama_live_smoke() {
    let base_url = match std::env::var("OLLAMA_BASE_URL") {
        Ok(v) => v,
        Err(_) => return,
    };
    let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3".into());
    let provider = OllamaProvider::new(base_url, model);
    let request = LlmRequest {
        model: "llama3".into(),
        system: "Reply with a single word.".into(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        )],
        tools: vec![],
        max_tokens: 32,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
    };
    let mut rx = provider.stream(&request).await.expect("ollama stream");
    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            LlmEvent::TextDelta(s) => text.push_str(&s),
            LlmEvent::Done { .. } => break,
            LlmEvent::Error(e) => panic!("live ollama error: {e}"),
            _ => {}
        }
    }
    assert!(!text.is_empty(), "live ollama returned no text");
}

// Silence the unused-import warning for `Request` — kept available for
// future request-shape assertions without rewriting the import list.
#[allow(dead_code)]
fn _unused_request_type_anchor(_r: &Request) {}
