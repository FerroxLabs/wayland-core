use serde_json::json;
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::openai::OpenAIProvider;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role, StopReason};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal LlmRequest suitable for all tests.
fn make_request() -> LlmRequest {
    LlmRequest {
        model: "gpt-4o".to_string(),
        system: "You are a test assistant.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )],
        tools: vec![],
        max_tokens: 512,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
    }
}

/// Collect all events from the receiver until the channel closes.
async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

/// Build a raw SSE body string from a slice of JSON lines.
/// Each line is wrapped in `data: ...\n\n` and a final `data: [DONE]\n\n` is appended.
fn build_sse_body(data_lines: &[&str]) -> String {
    let mut body = String::new();
    for line in data_lines {
        body.push_str("data: ");
        body.push_str(line);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

// ---------------------------------------------------------------------------
// test_openai_stream_text_response
// ---------------------------------------------------------------------------

/// Verify that a normal text response (multiple content deltas followed by a
/// stop chunk with usage) is parsed into the correct sequence of TextDelta
/// and Done events.
#[tokio::test]
async fn test_openai_stream_text_response() {
    let server = MockServer::start().await;

    // Chunk 1: first text delta
    let chunk1 = json!({
        "id": "chatcmpl-001",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant", "content": "Hello" },
            "finish_reason": null
        }]
    })
    .to_string();

    // Chunk 2: second text delta
    let chunk2 = json!({
        "id": "chatcmpl-001",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "content": ", world!" },
            "finish_reason": null
        }]
    })
    .to_string();

    // Chunk 3: finish_reason = "stop" with usage
    let chunk3 = json!({
        "id": "chatcmpl-001",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 25,
            "completion_tokens": 10
        }
    })
    .to_string();

    let sse_body = build_sse_body(&[&chunk1, &chunk2, &chunk3]);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    // Expect: TextDelta("Hello"), TextDelta(", world!"), Done{EndTurn}
    assert_eq!(events.len(), 3, "expected 3 events, got: {:?}", events);

    match &events[0] {
        LlmEvent::TextDelta(text) => assert_eq!(text, "Hello"),
        e => panic!("expected TextDelta, got: {:?}", e),
    }

    match &events[1] {
        LlmEvent::TextDelta(text) => assert_eq!(text, ", world!"),
        e => panic!("expected TextDelta, got: {:?}", e),
    }

    match &events[2] {
        LlmEvent::Done {
            stop_reason,
            finish_reason: _,
            usage,
        } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 25);
            assert_eq!(usage.output_tokens, 10);
        }
        e => panic!("expected Done, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// test_openai_stream_tool_call_aggregation
// ---------------------------------------------------------------------------

/// Verify that a tool call streamed in multiple delta chunks (id in first chunk,
/// name in first chunk, arguments split across chunks) is correctly aggregated
/// into a single ToolUse event.
#[tokio::test]
async fn test_openai_stream_tool_call_aggregation() {
    let server = MockServer::start().await;

    // Chunk 1: tool call header — id and function name arrive first
    let chunk1 = json!({
        "id": "chatcmpl-002",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":"
                    }
                }]
            },
            "finish_reason": null
        }]
    })
    .to_string();

    // Chunk 2: arguments continuation
    let chunk2 = json!({
        "id": "chatcmpl-002",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {
                        "arguments": "\"/tmp/test.txt\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    })
    .to_string();

    // Chunk 3: finish_reason = "tool_calls" with usage
    let chunk3 = json!({
        "id": "chatcmpl-002",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 40,
            "completion_tokens": 15
        }
    })
    .to_string();

    let sse_body = build_sse_body(&[&chunk1, &chunk2, &chunk3]);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    // Expect: ToolUse, Done{ToolUse}
    assert_eq!(events.len(), 2, "expected 2 events, got: {:?}", events);

    match &events[0] {
        LlmEvent::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, "call_abc123");
            assert_eq!(name, "read_file");
            assert_eq!(input["path"], "/tmp/test.txt");
        }
        e => panic!("expected ToolUse, got: {:?}", e),
    }

    match &events[1] {
        LlmEvent::Done {
            stop_reason,
            finish_reason: _,
            usage,
        } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
            assert_eq!(usage.input_tokens, 40);
            assert_eq!(usage.output_tokens, 15);
        }
        e => panic!("expected Done, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// test_openai_multiple_tool_calls
// ---------------------------------------------------------------------------

/// Verify that when the API streams multiple parallel tool calls (different
/// indices) they are all emitted as separate ToolUse events.
#[tokio::test]
async fn test_openai_multiple_tool_calls() {
    let server = MockServer::start().await;

    // Chunk 1: first tool call (index 0)
    let chunk1 = json!({
        "id": "chatcmpl-003",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_tool0",
                    "type": "function",
                    "function": {
                        "name": "list_files",
                        "arguments": "{\"dir\": \"/tmp\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    })
    .to_string();

    // Chunk 2: second tool call (index 1)
    let chunk2 = json!({
        "id": "chatcmpl-003",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 1,
                    "id": "call_tool1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\": \"/etc/hosts\"}"
                    }
                }]
            },
            "finish_reason": null
        }]
    })
    .to_string();

    // Chunk 3: finish_reason = "tool_calls"
    let chunk3 = json!({
        "id": "chatcmpl-003",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "tool_calls"
        }],
        "usage": {
            "prompt_tokens": 60,
            "completion_tokens": 20
        }
    })
    .to_string();

    let sse_body = build_sse_body(&[&chunk1, &chunk2, &chunk3]);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    // Expect: ToolUse (index 0), ToolUse (index 1), Done{ToolUse}
    assert_eq!(events.len(), 3, "expected 3 events, got: {:?}", events);

    match &events[0] {
        LlmEvent::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, "call_tool0");
            assert_eq!(name, "list_files");
            assert_eq!(input["dir"], "/tmp");
        }
        e => panic!("expected first ToolUse, got: {:?}", e),
    }

    match &events[1] {
        LlmEvent::ToolUse {
            id, name, input, ..
        } => {
            assert_eq!(id, "call_tool1");
            assert_eq!(name, "read_file");
            assert_eq!(input["path"], "/etc/hosts");
        }
        e => panic!("expected second ToolUse, got: {:?}", e),
    }

    match &events[2] {
        LlmEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        e => panic!("expected Done, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// test_openai_stream_state_transitions
// ---------------------------------------------------------------------------

/// Verify that the stream correctly stops processing events once it encounters
/// the `[DONE]` sentinel — any data after [DONE] is ignored and the receiver
/// channel closes cleanly.
#[tokio::test]
async fn test_openai_stream_state_transitions() {
    let server = MockServer::start().await;

    // A single text delta followed by a stop chunk, then the [DONE] sentinel.
    let chunk1 = json!({
        "id": "chatcmpl-004",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "content": "Transition test." },
            "finish_reason": null
        }]
    })
    .to_string();

    let chunk2 = json!({
        "id": "chatcmpl-004",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5
        }
    })
    .to_string();

    // Build SSE body manually: two data lines, then [DONE], then a stray line
    // that must NOT produce any events.
    let mut sse_body = String::new();
    sse_body.push_str("data: ");
    sse_body.push_str(&chunk1);
    sse_body.push_str("\n\n");
    sse_body.push_str("data: ");
    sse_body.push_str(&chunk2);
    sse_body.push_str("\n\n");
    sse_body.push_str("data: [DONE]\n\n");
    // Stray chunk after [DONE] — must be ignored
    sse_body.push_str("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ignored\"},\"finish_reason\":null}]}\n\n");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    // Expect exactly: TextDelta, Done — the trailing chunk after [DONE] is discarded.
    assert_eq!(events.len(), 2, "expected 2 events, got: {:?}", events);

    match &events[0] {
        LlmEvent::TextDelta(text) => assert_eq!(text, "Transition test."),
        e => panic!("expected TextDelta, got: {:?}", e),
    }

    match &events[1] {
        LlmEvent::Done {
            stop_reason,
            finish_reason: _,
            usage,
        } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(usage.cache_creation_tokens, 0);
            assert_eq!(usage.cache_read_tokens, 0);
        }
        e => panic!("expected Done, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// test_openai_api_error_non_success_status
// ---------------------------------------------------------------------------

/// Verify that a non-2xx HTTP response is surfaced as a ProviderError::Api.
#[tokio::test]
async fn test_openai_api_error_non_success_status() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"error":{"message":"Invalid API key","type":"invalid_request_error"}}"#,
        ))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "bad-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let result = provider.stream(&make_request()).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        wcore_providers::ProviderError::Api { status, .. } => {
            assert_eq!(status, 401);
        }
        e => panic!("expected Api error, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// test_openai_rate_limited
// ---------------------------------------------------------------------------

/// Verify that a 429 response is surfaced as ProviderError::RateLimited.
#[tokio::test]
async fn test_openai_rate_limited() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let result = provider.stream(&make_request()).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        wcore_providers::ProviderError::RateLimited { retry_after_ms } => {
            assert_eq!(retry_after_ms, 5000);
        }
        e => panic!("expected RateLimited error, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// HTTP error class: 400 / 403 / 500 / 503
// (The `!`-mutant at openai.rs:541 survives unless these are covered.)
// ---------------------------------------------------------------------------

/// 400 Bad Request → ProviderError::Api{status:400}, not Ok.
#[tokio::test]
async fn test_openai_400_bad_request_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"max_tokens is too large","type":"invalid_request_error"}}"#,
        ))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let result = provider.stream(&make_request()).await;
    assert!(result.is_err(), "HTTP 400 must surface as an error");
    match result.unwrap_err() {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 400),
        e => panic!("expected Api(400), got: {e:?}"),
    }
}

/// 403 Forbidden → ProviderError::Api{status:403}, not Ok.
#[tokio::test]
async fn test_openai_403_forbidden_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            r#"{"error":{"message":"Permission denied","type":"permission_error"}}"#,
        ))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let result = provider.stream(&make_request()).await;
    assert!(result.is_err(), "HTTP 403 must surface as an error");
    match result.unwrap_err() {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 403),
        e => panic!("expected Api(403), got: {e:?}"),
    }
}

/// 500 Internal Server Error → ProviderError::Api{status:500}, not Ok.
/// `builder_send_with_retry` retries 5xx — the mock must answer all 3 attempts
/// (1 initial + 2 retries = DEFAULT_MAX_RETRIES=2 → 3 total) to avoid an
/// assertion failure on the wiremock side when the server is dropped.
#[tokio::test]
async fn test_openai_500_internal_error_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string(
            r#"{"error":{"message":"Internal server error","type":"server_error"}}"#,
        ))
        // 3 calls: 1 initial attempt + 2 retries (DEFAULT_MAX_RETRIES = 2).
        .expect(3)
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let result = provider.stream(&make_request()).await;
    assert!(result.is_err(), "HTTP 500 must surface as an error");
    match result.unwrap_err() {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 500),
        e => panic!("expected Api(500), got: {e:?}"),
    }
}

/// 503 Service Unavailable → ProviderError::Api{status:503}, not Ok.
#[tokio::test]
async fn test_openai_503_unavailable_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .expect(3)
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let result = provider.stream(&make_request()).await;
    assert!(result.is_err(), "HTTP 503 must surface as an error");
    match result.unwrap_err() {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 503),
        e => panic!("expected Api(503), got: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// test_openai_stream_max_tokens_stop_reason
// ---------------------------------------------------------------------------

/// Verify that finish_reason "length" maps to StopReason::MaxTokens.
#[tokio::test]
async fn test_openai_stream_max_tokens_stop_reason() {
    let server = MockServer::start().await;

    let chunk1 = json!({
        "id": "chatcmpl-005",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "content": "Truncated" },
            "finish_reason": null
        }]
    })
    .to_string();

    let chunk2 = json!({
        "id": "chatcmpl-005",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "length"
        }],
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 512
        }
    })
    .to_string();

    let sse_body = build_sse_body(&[&chunk1, &chunk2]);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    assert_eq!(events.len(), 2);

    match &events[1] {
        LlmEvent::Done {
            stop_reason,
            finish_reason: _,
            usage,
        } => {
            assert_eq!(*stop_reason, StopReason::MaxTokens);
            assert_eq!(usage.input_tokens, 100);
            assert_eq!(usage.output_tokens, 512);
        }
        e => panic!("expected Done with MaxTokens, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// test_openai_stream_empty_content_delta_skipped
// ---------------------------------------------------------------------------

/// Verify that empty content strings in deltas do NOT produce TextDelta events
/// (the provider filters them out).
#[tokio::test]
async fn test_openai_stream_empty_content_delta_skipped() {
    let server = MockServer::start().await;

    // Chunk with empty content — should be silently skipped
    let chunk_empty = json!({
        "id": "chatcmpl-006",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "content": "" },
            "finish_reason": null
        }]
    })
    .to_string();

    let chunk_text = json!({
        "id": "chatcmpl-006",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "content": "actual content" },
            "finish_reason": null
        }]
    })
    .to_string();

    let chunk_done = json!({
        "id": "chatcmpl-006",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 3 }
    })
    .to_string();

    let sse_body = build_sse_body(&[&chunk_empty, &chunk_text, &chunk_done]);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    // Expect only TextDelta("actual content") and Done — no empty TextDelta
    assert_eq!(events.len(), 2, "expected 2 events, got: {:?}", events);

    match &events[0] {
        LlmEvent::TextDelta(text) => assert_eq!(text, "actual content"),
        e => panic!("expected TextDelta with actual content, got: {:?}", e),
    }

    match &events[1] {
        LlmEvent::Done { stop_reason, .. } => assert_eq!(*stop_reason, StopReason::EndTurn),
        e => panic!("expected Done, got: {:?}", e),
    }
}

// ---------------------------------------------------------------------------
// E-H3 — in-band OpenAI error frame must surface as LlmEvent::Error
// ---------------------------------------------------------------------------

/// An OpenAI-compatible provider can stream `{"error":{...}}` mid-stream
/// instead of `choices`. Before the fix `parse_sse_chunk` found no `choices`
/// and returned zero events — the turn ended as a silent empty success.
/// Now it must emit an `LlmEvent::Error` carrying the provider's message.
#[tokio::test]
async fn test_openai_in_band_error_frame_surfaces_error_event() {
    let server = MockServer::start().await;

    // Error frame followed by [DONE] — the realistic shape for an
    // OpenAI-compat provider that aborts mid-stream.
    let error_frame = r#"{"error":{"message":"upstream model overloaded","type":"server_error"}}"#;
    let mut body = String::new();
    body.push_str("data: ");
    body.push_str(error_frame);
    body.push_str("\n\n");
    body.push_str("data: [DONE]\n\n");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, LlmEvent::Error(m) if m.contains("overloaded"))),
        "in-band error frame must produce an Error event; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// E-H3 / D4 — a Done-less / truncated SSE close must surface an error
// ---------------------------------------------------------------------------

/// A stream that closes cleanly with content but NO `[DONE]` and no
/// `finish_reason` was truncated. Before the fix this produced a silent
/// "successful" turn with partial text. Now the parser must emit an
/// `LlmEvent::Error` so the engine does not mistake it for a clean turn.
#[tokio::test]
async fn test_openai_truncated_stream_no_done_surfaces_error() {
    let server = MockServer::start().await;

    // Partial content, then the connection just ends — no [DONE].
    let chunk = json!({
        "id": "chatcmpl-trunc",
        "object": "chat.completion.chunk",
        "choices": [{
            "index": 0,
            "delta": { "content": "partial answer" },
            "finish_reason": null
        }]
    })
    .to_string();
    let body = format!("data: {chunk}\n\n");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    // The partial TextDelta is fine to deliver, but the stream MUST end with
    // an Error event — never a clean close with no terminal event.
    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "truncated stream must surface an Error event; got: {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Done { .. })),
        "truncated stream must NOT produce a Done; got: {events:?}"
    );
}

/// An entirely empty stream (200 OK, zero bytes, clean close) is the worst
/// case of D4 — no content, no terminal event. Must surface an Error.
#[tokio::test]
async fn test_openai_empty_stream_surfaces_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("", "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    assert_eq!(events.len(), 1, "empty stream must yield exactly one event");
    assert!(
        matches!(&events[0], LlmEvent::Error(_)),
        "empty stream must surface an Error event; got: {events:?}"
    );
}

/// `[DONE]` arriving with no preceding `finish_reason` chunk is also a
/// truncation (the model never signalled completion). Must error.
#[tokio::test]
async fn test_openai_done_without_finish_reason_surfaces_error() {
    let server = MockServer::start().await;

    let chunk = json!({
        "id": "chatcmpl-x",
        "object": "chat.completion.chunk",
        "choices": [{ "index": 0, "delta": { "content": "hi" }, "finish_reason": null }]
    })
    .to_string();
    let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "[DONE] with no finish_reason must surface an Error; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// E-H1 — a 429 with a Retry-After header must be honoured (not hardcoded 5s)
// ---------------------------------------------------------------------------

/// A 429 carrying `Retry-After: 30` must produce `RateLimited` with
/// `retry_after_ms == 30_000`, not the hardcoded 5_000 default.
#[tokio::test]
async fn test_openai_429_honours_retry_after_header() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "30")
                .set_body_string("Too Many Requests"),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let result = provider.stream(&make_request()).await;

    match result.unwrap_err() {
        wcore_providers::ProviderError::RateLimited { retry_after_ms } => {
            assert_eq!(
                retry_after_ms, 30_000,
                "must honour the Retry-After header, not the 5s default"
            );
        }
        e => panic!("expected RateLimited, got: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// M4 — an SSE stream with no newline delimiter must not grow the buffer
// without bound; it must error once a single frame exceeds the cap.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_openai_unterminated_sse_frame_is_capped() {
    let server = MockServer::start().await;

    // 2 MiB of bytes, zero newlines — a single never-terminating SSE frame.
    let huge = "x".repeat(2 * 1024 * 1024);

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(huge, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(
        "test-key",
        &server.uri(),
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    );
    let rx = provider.stream(&make_request()).await.unwrap();
    let events = collect_events(rx).await;

    // Must surface an Error (the Parse/cap error), never silently succeed.
    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "an unterminated >1MiB SSE frame must surface an Error; got {} events",
        events.len()
    );
}
