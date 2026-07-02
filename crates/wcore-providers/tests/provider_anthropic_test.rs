// Integration tests for AnthropicProvider using wiremock to mock the Anthropic API.

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};
use wcore_types::message::{ContentBlock, Message, Role, StopReason};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn minimal_request() -> LlmRequest {
    LlmRequest {
        model: "claude-3-5-sonnet-20241022".to_string(),
        system: "You are helpful.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )],
        tools: vec![],
        max_tokens: 1024,
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

/// Build a complete SSE body for a simple text response.
fn text_sse_body(text: &str) -> String {
    format!(
        "event: message_start\n\
         data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-5-sonnet-20241022\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{{\"input_tokens\":100,\"output_tokens\":1}}}}}}\n\n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\n\
         data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}},\"usage\":{{\"output_tokens\":50}}}}\n\n\
         event: message_stop\n\
         data: {{\"type\":\"message_stop\"}}\n\n"
    )
}

/// Collect all events from a receiver into a Vec, draining until closed.
async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

// ---------------------------------------------------------------------------
// test_anthropic_stream_text_response
// ---------------------------------------------------------------------------

/// A normal text SSE stream produces TextDelta events followed by a Done event.
#[tokio::test]
async fn test_anthropic_stream_text_response() {
    // Arrange: start a mock server
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(text_sse_body("Hello, world!"), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-api-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let request = minimal_request();

    // Act
    let rx = provider
        .stream(&request)
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    // Assert: at least one TextDelta and exactly one Done
    let text_deltas: Vec<&LlmEvent> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::TextDelta(_)))
        .collect();
    assert!(!text_deltas.is_empty(), "expected at least one TextDelta");

    match &text_deltas[0] {
        LlmEvent::TextDelta(text) => assert_eq!(text, "Hello, world!"),
        _ => panic!("expected TextDelta"),
    }

    let done_events: Vec<&LlmEvent> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::Done { .. }))
        .collect();
    assert_eq!(done_events.len(), 1, "expected exactly one Done event");

    match done_events[0] {
        LlmEvent::Done {
            stop_reason,
            finish_reason: _,
            usage,
        } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 100);
            assert_eq!(usage.output_tokens, 50);
        }
        _ => panic!("expected Done"),
    }
}

// ---------------------------------------------------------------------------
// test_anthropic_stream_tool_use
// ---------------------------------------------------------------------------

/// An SSE stream containing a tool_use block produces a ToolUse event with
/// accumulated JSON input.
#[tokio::test]
async fn test_anthropic_stream_tool_use() {
    let server = MockServer::start().await;

    let sse_body = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tool\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-5-sonnet-20241022\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":80,\"output_tokens\":1}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_abc\",\"name\":\"Read\",\"input\":{}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"_path\\\":\\\"/tmp/test\\\"}\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":30}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-api-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let request = minimal_request();

    // Act
    let rx = provider
        .stream(&request)
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    // Assert: one ToolUse event with correct fields
    let tool_events: Vec<&LlmEvent> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::ToolUse { .. }))
        .collect();
    assert_eq!(tool_events.len(), 1, "expected exactly one ToolUse event");

    match tool_events[0] {
        LlmEvent::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, "toolu_abc");
            assert_eq!(name, "Read");
            assert_eq!(input["file_path"], "/tmp/test");
        }
        _ => panic!("expected ToolUse"),
    }

    // Done event should reflect tool_use stop reason
    let done_events: Vec<&LlmEvent> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::Done { .. }))
        .collect();
    assert_eq!(done_events.len(), 1);
    match done_events[0] {
        LlmEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        _ => panic!("expected Done"),
    }
}

// ---------------------------------------------------------------------------
// test_anthropic_stream_with_thinking
// ---------------------------------------------------------------------------

/// An SSE stream containing a thinking block produces ThinkingDelta events.
#[tokio::test]
async fn test_anthropic_stream_with_thinking() {
    let server = MockServer::start().await;

    let sse_body = "\
event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_think\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-5-sonnet-20241022\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":90,\"output_tokens\":1}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me think...\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Answer.\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":20}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    // Enable thinking in the request
    let mut request = minimal_request();
    request.thinking = Some(ThinkingConfig::Enabled {
        budget_tokens: 5000,
    });

    let provider = AnthropicProvider::new(
        "test-api-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);

    // Act
    let rx = provider
        .stream(&request)
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    // Assert: ThinkingDelta event present with expected content
    let thinking_events: Vec<&LlmEvent> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::ThinkingDelta(_)))
        .collect();
    assert!(
        !thinking_events.is_empty(),
        "expected at least one ThinkingDelta"
    );

    match thinking_events[0] {
        LlmEvent::ThinkingDelta(text) => assert_eq!(text, "Let me think..."),
        _ => panic!("expected ThinkingDelta"),
    }

    // TextDelta should also be present
    let text_events: Vec<&LlmEvent> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::TextDelta(_)))
        .collect();
    assert!(
        !text_events.is_empty(),
        "expected at least one TextDelta after thinking"
    );
}

// ---------------------------------------------------------------------------
// test_anthropic_auth_error
// ---------------------------------------------------------------------------

/// A 401 response from the API should produce a ProviderError::Api with status 401.
#[tokio::test]
async fn test_anthropic_auth_error() {
    let server = MockServer::start().await;

    let error_body =
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string(error_body))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "bad-api-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let request = minimal_request();

    // Act
    let result = provider.stream(&request).await;

    // Assert: returns an Api error with status 401
    match result {
        Err(ProviderError::Api { status, message }) => {
            assert_eq!(status, 401);
            assert!(
                message.contains("authentication_error") || message.contains("invalid x-api-key"),
                "unexpected error message: {message}"
            );
        }
        Err(other) => panic!("expected Api error, got: {other:?}"),
        Ok(_) => panic!("expected an error but stream succeeded"),
    }
}

// ---------------------------------------------------------------------------
// HTTP error class: 400 / 403 / 500 / 503
// (Mutation at anthropic.rs:191 — removing `!` from `!status.is_success()`.)
// ---------------------------------------------------------------------------

/// 400 Bad Request → ProviderError::Api{status:400}, not Ok.
#[tokio::test]
async fn test_anthropic_400_bad_request_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens is required"}}"#,
        ))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let result = provider.stream(&minimal_request()).await;
    assert!(result.is_err(), "HTTP 400 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 400),
        e => panic!("expected Api(400), got: {e:?}"),
    }
}

/// 403 Forbidden → ProviderError::Api{status:403}, not Ok.
#[tokio::test]
async fn test_anthropic_403_forbidden_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            r#"{"type":"error","error":{"type":"permission_error","message":"Permission denied"}}"#,
        ))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let result = provider.stream(&minimal_request()).await;
    assert!(result.is_err(), "HTTP 403 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 403),
        e => panic!("expected Api(403), got: {e:?}"),
    }
}

/// 500 Internal Server Error → ProviderError::Api{status:500}, not Ok.
/// `builder_send_with_retry` retries 5xx — mock must answer all 3 attempts.
#[tokio::test]
async fn test_anthropic_500_internal_error_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(500).set_body_string(
            r#"{"type":"error","error":{"type":"api_error","message":"Internal server error"}}"#,
        ))
        .expect(3) // 1 initial + 2 retries = 3 total
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let result = provider.stream(&minimal_request()).await;
    assert!(result.is_err(), "HTTP 500 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 500),
        e => panic!("expected Api(500), got: {e:?}"),
    }
}

/// 503 Service Unavailable → ProviderError::Api{status:503}, not Ok.
#[tokio::test]
async fn test_anthropic_503_unavailable_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .expect(3) // 1 initial + 2 retries = 3 total
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let result = provider.stream(&minimal_request()).await;
    assert!(result.is_err(), "HTTP 503 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 503),
        e => panic!("expected Api(503), got: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// test_anthropic_rate_limit_retryable
// ---------------------------------------------------------------------------

/// A 429 response from the API should produce a ProviderError::RateLimited.
#[tokio::test]
async fn test_anthropic_rate_limit_retryable() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_string(
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"rate limit exceeded"}}"#,
        ))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-api-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let request = minimal_request();

    // Act
    let result = provider.stream(&request).await;

    // Assert: RateLimited error, which is retryable
    match result {
        Err(ProviderError::RateLimited { retry_after_ms }) => {
            assert!(retry_after_ms > 0, "retry_after_ms should be positive");
        }
        Err(other) => panic!("expected RateLimited error, got: {other:?}"),
        Ok(_) => panic!("expected an error but stream succeeded"),
    }
}

// ---------------------------------------------------------------------------
// test_anthropic_request_headers
// ---------------------------------------------------------------------------

/// The provider must send the correct HTTP headers: x-api-key, anthropic-version,
/// and content-type. This test uses wiremock header matchers to verify them.
#[tokio::test]
async fn test_anthropic_request_headers() {
    let server = MockServer::start().await;

    // Register the mock with header matchers; only requests carrying the
    // correct headers will match and receive a 200 response.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "my-secret-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(header("content-type", "application/json"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(text_sse_body("ok"), "text/event-stream"),
        )
        .expect(1) // exactly one matching request must arrive
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "my-secret-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let request = minimal_request();

    // Act — should succeed because the headers are correct
    let result = provider.stream(&request).await;
    assert!(result.is_ok(), "stream failed: {:?}", result.err());

    // Drain the channel so the spawned task finishes
    if let Ok(rx) = result {
        collect_events(rx).await;
    }

    // wiremock verifies the `expect(1)` assertion when MockServer is dropped;
    // if the header matcher was not satisfied the test will panic here.
    server.verify().await;
}

// ---------------------------------------------------------------------------
// test_anthropic_prompt_caching_header
// ---------------------------------------------------------------------------

/// When cache is enabled the provider must include the anthropic-beta header
/// for prompt caching.
#[tokio::test]
async fn test_anthropic_prompt_caching_header() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("anthropic-beta", "prompt-caching-2024-07-31"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(text_sse_body("cached"), "text/event-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;

    // with_cache(true) — default, but explicit here for clarity
    let provider = AnthropicProvider::new(
        "test-api-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(true);
    let request = minimal_request();

    let result = provider.stream(&request).await;
    assert!(result.is_ok(), "stream failed: {:?}", result.err());

    if let Ok(rx) = result {
        collect_events(rx).await;
    }

    server.verify().await;
}

// ---------------------------------------------------------------------------
// test_anthropic_no_prompt_caching_header_when_disabled
// ---------------------------------------------------------------------------

/// When cache is disabled the anthropic-beta header must NOT be present.
/// We verify this by mounting a mock that matches only without that header and
/// checking it receives exactly one request.
#[tokio::test]
async fn test_anthropic_no_prompt_caching_header_when_disabled() {
    let server = MockServer::start().await;

    // This mock matches any POST to /v1/messages (no anthropic-beta requirement).
    // We then confirm via received_requests that the header is absent.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(text_sse_body("no cache"), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-api-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);
    let request = minimal_request();

    let result = provider.stream(&request).await;
    assert!(result.is_ok(), "stream failed: {:?}", result.err());

    if let Ok(rx) = result {
        collect_events(rx).await;
    }

    // Inspect the captured request to assert that anthropic-beta is absent
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "expected exactly one request");
    let has_beta = received[0].headers.contains_key("anthropic-beta");
    assert!(
        !has_beta,
        "anthropic-beta header should not be present when cache is disabled"
    );
}

// ---------------------------------------------------------------------------
// E-H3 / D4 — a truncated Anthropic SSE stream (no message_delta) must error
// ---------------------------------------------------------------------------

/// An Anthropic stream that delivers content but is cut before the
/// `message_delta` event (which carries the `Done`) was truncated. Before
/// the fix `process_sse_stream` returned `Ok(())` and the channel just
/// closed — the engine read that as a clean empty turn. Now the parser must
/// surface an `LlmEvent::Error`.
#[tokio::test]
async fn test_anthropic_truncated_stream_no_message_delta_surfaces_error() {
    let server = MockServer::start().await;

    // message_start + a text delta, then the connection ends — no
    // message_delta, no message_stop.
    let truncated = "event: message_start\n\
         data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_t\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-5-sonnet-20241022\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n\
         event: content_block_start\n\
         data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
         event: content_block_delta\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n";

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(truncated, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&minimal_request()).await.unwrap();
    let events = collect_events(rx).await;

    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "truncated stream must surface an Error event; got: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Done { .. })),
        "truncated stream must NOT produce a Done; got: {events:?}"
    );
}

/// An entirely empty Anthropic stream (200 OK, zero bytes) must also error.
#[tokio::test]
async fn test_anthropic_empty_stream_surfaces_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("", "text/event-stream"))
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&minimal_request()).await.unwrap();
    let events = collect_events(rx).await;

    assert_eq!(events.len(), 1, "empty stream must yield exactly one event");
    assert!(
        matches!(&events[0], LlmEvent::Error(_)),
        "empty stream must surface an Error; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// E-H1 — a 429 with a Retry-After header is honoured
// ---------------------------------------------------------------------------

/// A 429 carrying `Retry-After: 12` must produce `RateLimited` with
/// `retry_after_ms == 12_000`, not the hardcoded 5_000 default.
#[tokio::test]
async fn test_anthropic_429_honours_retry_after_header() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "12")
                .set_body_string(r#"{"type":"error","error":{"type":"rate_limit_error"}}"#),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    );
    match provider.stream(&minimal_request()).await.unwrap_err() {
        ProviderError::RateLimited { retry_after_ms } => {
            assert_eq!(retry_after_ms, 12_000, "must honour Retry-After header");
        }
        e => panic!("expected RateLimited, got: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// Region-locked-key failover (compat.auth_fallback_base_url)
// ---------------------------------------------------------------------------

/// A 401 on the primary host with a configured `auth_fallback_base_url` must
/// transparently retry the SAME key against the fallback host and succeed.
/// Motivating case: MiniMax's two region-locked platforms (`api.minimax.io`
/// vs `api.minimaxi.com`) — a valid key works on exactly one.
#[tokio::test]
async fn test_anthropic_region_failover_retries_alternate_host_on_401() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    // Primary rejects the key.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid api key"}}"#,
        ))
        .mount(&primary)
        .await;

    // Fallback accepts the same key.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(text_sse_body("from fallback"), "text/event-stream"),
        )
        .mount(&fallback)
        .await;

    let mut compat = ProviderCompat::anthropic_defaults();
    compat.auth_fallback_base_url = Some(fallback.uri());

    let provider = AnthropicProvider::new(
        "region-locked-key",
        &primary.uri(),
        compat,
        DebugConfig::default(),
    )
    .with_cache(false);

    let rx = provider
        .stream(&minimal_request())
        .await
        .expect("failover to the fallback host must succeed");
    let events = collect_events(rx).await;

    let got_text = events
        .iter()
        .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "from fallback"));
    assert!(
        got_text,
        "expected the fallback host's response; got: {events:?}"
    );
}

/// Once the fallback authenticates it is PINNED: a second request goes straight
/// to the fallback and does not re-pay the primary's certain 401.
#[tokio::test]
async fn test_anthropic_region_failover_pins_working_host() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string("nope"))
        .mount(&primary)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(text_sse_body("ok"), "text/event-stream"),
        )
        .mount(&fallback)
        .await;

    let mut compat = ProviderCompat::anthropic_defaults();
    compat.auth_fallback_base_url = Some(fallback.uri());
    let provider = AnthropicProvider::new("k", &primary.uri(), compat, DebugConfig::default())
        .with_cache(false);

    // First request: primary 401 -> fallback 200, pins the fallback.
    collect_events(provider.stream(&minimal_request()).await.unwrap()).await;
    // Second request: must resolve on the pinned fallback only.
    collect_events(provider.stream(&minimal_request()).await.unwrap()).await;

    let primary_hits = primary.received_requests().await.unwrap().len();
    assert_eq!(
        primary_hits, 1,
        "after pinning, the primary host must not be retried"
    );
    let fallback_hits = fallback.received_requests().await.unwrap().len();
    assert_eq!(
        fallback_hits, 2,
        "both requests must resolve on the pinned fallback host"
    );
}

/// With NO fallback configured (the default for ordinary Anthropic providers),
/// a 401 surfaces as the error unchanged — no behavior drift.
#[tokio::test]
async fn test_anthropic_no_failover_without_fallback_configured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string("denied"))
        .mount(&server)
        .await;

    // anthropic_defaults() leaves auth_fallback_base_url = None.
    let provider = AnthropicProvider::new(
        "k",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    .with_cache(false);

    match provider.stream(&minimal_request()).await {
        Err(ProviderError::Api { status, .. }) => assert_eq!(status, 401),
        other => panic!("expected Api(401) with no failover, got: {other:?}"),
    }
}
