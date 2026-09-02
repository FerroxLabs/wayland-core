// Integration test for the Cohere chat endpoint URL join.
//
// FerroxLabs/wayland#1217 c3. `stream()` built the chat URL with a bare
// `format!("{}/chat", self.base_url)` — the untrimmed-slash half of the same
// defect the Anthropic wire carried, and the reason c3 names cohere.rs in the
// same pass. `COHERE_DEFAULT_BASE_URL` already ends in `/v1`, so a user who
// copies their gateway URL with the trailing slash Cohere's own console shows
// (`https://api.cohere.com/v1/`) built `//chat`.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::cohere::CohereProvider;
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};

fn minimal_request() -> LlmRequest {
    LlmRequest {
        flux_loop_intent: None,
        flux_turn_nonce: None,
        model: "command-r-plus-08-2024".to_string(),
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
        routed_model_hint: None,
        replay_reasoning_content: false,
    }
}

/// The cross product, not one example: {no trailing slash, trailing slash} ×
/// {bare root, `/v1`}. The observable is the path the mock server actually
/// receives — the mock answers ONLY `/v1/chat`, and the assertion reads the
/// request log, so it cannot pass while the bytes go somewhere else.
#[tokio::test]
async fn cohere_base_url_spellings_all_post_exactly_one_chat() {
    for suffix in ["/v1", "/v1/"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("data: {\"type\":\"stream-end\"}\n\n", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let base = format!("{}{suffix}", server.uri());
        let provider = CohereProvider::new(
            "test-api-key",
            &base,
            "command-r-plus-08-2024",
            DebugConfig::default(),
        );

        let rx = provider
            .stream(&minimal_request())
            .await
            .unwrap_or_else(|e| panic!("base_url {base:?} must reach /v1/chat, got {e:?}"));
        drop(rx);

        let received = server.received_requests().await.expect("request log");
        assert_eq!(received.len(), 1, "base_url {base:?}: {received:?}");
        assert_eq!(
            received[0].url.path(),
            "/v1/chat",
            "base_url {base:?} dialed the wrong path"
        );
    }
}

/// The wrong-collapse control: a base carrying a REAL path prefix keeps it, and
/// a segment that merely CONTAINS the appended one is not collapsed. A fix that
/// forced the path to `/v1/chat`, or that stripped the base's path, would pass
/// the row above and fail this one.
#[tokio::test]
async fn cohere_base_url_path_prefix_is_preserved_not_collapsed() {
    for (suffix, expected) in [
        ("/gateway/v1", "/gateway/v1/chat"),
        ("/gateway/v1/", "/gateway/v1/chat"),
        ("/chatty", "/chatty/chat"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(expected))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("data: {\"type\":\"stream-end\"}\n\n", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let base = format!("{}{suffix}", server.uri());
        let provider = CohereProvider::new(
            "test-api-key",
            &base,
            "command-r-plus-08-2024",
            DebugConfig::default(),
        );

        let rx = provider
            .stream(&minimal_request())
            .await
            .unwrap_or_else(|e| panic!("base_url {base:?} must reach {expected}, got {e:?}"));
        drop(rx);

        let received = server.received_requests().await.expect("request log");
        assert_eq!(
            received[0].url.path(),
            expected,
            "base_url {base:?} dialed the wrong path"
        );
    }
}
