//! Flux Router provider — Sean's own OpenAI-API-compatible router product.
//!
//! Like [`crate::openrouter::OpenRouterProvider`], Flux Router fronts multiple
//! upstream models behind a single OpenAI-compatible chat-completions surface.
//! This adapter is a thin newtype wrapper over [`OpenAIProvider`]; vendor
//! quirks belong in [`OpenAIProvider`] and [`ProviderCompat`], not here.
//!
//! v0.8.1 task U10a. The default base URL is a placeholder
//! (`FLUX_ROUTER_DEFAULT_BASE_URL`) and should be overridden via config or
//! `--base-url` until the production endpoint is finalized.
//!
//! Register via [`register_flux_router_in`] against a [`ProviderRegistry`].
//! The id is lowercased to `"flux-router"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Flux Router base URL (OpenAI-compat surface).
///
/// Placeholder until Sean finalizes the production endpoint. Set
/// `base_url` in config (or `--base-url`) to override.
pub const FLUX_ROUTER_DEFAULT_BASE_URL: &str = "https://api.fluxrouter.ai/v1";

/// Flux Router provider — delegates to [`OpenAIProvider`] over Flux's
/// OpenAI-compatible endpoint.
pub struct FluxRouterProvider {
    inner: OpenAIProvider,
}

impl FluxRouterProvider {
    /// Construct with an explicit base URL (use [`FLUX_ROUTER_DEFAULT_BASE_URL`]
    /// only as a placeholder; override via config in production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with Flux Router's default placeholder base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, FLUX_ROUTER_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for FluxRouterProvider {
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

/// Register a Flux Router factory in the given registry under the lowercased
/// id `"flux-router"`. The factory captures the provided `api_key`,
/// `base_url`, `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_flux_router_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(FluxRouterProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("flux-router", factory)
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
        let p = FluxRouterProvider::with_defaults(
            "flux-test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = FluxRouterProvider::new(
            "flux-test",
            "http://localhost:9999",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn default_base_url_is_flux_v1() {
        assert_eq!(FLUX_ROUTER_DEFAULT_BASE_URL, "https://api.fluxrouter.ai/v1");
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path.
        let p = FluxRouterProvider::new(
            "flux-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "gpt-4o".into(),
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
        register_flux_router_in(
            &mut r,
            "flux-test".into(),
            FLUX_ROUTER_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("flux-router").is_some());
        assert!(r.get("Flux-Router").is_none());
        assert!(r.get("FLUX-ROUTER").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_flux_router_in(
            &mut r,
            "flux-test".into(),
            FLUX_ROUTER_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_flux_router_in(
            &mut r,
            "flux-other".into(),
            FLUX_ROUTER_DEFAULT_BASE_URL.into(),
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
        register_flux_router_in(
            &mut r,
            "flux-test".into(),
            FLUX_ROUTER_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("flux-router").is_some());
    }
}
