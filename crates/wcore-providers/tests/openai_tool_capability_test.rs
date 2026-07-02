//! Live-HTTP integration tests for the capability-first tools gate
//! (#389 / #97 follow-up), driving the real `OpenAIProvider::stream` path
//! against a `wiremock` backend.
//!
//! Two behaviors are covered end-to-end over real HTTP:
//!
//! 1. **Reactive retry** — a backend (llama.cpp without `--jinja`) 400s a
//!    request that carries a `tools` array, using its real wording. The
//!    provider must drop the array and retry once so the turn completes,
//!    instead of surfacing the raw 400. This is the only line of defense for
//!    backends with no capability endpoint to probe (llama.cpp).
//! 2. **Proactive Ollama probe** — for an Ollama-served model whose
//!    `/api/show` reports no `tools` capability, the provider must strip the
//!    `tools` array BEFORE sending the chat request (no failed round-trip).

use serde_json::json;
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::openai::OpenAIProvider;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role};
use wcore_types::tool::ToolDef;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A request carrying one function tool, for `model`.
fn request_with_tools(model: &str) -> LlmRequest {
    LlmRequest {
        model: model.to_string(),
        system: "test".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        )],
        tools: vec![ToolDef {
            name: "get_time".to_string(),
            description: "Get the current time".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            deferred: false,
            server: None,
        }],
        max_tokens: 256,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
        omit_max_tokens: false,
    }
}

async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

/// A minimal, valid OpenAI-style SSE completion that yields a single
/// `TextDelta("ok")` followed by a `Done`.
fn ok_sse() -> String {
    let mut body = String::new();
    body.push_str("data: ");
    body.push_str(r#"{"choices":[{"delta":{"content":"ok"},"index":0}]}"#);
    body.push_str("\n\n");
    body.push_str("data: ");
    body.push_str(
        r#"{"choices":[{"delta":{},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
    );
    body.push_str("\n\n");
    body.push_str("data: [DONE]\n\n");
    body
}

// ---------------------------------------------------------------------------
// Reactive retry (llama.cpp real error wording)
// ---------------------------------------------------------------------------

/// A request with tools hits a backend that 400s with llama.cpp's real
/// no-`--jinja` wording; the provider must drop tools and retry so the turn
/// completes. Exercises the #97 marker + #389 retry over real HTTP.
#[tokio::test]
async fn llamacpp_tools_unsupported_400_triggers_reactive_retry() {
    let server = MockServer::start().await;

    // First attempt carries `tools` → 400 with the real llama.cpp message.
    // Higher priority (1) so it wins over the no-tools mock for this request.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"tools\""))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"tools param requires --jinja flag","type":"invalid_request_error"}}"#,
        ))
        .with_priority(1)
        .mount(&server)
        .await;

    // The reactive retry drops `tools`; this mock (default priority 5) answers
    // it with a normal streamed completion.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ok_sse(), "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );

    let rx = provider
        .stream(&request_with_tools("gpt-4o"))
        .await
        .expect("stream() must succeed via the reactive retry, not surface the 400");
    let events = collect_events(rx).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "ok")),
        "expected the retry's text output; got {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::Done { .. })),
        "expected a Done event after the reactive retry; got {events:?}"
    );

    // Two chat requests reached the wire: the tools attempt (400) + the retry.
    let reqs = server.received_requests().await.expect("recorded requests");
    let chat_calls = reqs
        .iter()
        .filter(|r| r.url.path() == "/v1/chat/completions")
        .count();
    assert_eq!(
        chat_calls, 2,
        "expected exactly two chat calls (tools attempt + no-tools retry)"
    );
}

// ---------------------------------------------------------------------------
// Proactive Ollama probe
// ---------------------------------------------------------------------------

/// For an Ollama model whose `/api/show` reports no `tools` capability, the
/// provider must strip the `tools` array BEFORE sending the chat request, so
/// the backend never sees an unsupported field. Exercises the probe + cache +
/// request-builder gate over real HTTP.
#[tokio::test]
async fn ollama_probe_strips_tools_before_request_for_no_tool_model() {
    let server = MockServer::start().await;

    // `/api/show` reports a model WITHOUT "tools" capability → probe ⇒ strip.
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "capabilities": ["completion"]
        })))
        .mount(&server)
        .await;

    // Chat endpoint answers normally; we assert below it received NO tools.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ok_sse(), "text/event-stream"))
        .mount(&server)
        .await;

    // Ollama provider: api_path defaults to `/v1/chat/completions`, so the base
    // URL is bare (no `/v1`); the probe derives `{base}/api/show`.
    let provider = OpenAIProvider::new(
        "ollama",
        &server.uri(),
        ProviderCompat::ollama_defaults(),
        DebugConfig::default(),
    );

    let rx = provider
        .stream(&request_with_tools("smollm2:135m"))
        .await
        .expect("stream() must succeed");
    let _ = collect_events(rx).await;

    let reqs = server.received_requests().await.expect("recorded requests");

    // The probe ran.
    assert!(
        reqs.iter().any(|r| r.url.path() == "/api/show"),
        "the Ollama /api/show probe must have been called"
    );

    // The chat request carried NO `tools` array (stripped pre-emptively).
    let chat = reqs
        .iter()
        .find(|r| r.url.path() == "/v1/chat/completions")
        .expect("a chat request must have been sent");
    let body: serde_json::Value = serde_json::from_slice(&chat.body).expect("chat body is JSON");
    assert!(
        body.get("tools").is_none(),
        "the probe must have stripped `tools` BEFORE the request; body was {body}"
    );
}

/// The inverse of the strip test: an Ollama model whose `/api/show` reports
/// `tools` support must KEEP its tools — the gate is capability-aware, not
/// blind. (Guards against a regression that strips tools from capable models.)
#[tokio::test]
async fn ollama_probe_keeps_tools_for_tool_capable_model() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "capabilities": ["completion", "tools"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ok_sse(), "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "ollama",
        &server.uri(),
        ProviderCompat::ollama_defaults(),
        DebugConfig::default(),
    );

    let rx = provider
        .stream(&request_with_tools("llama3.2:1b"))
        .await
        .expect("stream() must succeed");
    let _ = collect_events(rx).await;

    let reqs = server.received_requests().await.expect("recorded requests");
    let chat = reqs
        .iter()
        .find(|r| r.url.path() == "/v1/chat/completions")
        .expect("a chat request must have been sent");
    let body: serde_json::Value = serde_json::from_slice(&chat.body).expect("chat body is JSON");
    assert!(
        body.get("tools")
            .and_then(|t| t.as_array())
            .is_some_and(|a| a.len() == 1),
        "a tool-capable model must KEEP its tools; body was {body}"
    );
}

/// A failed probe must fail OPEN: tools stay attached (optimistic), and the
/// turn proceeds — the reactive net is the backstop if the model truly can't
/// do tools.
#[tokio::test]
async fn ollama_probe_failure_keeps_tools_optimistically() {
    let server = MockServer::start().await;

    // Probe endpoint errors → capability unknown.
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ok_sse(), "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "ollama",
        &server.uri(),
        ProviderCompat::ollama_defaults(),
        DebugConfig::default(),
    );

    let rx = provider
        .stream(&request_with_tools("mystery-model"))
        .await
        .expect("stream() must succeed despite a failed probe");
    let _ = collect_events(rx).await;

    let reqs = server.received_requests().await.expect("recorded requests");
    let chat = reqs
        .iter()
        .find(|r| r.url.path() == "/v1/chat/completions")
        .expect("a chat request must have been sent");
    let body: serde_json::Value = serde_json::from_slice(&chat.body).expect("chat body is JSON");
    assert!(
        body.get("tools").is_some(),
        "a failed probe must leave tools attached (fail-open); body was {body}"
    );
}

/// Cache-learning across turns: after a reactive tools-unsupported 400 on turn
/// 1, the SAME provider must drop tools PRE-EMPTIVELY on turn 2 — so only the
/// very first request ever carries a `tools` array. This is the mechanism that
/// covers backends with no probe endpoint (e.g. llama.cpp) on subsequent turns.
#[tokio::test]
async fn reactive_400_is_remembered_and_strips_tools_on_next_turn() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"tools\""))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"tools param requires --jinja flag","type":"invalid_request_error"}}"#,
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ok_sse(), "text/event-stream"))
        .mount(&server)
        .await;

    // Plain OpenAI provider (no probe) so we isolate the reactive-learning path.
    let provider = OpenAIProvider::new(
        "key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );

    // Turn 1: tools attached → 400 → reactive retry → completes; cache learns.
    let rx = provider
        .stream(&request_with_tools("gpt-4o"))
        .await
        .expect("turn 1 must complete via reactive retry");
    let _ = collect_events(rx).await;

    // Turn 2: same provider/model → tools must be stripped pre-emptively.
    let rx = provider
        .stream(&request_with_tools("gpt-4o"))
        .await
        .expect("turn 2 must complete");
    let _ = collect_events(rx).await;

    let reqs = server.received_requests().await.expect("recorded requests");
    let with_tools = reqs
        .iter()
        .filter(|r| r.url.path() == "/v1/chat/completions")
        .filter(|r| {
            serde_json::from_slice::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|b| b.get("tools").cloned())
                .is_some()
        })
        .count();
    assert_eq!(
        with_tools, 1,
        "only turn 1's first attempt may carry tools; turn 2 must strip them pre-emptively"
    );
}
