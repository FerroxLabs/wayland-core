// Wiremock-based HTTP-error tests for BedrockProvider.
//
// The mutation baseline (E2E-MUTATION-BASELINE-2026-05-24.md) found that
// removing the `!` from `if !status.is_success()` in bedrock.rs survived all
// tests — meaning HTTP 400/401/403/429/500 responses silently passed through
// as success, getting parsed as valid JSON, with no test catching it.
//
// These tests cover both code paths that check status:
//   - `invoke_buffered()` (Cohere / Mistral family — buffered invoke endpoint)
//   - `stream()` Anthropic path   (invoke-with-response-stream endpoint)
//
// Bedrock uses SigV4 signing.  The wiremock server ignores signature headers,
// so we pass dummy credentials (`AwsCredentials::Explicit`) and use the
// test-only `new_with_endpoint_override` constructor to point the provider at
// the local mock instead of AWS.
//
// Retry note: Bedrock uses `with_retry` which wraps only the HTTP-send step.
// The closure returns `Ok(response)` for every HTTP response (status is NOT
// inspected inside the retry closure), so no status is retried — the mock
// receives exactly one request per test.

use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::ProviderError;
use wcore_providers::bedrock::{AwsCredentials, BedrockProvider};
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dummy_credentials() -> AwsCredentials {
    AwsCredentials::Explicit {
        access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
        session_token: None,
    }
}

/// Build a provider whose requests go to `server` instead of AWS.
fn provider_for(server: &MockServer) -> BedrockProvider {
    BedrockProvider::new_with_endpoint_override(
        "us-east-1",
        dummy_credentials(),
        false,
        ProviderCompat::default(),
        DebugConfig::default(),
        &server.uri(),
    )
}

/// A minimal request routed to a Cohere model (uses `invoke_buffered` path).
fn cohere_request() -> LlmRequest {
    LlmRequest {
        model: "cohere.command-r-v1:0".to_string(),
        system: String::new(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
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
        omit_max_tokens: false,
    }
}

/// A minimal request routed to an Anthropic-on-Bedrock model (stream path).
fn anthropic_request() -> LlmRequest {
    LlmRequest {
        model: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
        system: String::new(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
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
        omit_max_tokens: false,
    }
}

// ---------------------------------------------------------------------------
// invoke_buffered path (Cohere / Mistral) — status checks at bedrock.rs:388
// These tests directly verify that removing `!` from `if !status.is_success()`
// causes them to fail (the mutation the baseline reported as surviving).
// ---------------------------------------------------------------------------

/// 400 on the buffered invoke path → ProviderError::Api{status:400}
#[tokio::test]
async fn bedrock_invoke_buffered_400_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke$"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"message":"ValidationException: 1 validation error"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&cohere_request()).await;
    assert!(result.is_err(), "HTTP 400 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 400),
        e => panic!("expected Api(400), got: {e:?}"),
    }
}

/// 401 on the buffered invoke path → ProviderError::Api{status:401}
#[tokio::test]
async fn bedrock_invoke_buffered_401_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke$"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"message":"UnrecognizedException: The security token included in the request is invalid."}"#,
        ))
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&cohere_request()).await;
    assert!(result.is_err(), "HTTP 401 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 401),
        e => panic!("expected Api(401), got: {e:?}"),
    }
}

/// 403 on the buffered invoke path → ProviderError::Api{status:403}
#[tokio::test]
async fn bedrock_invoke_buffered_403_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke$"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            r#"{"message":"AccessDeniedException: User does not have access to the model"}"#,
        ))
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&cohere_request()).await;
    assert!(result.is_err(), "HTTP 403 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 403),
        e => panic!("expected Api(403), got: {e:?}"),
    }
}

/// 429 on the buffered invoke path → ProviderError::RateLimited
#[tokio::test]
async fn bedrock_invoke_buffered_429_surfaces_as_rate_limited() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke$"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string(r#"{"message":"ThrottlingException: Too many requests"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&cohere_request()).await;
    assert!(result.is_err(), "HTTP 429 must surface as an error");
    match result.unwrap_err() {
        ProviderError::RateLimited { .. } => {} // correct
        e => panic!("expected RateLimited, got: {e:?}"),
    }
}

/// 500 on the buffered invoke path → ProviderError::Api{status:500}
/// Bedrock's with_retry returns Ok(response) for HTTP errors — the status
/// check happens after the retry loop, so the mock receives exactly one call.
#[tokio::test]
async fn bedrock_invoke_buffered_500_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke$"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string(r#"{"message":"InternalServerException"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&cohere_request()).await;
    assert!(result.is_err(), "HTTP 500 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 500),
        e => panic!("expected Api(500), got: {e:?}"),
    }
}

/// 503 on the buffered invoke path → ProviderError::Api{status:503}
#[tokio::test]
async fn bedrock_invoke_buffered_503_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke$"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_string(r#"{"message":"ServiceUnavailableException"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&cohere_request()).await;
    assert!(result.is_err(), "HTTP 503 must surface as an error");
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 503),
        e => panic!("expected Api(503), got: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// stream() Anthropic path — status check at bedrock.rs:568
// Both mutations in the file are now covered.
// ---------------------------------------------------------------------------

/// 400 on the Anthropic streaming path → ProviderError::Api{status:400}
#[tokio::test]
async fn bedrock_stream_anthropic_400_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke-with-response-stream$"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"message":"ValidationException: invalid request"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&anthropic_request()).await;
    assert!(
        result.is_err(),
        "HTTP 400 on stream path must surface as an error"
    );
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 400),
        e => panic!("expected Api(400), got: {e:?}"),
    }
}

/// 401 on the Anthropic streaming path → ProviderError::Api{status:401}
/// This is the headline mutation the baseline reported at bedrock.rs:347 and
/// bedrock.rs:568 — removing `!` lets this through silently.
#[tokio::test]
async fn bedrock_stream_anthropic_401_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke-with-response-stream$"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"message":"UnrecognizedException: invalid security token"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&anthropic_request()).await;
    assert!(
        result.is_err(),
        "HTTP 401 on stream path must surface as an error"
    );
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 401),
        e => panic!("expected Api(401), got: {e:?}"),
    }
}

/// 403 on the Anthropic streaming path → ProviderError::Api{status:403}
#[tokio::test]
async fn bedrock_stream_anthropic_403_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke-with-response-stream$"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_string(r#"{"message":"AccessDeniedException: User not authorized"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&anthropic_request()).await;
    assert!(
        result.is_err(),
        "HTTP 403 on stream path must surface as an error"
    );
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 403),
        e => panic!("expected Api(403), got: {e:?}"),
    }
}

/// 429 on the Anthropic streaming path → ProviderError::RateLimited
#[tokio::test]
async fn bedrock_stream_anthropic_429_surfaces_as_rate_limited() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke-with-response-stream$"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string(r#"{"message":"ThrottlingException: Too many requests"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&anthropic_request()).await;
    assert!(
        result.is_err(),
        "HTTP 429 on stream path must surface as an error"
    );
    match result.unwrap_err() {
        ProviderError::RateLimited { .. } => {} // correct
        e => panic!("expected RateLimited, got: {e:?}"),
    }
}

/// 500 on the Anthropic streaming path → ProviderError::Api{status:500}
#[tokio::test]
async fn bedrock_stream_anthropic_500_surfaces_as_api_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke-with-response-stream$"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string(r#"{"message":"InternalServerException"}"#),
        )
        .mount(&server)
        .await;

    let result = provider_for(&server).stream(&anthropic_request()).await;
    assert!(
        result.is_err(),
        "HTTP 500 on stream path must surface as an error"
    );
    match result.unwrap_err() {
        ProviderError::Api { status, .. } => assert_eq!(status, 500),
        e => panic!("expected Api(500), got: {e:?}"),
    }
}

// ---------------------------------------------------------------------------
// Positive baseline: a 200 with garbage JSON body surfaces as a channel Error
// event (not a panic, not an Ok with wrong data). This pins the success path
// so mutations that negate the status check cannot silently "succeed" with the
// garbage body.
// ---------------------------------------------------------------------------

/// Garbage-JSON body on a 200 response on the buffered path → error in result.
/// If status-check mutant removes `!`, the garbage JSON is "parsed" and the
/// caller receives garbage or a parse error inside the returned events —
/// this test ensures the error is visible.
#[tokio::test]
async fn bedrock_invoke_buffered_garbage_json_200_surfaces_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path_regex(r".*/model/.*/invoke$"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json at all!!"))
        .mount(&server)
        .await;

    // The buffered path parses the body with `decode_buffered_response`.
    // A Cohere 200 with non-JSON body → ProviderError::Parse or Connection.
    let result = provider_for(&server).stream(&cohere_request()).await;
    assert!(
        result.is_err(),
        "garbage JSON on 200 buffered path must surface as error"
    );
}
