//! Perplexity provider — OpenAI-API-compatible.
//!
//! Perplexity exposes an OpenAI-compatible chat-completions surface at
//! `https://api.perplexity.ai`, so this adapter is a thin newtype wrapper over
//! [`OpenAIProvider`]. The defining feature of Perplexity is the `sonar` model
//! family (`sonar`, `sonar-pro`, `sonar-reasoning`, `sonar-deep-research`,
//! etc.): these are online-search-augmented models that ground responses in
//! live web results. That augmentation is a model-side capability — the wire
//! protocol is plain OpenAI `/chat/completions`, so no provider-specific
//! request shaping is needed here. Provider-specific quirks (if any surface
//! later) belong in [`OpenAIProvider`] and the [`ProviderCompat`] config —
//! DO NOT add hardcoded provider conditionals here.
//!
//! Register via [`register_perplexity_in`] against a [`ProviderRegistry`]. The
//! id is lowercased to `"perplexity"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Perplexity base URL (OpenAI-compat surface).
pub const PERPLEXITY_DEFAULT_BASE_URL: &str = "https://api.perplexity.ai";

/// Perplexity provider — delegates to [`OpenAIProvider`] over Perplexity's
/// OpenAI-compatible endpoint. Use the `sonar` model family for
/// online-search-augmented completions.
pub struct PerplexityProvider {
    inner: OpenAIProvider,
}

impl PerplexityProvider {
    /// Construct with an explicit base URL (use [`PERPLEXITY_DEFAULT_BASE_URL`]
    /// for production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with Perplexity's default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, PERPLEXITY_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for PerplexityProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

/// Register a Perplexity factory in the given registry under the lowercased id
/// `"perplexity"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_perplexity_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(PerplexityProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("perplexity", factory)
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
        let p = PerplexityProvider::with_defaults(
            "pplx-test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = PerplexityProvider::new(
            "pplx-test",
            "http://localhost:9999",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn default_base_url_is_perplexity_api() {
        // The production base URL must be Perplexity's OpenAI-compat endpoint.
        assert_eq!(PERPLEXITY_DEFAULT_BASE_URL, "https://api.perplexity.ai");
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path (which
        // sets the `Authorization: Bearer …` header). Exercises the request
        // body construction against a `sonar` model.
        let p = PerplexityProvider::new(
            "pplx-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "sonar".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 16,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
        };
        let result = p.stream(&req).await;
        assert!(result.is_err(), "expected error from unreachable host");
    }

    #[test]
    fn register_uses_lowercase_id() {
        let mut r = WaylandProviderRegistry::new();
        register_perplexity_in(
            &mut r,
            "pplx-test".into(),
            PERPLEXITY_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("perplexity").is_some());
        assert!(r.get("Perplexity").is_none());
        assert!(r.get("PERPLEXITY").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_perplexity_in(
            &mut r,
            "pplx-test".into(),
            PERPLEXITY_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_perplexity_in(
            &mut r,
            "pplx-other".into(),
            PERPLEXITY_DEFAULT_BASE_URL.into(),
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
        register_perplexity_in(
            &mut r,
            "pplx-test".into(),
            PERPLEXITY_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("perplexity").is_some());
    }
}
