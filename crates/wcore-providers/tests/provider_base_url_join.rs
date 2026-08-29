//! #1178 on the wires the original fix did not reach.
//!
//! #1178 closed the `/v1`-doubling for every OpenAI-compat endpoint by routing
//! `openai.rs` through `wcore_config::compat::join_endpoint`. The Anthropic
//! wire carried the IDENTICAL defect, unfixed and untested: `try_stream` built
//! its URL with a bare `format!("{}/v1/messages", base_url)` — no join, and
//! unlike its neighbours not even a `trim_end_matches('/')` — while
//! `--base-url` / `[providers.anthropic].base_url` flow into it verbatim.
//!
//! MEASURED live against api.anthropic.com on 2026-08-29, with the working
//! spelling as the positive control:
//!
//! ```text
//! POST https://api.anthropic.com/v1/messages    -> 401   (routed)
//! POST https://api.anthropic.com/v1/v1/messages -> 404
//! POST https://api.anthropic.com//v1/messages   -> 404
//! ```
//!
//! Both broken spellings are ones a user copies straight out of Anthropic's own
//! docs, and MiniMax already ships an Anthropic-wire base URL by default
//! (`https://api.minimax.io/anthropic`). The trailing-slash arm is strictly
//! worse here than it ever was on the OpenAI wire: api.openai.com tolerates the
//! double slash, api.anthropic.com does not.
//!
//! These tests assert the PATH THAT WENT ON THE WIRE, read back from the mock
//! server, rather than the return value of a helper — so they hold against the
//! pre-fix source, which had no helper to call.

use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_providers::cohere::CohereProvider;
use wcore_providers::gemini::GeminiProvider;
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};

fn minimal_request() -> LlmRequest {
    LlmRequest {
        flux_loop_intent: None,
        flux_turn_nonce: None,
        model: "test-model".to_string(),
        system: String::new(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
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
        omit_max_tokens: false,
        routed_model_hint: None,
        replay_reasoning_content: false,
    }
}

/// The four spellings of one base URL that a user can reasonably type. The bare
/// root is the control: it worked before the fix and must still work after.
fn spellings(root: &str, suffix: &str) -> Vec<String> {
    vec![
        root.to_string(),
        format!("{root}/"),
        format!("{root}{suffix}"),
        format!("{root}{suffix}/"),
    ]
}

/// Read back the single path the provider actually requested.
async fn only_requested_path(server: &MockServer) -> String {
    let requests = server
        .received_requests()
        .await
        .expect("mock server must record requests");
    assert_eq!(
        requests.len(),
        1,
        "expected exactly one request, got {}",
        requests.len()
    );
    requests[0].url.path().to_string()
}

/// THE DEFECT. Every spelling of an Anthropic base URL must reach
/// `/v1/messages` exactly once.
#[tokio::test]
async fn anthropic_messages_path_is_identical_for_every_base_spelling() {
    for base in spellings("", "/v1") {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ))
            .mount(&server)
            .await;

        let base_url = format!("{}{base}", server.uri());
        let provider = AnthropicProvider::new(
            "test-key",
            &base_url,
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let _ = provider.stream(&minimal_request()).await;

        assert_eq!(
            only_requested_path(&server).await,
            "/v1/messages",
            "base_url {base_url:?} must reach the single-/v1 messages endpoint"
        );
    }
}

/// `GET /v1/models` is derived from the same base and carried the `/v1` half of
/// the same bug (`trim_end_matches` fixed only the trailing slash).
#[tokio::test]
async fn anthropic_models_path_is_identical_for_every_base_spelling() {
    for base in spellings("", "/v1") {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":[]}"#))
            .mount(&server)
            .await;

        let base_url = format!("{}{base}", server.uri());
        let provider = AnthropicProvider::new(
            "test-key",
            &base_url,
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let _ = provider.list_models().await;

        assert_eq!(
            only_requested_path(&server).await,
            "/v1/models",
            "base_url {base_url:?} must reach the single-/v1 models endpoint"
        );
    }
}

/// The untrimmed-slash half of the same class. Cohere's own default base URL
/// already ends in `/v1`, so a user who writes it with a trailing slash built
/// `//chat`.
#[tokio::test]
async fn cohere_chat_path_is_identical_for_every_base_spelling() {
    for base in spellings("", "/v1") {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "{\"event_type\":\"stream-end\",\"finish_reason\":\"COMPLETE\"}\n",
            ))
            .mount(&server)
            .await;

        let base_url = format!("{}{base}", server.uri());
        let provider =
            CohereProvider::new("test-key", &base_url, "command-r", DebugConfig::default());
        let _ = provider.stream(&minimal_request()).await;

        let path = only_requested_path(&server).await;
        let expected = if base.starts_with("/v1") { "/v1/chat" } else { "/chat" };
        assert_eq!(
            path, expected,
            "base_url {base_url:?} must reach a single, unduplicated chat path"
        );
    }
}

/// Gemini publishes its endpoint WITH the `/v1beta` in it, so the same class
/// applies to the native Gemini wire.
#[tokio::test]
async fn gemini_generate_path_is_identical_for_every_base_spelling() {
    for base in spellings("", "/v1beta") {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("data: {}\n\n"))
            .mount(&server)
            .await;

        let base_url = format!("{}{base}", server.uri());
        let provider = GeminiProvider::new(
            "test-key",
            &base_url,
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = provider.stream(&minimal_request()).await;

        assert_eq!(
            only_requested_path(&server).await,
            "/v1beta/models/test-model:streamGenerateContent",
            "base_url {base_url:?} must reach the single-/v1beta generate endpoint"
        );
    }
}

/// NEGATIVE CONTROL — must hold in BOTH arms. The overlap is matched on whole
/// path SEGMENTS, so a proxy mounted under a prefix that merely ENDS in the
/// same characters keeps the full suffix. Collapsing these would silently break
/// a working deployment, which is the failure mode the fix must not introduce.
#[tokio::test]
async fn a_base_path_that_only_resembles_the_api_prefix_is_not_collapsed() {
    for (prefix, expected) in [
        ("/apiv1", "/apiv1/v1/messages"),
        ("/v10", "/v10/v1/messages"),
        ("/anthropic", "/anthropic/v1/messages"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ))
            .mount(&server)
            .await;

        let base_url = format!("{}{prefix}", server.uri());
        let provider = AnthropicProvider::new(
            "test-key",
            &base_url,
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        );
        let _ = provider.stream(&minimal_request()).await;

        assert_eq!(
            only_requested_path(&server).await,
            expected,
            "base_url {base_url:?} must keep its full path prefix"
        );
    }
}
