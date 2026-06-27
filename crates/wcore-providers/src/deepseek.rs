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
