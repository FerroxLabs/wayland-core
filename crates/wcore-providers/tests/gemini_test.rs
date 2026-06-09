// Integration tests for the native GeminiProvider.
//
// Fixture-replay against a wiremock server hosting the
// `:streamGenerateContent?alt=sse` endpoint. The fixtures encode
// real-shape Gemini SSE frames — see
// https://ai.google.dev/api/generate-content#method:-models.streamgeneratecontent
// for the canonical wire format.
//
// One live-API smoke test is gated behind `#[ignore]` and the
// `GEMINI_API_KEY` env var so CI doesn't spend tokens unless a maintainer
// opts in (`vx cargo nextest run -p wcore-providers --run-ignored=all
// --features live-gemini` once the feature is added in a follow-up).

use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::gemini::{GeminiProvider, SafetySetting};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role, StopReason};
use wcore_types::tool::ToolDef;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn minimal_request() -> LlmRequest {
    LlmRequest {
        model: "gemini-2.5-pro".to_string(),
        system: String::new(),
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
    }
}

/// Drain the receiver into a Vec.
async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

/// Build a Gemini SSE response body. Each chunk is JSON wrapped in
/// `data: <json>\n\n`.
fn sse_body(chunks: &[&str]) -> String {
    let mut out = String::new();
    for c in chunks {
        out.push_str("data: ");
        out.push_str(c);
        out.push_str("\n\n");
    }
    out
}

/// Build a provider rooted at the mock server with merge_same_role on
/// (matches gemini_defaults()).
fn provider_for(server: &MockServer) -> GeminiProvider {
    let compat = ProviderCompat {
        merge_same_role: Some(true),
        ..Default::default()
    };
    GeminiProvider::new(
        "test-api-key",
        &server.uri(),
        compat,
        DebugConfig::default(),
    )
}

// ---------------------------------------------------------------------------
// 1) Text streaming → TextDelta events + Done(EndTurn / Stop) on EOF
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gemini_stream_text_response_produces_text_deltas_and_done() {
    let server = MockServer::start().await;

    let body = sse_body(&[
        r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}]}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1}}"#,
        r#"{"candidates":[{"content":{"parts":[{"text":", world!"}]}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3}}"#,
        r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3}}"#,
    ]);

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "test-api-key"))
        .and(header("content-type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let provider = provider_for(&server);
    let rx = provider
        .stream(&minimal_request())
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    // TextDeltas concatenate to "Hello, world!"
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello, world!");

    // Exactly one Done event with EndTurn / usage propagated.
    let done = events
        .iter()
        .find(|e| matches!(e, LlmEvent::Done { .. }))
        .expect("Done event missing");
    match done {
        LlmEvent::Done {
            stop_reason, usage, ..
        } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 5);
            assert_eq!(usage.output_tokens, 3);
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// 2) Thought parts → ThinkingDelta, separate from final text
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gemini_stream_thought_parts_route_to_thinking_delta() {
    let server = MockServer::start().await;

    // First chunk: thought; second chunk: regular text; finish.
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"parts":[{"text":"Let me think.","thought":true}]}}]}"#,
        r#"{"candidates":[{"content":{"parts":[{"text":"Answer: 42"}]}}]}"#,
        r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#,
    ]);

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let rx = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    let thinking: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::ThinkingDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking, vec!["Let me think."]);

    let text: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, vec!["Answer: 42"]);
}

// ---------------------------------------------------------------------------
// 3) Native function call → ToolUse event with thoughtSignature in `extra`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gemini_stream_function_call_emits_tool_use_with_signature() {
    let server = MockServer::start().await;

    let body = sse_body(&[
        r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"Read","args":{"path":"/tmp/x"}},"thoughtSignature":"abc-sig"}]}}]}"#,
        r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":4}}"#,
    ]);

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let rx = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    let tool_use = events
        .iter()
        .find(|e| matches!(e, LlmEvent::ToolUse { .. }))
        .expect("ToolUse event missing");
    match tool_use {
        LlmEvent::ToolUse {
            name, input, extra, ..
        } => {
            assert_eq!(name, "Read");
            assert_eq!(input["path"], "/tmp/x");
            let extra = extra.as_ref().expect("signature must be captured");
            assert_eq!(extra["thoughtSignature"], "abc-sig");
        }
        _ => unreachable!(),
    }

    // STOP with a tool call observed -> StopReason::ToolUse
    let done = events
        .iter()
        .find(|e| matches!(e, LlmEvent::Done { .. }))
        .expect("Done event missing");
    match done {
        LlmEvent::Done { stop_reason, .. } => {
            assert_eq!(*stop_reason, StopReason::ToolUse);
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// 4) MAX_TOKENS finishReason -> StopReason::MaxTokens + FinishReason::Length
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gemini_max_tokens_finish_reason_maps_to_length() {
    let server = MockServer::start().await;

    let body = sse_body(&[
        r#"{"candidates":[{"content":{"parts":[{"text":"Partial"}]}}]}"#,
        r#"{"candidates":[{"content":{"parts":[]},"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":1024}}"#,
    ]);

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let rx = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    let done = events
        .iter()
        .find(|e| matches!(e, LlmEvent::Done { .. }))
        .expect("Done event missing");
    match done {
        LlmEvent::Done {
            stop_reason,
            finish_reason,
            ..
        } => {
            assert_eq!(*stop_reason, StopReason::MaxTokens);
            assert_eq!(*finish_reason, wcore_types::message::FinishReason::Length);
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// 5) SAFETY finishReason -> FinishReason::Error (refusal surfaces to host)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gemini_safety_finish_reason_maps_to_error() {
    let server = MockServer::start().await;

    let body = sse_body(&[
        r#"{"candidates":[{"content":{"parts":[]},"finishReason":"SAFETY"}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":0}}"#,
    ]);

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let rx = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect("stream should succeed");
    let events = collect_events(rx).await;

    let done = events
        .iter()
        .find(|e| matches!(e, LlmEvent::Done { .. }))
        .expect("Done event missing");
    match done {
        LlmEvent::Done { finish_reason, .. } => {
            assert_eq!(
                *finish_reason,
                wcore_types::message::FinishReason::Error,
                "SAFETY must surface as FinishReason::Error, not silently as Stop"
            );
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// 6) 429 → ProviderError::RateLimited
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gemini_rate_limit_surfaces_as_provider_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let err = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect_err("429 should surface as ProviderError");
    match err {
        wcore_providers::ProviderError::RateLimited { .. } => {} // ok
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// HTTP error class: 400 / 401 / 403 / 500 / 503
// (Mutation at gemini.rs:425 — removing `!` from `!status.is_success()`.)
// ---------------------------------------------------------------------------

/// 400 Bad Request → ProviderError::Api{status:400}, not Ok.
#[tokio::test]
async fn gemini_400_bad_request_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"code":400,"message":"Request payload too large","status":"INVALID_ARGUMENT"}}"#,
        ))
        .mount(&server)
        .await;

    let err = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect_err("HTTP 400 must surface as an error");
    match err {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 400),
        other => panic!("expected Api(400), got {other:?}"),
    }
}

/// 401 Unauthorized → ProviderError::Api{status:401}, not Ok.
#[tokio::test]
async fn gemini_401_unauthorized_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"error":{"code":401,"message":"API key not valid","status":"UNAUTHENTICATED"}}"#,
        ))
        .mount(&server)
        .await;

    let err = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect_err("HTTP 401 must surface as an error");
    match err {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 401),
        other => panic!("expected Api(401), got {other:?}"),
    }
}

/// 403 Forbidden → ProviderError::Api{status:403}, not Ok.
#[tokio::test]
async fn gemini_403_forbidden_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            r#"{"error":{"code":403,"message":"Permission denied","status":"PERMISSION_DENIED"}}"#,
        ))
        .mount(&server)
        .await;

    let err = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect_err("HTTP 403 must surface as an error");
    match err {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 403),
        other => panic!("expected Api(403), got {other:?}"),
    }
}

/// 500 Internal Server Error → ProviderError::Api{status:500}, not Ok.
/// `builder_send_with_retry` retries 5xx — mock must answer all 3 attempts.
#[tokio::test]
async fn gemini_500_internal_error_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(500).set_body_string(
            r#"{"error":{"code":500,"message":"Internal error","status":"INTERNAL"}}"#,
        ))
        .expect(3) // 1 initial + 2 retries = 3 total
        .mount(&server)
        .await;

    let err = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect_err("HTTP 500 must surface as an error");
    match err {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 500),
        other => panic!("expected Api(500), got {other:?}"),
    }
}

/// 503 Service Unavailable → ProviderError::Api{status:503}, not Ok.
#[tokio::test]
async fn gemini_503_unavailable_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .expect(3) // 1 initial + 2 retries = 3 total
        .mount(&server)
        .await;

    let err = provider_for(&server)
        .stream(&minimal_request())
        .await
        .expect_err("HTTP 503 must surface as an error");
    match err {
        wcore_providers::ProviderError::Api { status, .. } => assert_eq!(status, 503),
        other => panic!("expected Api(503), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 7) Request-body wiring: tools + safety_settings reach the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gemini_request_body_carries_tools_and_safety_settings() {
    let server = MockServer::start().await;

    // Match against the JSON body — fail the request body assertion will
    // simply return 200 on miss, so we capture the request via a custom
    // matcher: wiremock body_json_string isn't ideal here, so we mount a
    // wildcard responder and inspect via `received_requests`.
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-pro:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            sse_body(&[
                r#"{"candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":0}}"#,
            ]),
            "text/event-stream",
        ))
        .mount(&server)
        .await;

    let compat = ProviderCompat {
        merge_same_role: Some(true),
        ..Default::default()
    };
    let provider = GeminiProvider::new(
        "test-api-key",
        &server.uri(),
        compat,
        DebugConfig::default(),
    )
    .with_safety_settings(vec![SafetySetting {
        category: "HARM_CATEGORY_HATE_SPEECH".into(),
        threshold: "BLOCK_LOW_AND_ABOVE".into(),
    }]);

    let mut request = minimal_request();
    request.tools = vec![ToolDef {
        name: "Read".into(),
        description: "Read a file".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
        deferred: false,
    }];
    request.system = "Be terse.".into();
    request.messages.insert(
        0,
        Message::new(
            Role::System,
            vec![ContentBlock::Text {
                text: "Be terse.".into(),
            }],
        ),
    );

    let rx = provider.stream(&request).await.expect("stream ok");
    let _ = collect_events(rx).await;

    let received = server.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("request body is JSON");

    // systemInstruction surface
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"], "Be terse.",
        "system extracted into top-level systemInstruction"
    );
    // generationConfig surface
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 1024);
    // tools surface
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["name"], "Read",
        "tools serialized as functionDeclarations, NOT openai tool_calls"
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["parameters"]["properties"]["path"]["type"],
        "string"
    );
    // safetySettings surface
    assert_eq!(
        body["safetySettings"][0]["category"], "HARM_CATEGORY_HATE_SPEECH",
        "safetySettings wired through"
    );
    assert_eq!(
        body["safetySettings"][0]["threshold"],
        "BLOCK_LOW_AND_ABOVE"
    );
    // contents surface — Role::System pulled out, only Role::User remains.
    assert_eq!(body["contents"][0]["role"], "user");
    assert_eq!(body["contents"][0]["parts"][0]["text"], "Hello");
}

// ---------------------------------------------------------------------------
// 8) Live-API smoke test (gated)
// ---------------------------------------------------------------------------
//
// M2.6: Gated behind the `live-gemini` cargo feature. Default CI does not
// compile or run this test. Enable manually with:
//   cargo nextest run -p wcore-providers --features live-gemini \
//       --test gemini_test -E 'test(gemini_live_api_smoke_test)'
// With the feature ON, a missing `GEMINI_API_KEY` PANICS — the contract is
// "you asked for live tests; you must supply the key" — rather than the
// previous silent skip which masked credential-misconfiguration failures.

#[cfg(feature = "live-gemini")]
#[tokio::test]
async fn gemini_live_api_smoke_test() {
    let api_key = std::env::var("GEMINI_API_KEY").expect(
        "[gemini_live_api_smoke_test] GEMINI_API_KEY required when \
         --features live-gemini is enabled (wcore-providers/live-gemini)",
    );

    let provider = GeminiProvider::new(
        &api_key,
        wcore_providers::gemini::DEFAULT_GEMINI_BASE_URL,
        ProviderCompat::default(),
        DebugConfig::default(),
    );

    let request = LlmRequest {
        model: "gemini-2.5-flash".to_string(), // use Flash for cheaper smoke
        system: "Be very brief. One word only.".to_string(),
        messages: vec![
            Message::new(
                Role::System,
                vec![ContentBlock::Text {
                    text: "Be very brief. One word only.".into(),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "Say hello.".into(),
                }],
            ),
        ],
        tools: vec![],
        max_tokens: 32,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
    };

    let rx = provider
        .stream(&request)
        .await
        .expect("live Gemini API stream should succeed");
    let events = collect_events(rx).await;

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !text.is_empty(),
        "live API should produce at least one text delta"
    );
    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::Done { .. })),
        "live API should produce a Done event"
    );
}
