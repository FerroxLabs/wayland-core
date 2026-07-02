//! DeepSeek provider — OpenAI-API-compatible.
//!
//! Source: Forge `packages/core/src/providers/deepseek-provider.ts` (Apache-2.0).
//! DeepSeek exposes an OpenAI-compatible chat-completions surface, so this
//! adapter is a thin newtype wrapper over [`OpenAIProvider`]. Provider-specific
//! quirks (cache-hit token accounting, `reasoning_content` shaping for
//! deepseek-reasoner, etc.) live in [`OpenAIProvider`] and the
//! [`ProviderCompat`] config — DO NOT add hardcoded provider conditionals here.
//!
//! Register via [`register_deepseek_in`] against a [`ProviderRegistry`]. The
//! id is lowercased to `"deepseek"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default DeepSeek base URL (OpenAI-compat surface).
pub const DEEPSEEK_DEFAULT_BASE_URL: &str = "https://api.deepseek.com";

/// DeepSeek provider — delegates to [`OpenAIProvider`] over DeepSeek's
/// OpenAI-compatible endpoint.
pub struct DeepSeekProvider {
    inner: OpenAIProvider,
}

impl DeepSeekProvider {
    /// Construct with an explicit base URL (use [`DEEPSEEK_DEFAULT_BASE_URL`]
    /// for production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with DeepSeek's default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, DEEPSEEK_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for DeepSeekProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }

    async fn list_models(&self) -> anyhow::Result<Vec<crate::ModelInfo>> {
        self.inner.list_models().await
    }
}

/// Register a DeepSeek factory in the given registry under the lowercased id
/// `"deepseek"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_deepseek_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(DeepSeekProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("deepseek", factory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::WaylandProviderRegistry;

    fn compat_with_max_tokens_field(field: &str) -> ProviderCompat {
        ProviderCompat {
            max_tokens_field: Some(field.into()),
            ..Default::default()
        }
    }

    #[test]
    fn constructs_with_default_url() {
        let p = DeepSeekProvider::with_defaults(
            "sk-test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        // No panic and the wrapper is alive. We can't reach the private inner
        // base_url; the next test exercises a custom URL to verify the
        // constructor path accepts overrides.
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = DeepSeekProvider::new(
            "sk-test",
            "http://localhost:9999",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path.
        let p = DeepSeekProvider::new(
            "sk-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "deepseek-chat".into(),
            system: String::new(),
            messages: vec![],
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
        let result = p.stream(&req).await;
        assert!(result.is_err(), "expected error from unreachable host");
    }

    /// #139 escalation pin: the desktop's direct-DeepSeek selection resolves
    /// THIS provider. Prove the newtype delegation reaches OpenAIProvider's
    /// ENCODED request build — a dirty `Browser::execute` tool must hit the
    /// wire as its `wct_`-encoded form, and a clean name byte-identical. If a
    /// future refactor gives DeepSeekProvider its own body builder without
    /// the codec, this test fails.
    #[tokio::test]
    async fn dirty_tool_names_are_encoded_on_the_deepseek_wire() {
        use wcore_types::llm::Message;
        use wcore_types::llm::Role;
        use wcore_types::message::ContentBlock;
        use wcore_types::tool::ToolDef;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",",
            "\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let p = DeepSeekProvider::new(
            "sk-test",
            &server.uri(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "deepseek-v4-flash".into(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".into() }],
            )],
            tools: vec![
                ToolDef {
                    name: "Browser::execute".into(),
                    description: "dirty".into(),
                    input_schema: serde_json::json!({"type":"object","properties":{}}),
                    deferred: false,
                    server: None,
                },
                ToolDef {
                    name: "get_weather".into(),
                    description: "clean".into(),
                    input_schema: serde_json::json!({"type":"object","properties":{}}),
                    deferred: false,
                    server: None,
                },
            ],
            max_tokens: 16,
            ..Default::default()
        };
        let mut rx = p.stream(&req).await.expect("stream ok");
        while rx.recv().await.is_some() {}

        let reqs = server.received_requests().await.expect("requests recorded");
        assert_eq!(reqs.len(), 1, "exactly one wire request");
        let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).expect("json body");
        let names: Vec<&str> = body["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["function"]["name"].as_str().expect("name"))
            .collect();
        let legal = |n: &str| {
            !n.is_empty()
                && n.len() <= 64
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        assert!(
            names.contains(&"wct_Browser_3A_3Aexecute"),
            "dirty name must be wct_-encoded on the DeepSeek wire, got {names:?}"
        );
        assert!(
            names.contains(&"get_weather"),
            "clean name must pass through byte-identical, got {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("::") || n.contains('.')),
            "no raw dirty name may reach the wire: {names:?}"
        );
        assert!(
            names.iter().all(|n| legal(n)),
            "every wire name must match ^[a-zA-Z0-9_-]{{1,64}}$: {names:?}"
        );
    }

    #[test]
    fn register_uses_lowercase_id() {
        let mut r = WaylandProviderRegistry::new();
        register_deepseek_in(
            &mut r,
            "sk-test".into(),
            DEEPSEEK_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("deepseek").is_some());
        assert!(r.get("DeepSeek").is_none());
        assert!(r.get("DEEPSEEK").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_deepseek_in(
            &mut r,
            "sk-test".into(),
            DEEPSEEK_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_deepseek_in(
            &mut r,
            "sk-other".into(),
            DEEPSEEK_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(second, Err(RegistryError::DuplicateId(_))));
    }

    #[test]
    fn compat_config_passes_through() {
        // The wrapper must forward ProviderCompat to OpenAIProvider. We can't
        // peek inside, but constructing with a non-default compat must not
        // panic and the registered factory must return a usable provider.
        let mut r = WaylandProviderRegistry::new();
        register_deepseek_in(
            &mut r,
            "sk-test".into(),
            DEEPSEEK_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("deepseek").is_some());
    }
}
