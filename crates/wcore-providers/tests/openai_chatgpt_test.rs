//! Integration tests for [`OpenAIChatGptProvider`] — the ChatGPT Codex OAuth
//! provider. Mirrors the Responses-parser stream tests in
//! `provider_openai_test.rs`, pointing `base_url` at a wiremock server that
//! serves a Codex SSE fixture (a `response.output_text.delta` followed by the
//! Codex terminal `response.completed` frame).

use std::sync::Arc;

use serde_json::json;
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::openai_chatgpt::{AsyncBearerSource, BearerCreds};
use wcore_providers::{LlmProvider, OpenAIChatGptProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role, StopReason};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A bearer source that returns fixed creds without any network round-trip.
fn static_bearer(access_token: &str, account_id: &str) -> AsyncBearerSource {
    let creds = BearerCreds {
        access_token: access_token.to_string(),
        account_id: account_id.to_string(),
    };
    Arc::new(move || {
        let creds = creds.clone();
        Box::pin(async move { Ok(creds) })
    })
}

/// A bearer source that always fails — proves an OAuth refresh error surfaces
/// before any HTTP request is attempted.
fn failing_bearer() -> AsyncBearerSource {
    Arc::new(|| Box::pin(async { Err(ProviderError::Connection("no token".into())) }))
}

fn make_request() -> LlmRequest {
    LlmRequest {
        model: "gpt-5.5".to_string(),
        system: "You are a test assistant.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )],
        max_tokens: 512,
        ..Default::default()
    }
}

async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

/// Build a Codex Responses SSE body from typed JSON frames. The Responses
/// stream has no `[DONE]` sentinel — the terminal frame is the success frame.
fn build_responses_sse(frames: &[&str]) -> String {
    let mut body = String::new();
    for f in frames {
        body.push_str("data: ");
        body.push_str(f);
        body.push_str("\n\n");
    }
    body
}

/// Happy path: a text delta followed by the terminal `response.completed`
/// frame yields `TextDelta` then `Done{EndTurn}`. Also asserts the Codex
/// headers (account id, beta, originator, accept) reach the server.
#[tokio::test]
async fn chatgpt_stream_text_then_done() {
    let server = MockServer::start().await;

    let delta = r#"{"type":"response.output_text.delta","delta":"Hello, world!"}"#;
    let completed = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":12,"output_tokens":5}}}"#;
    let sse_body = build_responses_sse(&[delta, completed]);

    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer tok-abc"))
        .and(header("chatgpt-account-id", "acct_42"))
        .and(header("openai-beta", "responses=experimental"))
        .and(header("originator", "wayland"))
        .and(header("accept", "text/event-stream"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIChatGptProvider::new(
        static_bearer("tok-abc", "acct_42"),
        ProviderCompat::default(),
        DebugConfig::default(),
    )
    .with_base_url(server.uri());

    let rx = provider.stream(&make_request()).await.expect("stream opens");
    let events = collect_events(rx).await;

    assert_eq!(events.len(), 2, "events: {events:?}");
    match &events[0] {
        LlmEvent::TextDelta(t) => assert_eq!(t, "Hello, world!"),
        e => panic!("expected TextDelta, got {e:?}"),
    }
    match &events[1] {
        LlmEvent::Done {
            stop_reason, usage, ..
        } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 12);
            assert_eq!(usage.output_tokens, 5);
        }
        e => panic!("expected Done, got {e:?}"),
    }
}

/// The Codex terminal-success alias `response.done` closes the stream just like
/// `response.completed` — a Codex turn must not surface as "response truncated".
#[tokio::test]
async fn chatgpt_stream_response_done_alias_closes_cleanly() {
    let server = MockServer::start().await;

    let delta = r#"{"type":"response.output_text.delta","delta":"done frame"}"#;
    let done = r#"{"type":"response.done","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#;
    let sse_body = build_responses_sse(&[delta, done]);

    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIChatGptProvider::new(
        static_bearer("tok", "acct"),
        ProviderCompat::default(),
        DebugConfig::default(),
    )
    .with_base_url(server.uri());

    let events = collect_events(provider.stream(&make_request()).await.unwrap()).await;

    // No trailing Error event (a truncation would append one).
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, LlmEvent::Error(_))),
        "stream must close cleanly on response.done: {events:?}"
    );
    assert!(matches!(events.last(), Some(LlmEvent::Done { .. })));
}

/// A 401 from the Codex backend maps to `MissingApiKey` so the CLI nudges
/// re-login (the OAuth token is bad beyond refresh).
#[tokio::test]
async fn chatgpt_401_maps_to_missing_api_key() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let provider = OpenAIChatGptProvider::new(
        static_bearer("expired", "acct"),
        ProviderCompat::default(),
        DebugConfig::default(),
    )
    .with_base_url(server.uri());

    let err = provider
        .stream(&make_request())
        .await
        .expect_err("401 must error");
    assert!(matches!(err, ProviderError::MissingApiKey), "got {err:?}");
}

/// A non-401 error status surfaces as `ProviderError::Api` carrying the status
/// and body.
#[tokio::test]
async fn chatgpt_500_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let provider = OpenAIChatGptProvider::new(
        static_bearer("tok", "acct"),
        ProviderCompat::default(),
        DebugConfig::default(),
    )
    .with_base_url(server.uri());

    let err = provider
        .stream(&make_request())
        .await
        .expect_err("500 must error");
    match err {
        ProviderError::Api { status, .. } => assert_eq!(status, 500),
        other => panic!("expected Api error, got {other:?}"),
    }
}

/// A failing bearer source aborts the turn before any HTTP request — the OAuth
/// error propagates out of `stream()`.
#[tokio::test]
async fn chatgpt_bearer_failure_propagates() {
    let provider = OpenAIChatGptProvider::new(
        failing_bearer(),
        ProviderCompat::default(),
        DebugConfig::default(),
    );
    let err = provider
        .stream(&make_request())
        .await
        .expect_err("bearer failure must error");
    assert!(matches!(err, ProviderError::Connection(_)), "got {err:?}");
}
