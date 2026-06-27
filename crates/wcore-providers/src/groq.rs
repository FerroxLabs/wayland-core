//! Groq provider — OpenAI-API-compatible.
//!
//! Source: Forge `packages/core/src/providers/groq-provider.ts` (Apache-2.0).
//! Groq hosts open-weight models (Llama, Mixtral) behind an OpenAI-compatible
//! chat-completions surface, so this adapter is a thin newtype wrapper over
//! [`OpenAIProvider`]. Provider-specific shaping (hybrid tool-calling parser,
//! reasoning surfaces, etc.) belongs in [`OpenAIProvider`] and the
//! [`ProviderCompat`] config — DO NOT add hardcoded provider conditionals here.
//!
//! Register via [`register_groq_in`] against a [`ProviderRegistry`]. The id is
//! lowercased to `"groq"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Groq base URL (OpenAI-compat surface).
pub const GROQ_DEFAULT_BASE_URL: &str = "https://api.groq.com/openai";

/// Groq provider — delegates to [`OpenAIProvider`] over Groq's
/// OpenAI-compatible endpoint.
pub struct GroqProvider {
    inner: OpenAIProvider,
}

impl GroqProvider {
    /// Construct with an explicit base URL (use [`GROQ_DEFAULT_BASE_URL`] for
    /// production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with Groq's default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, GROQ_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for GroqProvider {
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

/// Register a Groq factory in the given registry under the lowercased id
/// `"groq"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_groq_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(GroqProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("groq", factory)
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
        let p = GroqProvider::with_defaults(
            "gsk_test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = GroqProvider::new(
            "gsk_test",
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
        let p = GroqProvider::new(
            "gsk_test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "llama-3.3-70b-versatile".into(),
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

    #[test]
    fn register_uses_lowercase_id() {
        let mut r = WaylandProviderRegistry::new();
        register_groq_in(
            &mut r,
            "gsk_test".into(),
            GROQ_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("groq").is_some());
        assert!(r.get("Groq").is_none());
        assert!(r.get("GROQ").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_groq_in(
            &mut r,
            "gsk_test".into(),
            GROQ_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_groq_in(
            &mut r,
            "gsk_other".into(),
            GROQ_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(second, Err(RegistryError::DuplicateId(_))));
    }

    #[tokio::test]
    async fn list_models_does_a_live_fetch_not_the_alias_floor() {
        // Regression (E2E-found): the OpenAI-compat newtypes must forward
        // `list_models` to their inner `OpenAIProvider` (a live `GET /v1/models`),
        // NOT inherit the trait-default alias catalog. Before the fix, Groq's
        // `list_models` returned the (empty) alias floor, so paste-to-connect
        // validation could never authenticate Groq/OpenRouter/Cerebras/Deepseek
        // even with valid keys — they all fell through to the picker.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"data":[{"id":"llama-3.3-70b-versatile"},{"id":"mixtral-8x7b"}]}"#,
            ))
            .mount(&server)
            .await;

        let p = GroqProvider::new(
            "gsk_test",
            &server.uri(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let models = p.list_models().await.expect("list_models");
        assert_eq!(
            models.len(),
            2,
            "Groq must surface the live model catalog, not the empty alias floor"
        );
        assert!(models.iter().any(|m| m.id == "llama-3.3-70b-versatile"));
    }

    #[test]
    fn compat_config_passes_through() {
        // The wrapper must forward ProviderCompat to OpenAIProvider. We can't
        // peek inside, but constructing with a non-default compat must not
        // panic and the registered factory must return a usable provider.
        let mut r = WaylandProviderRegistry::new();
        register_groq_in(
            &mut r,
            "gsk_test".into(),
            GROQ_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("groq").is_some());
    }
}
