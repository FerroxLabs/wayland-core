//! W8a B.2 — OllamaProvider HTTP shape verification via wiremock.
//!
//! Confirms the provider:
//!   * POSTs to the configured base_url,
//!   * sends `{model, messages: [{role, content}, ...], stream: false}`,
//!   * extracts `message.content` from the Ollama response shape,
//!   * surfaces non-2xx statuses as `OllamaError::Status`.

use serde_json::{Value, json};
use wcore_types::message::{ContentBlock, Message, Role};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wayland_ollama::{OllamaError, OllamaProvider};

#[tokio::test]
async fn ollama_provider_round_trips_a_text_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "Hello!" },
            "done": true,
        })))
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(format!("{}/api/chat", server.uri()), "llama3");
    let user_msg = Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: "Hi".to_string(),
        }],
    );
    let got = provider.send_chat_blocking(&[user_msg]).await.unwrap();
    assert_eq!(got, "Hello!");
}

#[tokio::test]
async fn ollama_provider_sends_canonical_request_shape() {
    let server = MockServer::start().await;
    // Captures the request body via the mock so we can inspect what
    // the provider actually sent.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "message": { "role": "assistant", "content": "ok" },
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(format!("{}/api/chat", server.uri()), "llama4");
    let msg = Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: "ping".to_string(),
        }],
    );
    let _ = provider.send_chat_blocking(&[msg]).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "llama4");
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "ping");
}

#[tokio::test]
async fn ollama_provider_surfaces_non_2xx_as_status_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(503).set_body_string("server busy"))
        .mount(&server)
        .await;

    let provider = OllamaProvider::new(format!("{}/api/chat", server.uri()), "llama3");
    let msg = Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: "x".to_string(),
        }],
    );
    let err = provider.send_chat_blocking(&[msg]).await.unwrap_err();
    match err {
        OllamaError::Status { status, body } => {
            assert_eq!(status, 503);
            assert!(body.contains("server busy"));
        }
        other => panic!("expected OllamaError::Status, got {other:?}"),
    }
}
