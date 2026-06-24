//! Sakana AI provider — the "Fugu" OpenAI-API-compatible endpoint.
//!
//! Sakana's `https://api.sakana.ai/v1` surface speaks the standard OpenAI
//! chat-completions wire format (Bearer auth, `/v1/chat/completions`, SSE
//! `chat.completion.chunk` frames terminated by `[DONE]`, OpenAI error
//! envelopes — verified live). Fugu itself is a multi-agent orchestration
//! layer that routes across upstream frontier models. Like
//! [`crate::flux_router::FluxRouterProvider`], this adapter is a thin newtype
//! over [`OpenAIProvider`]; vendor quirks belong in [`OpenAIProvider`] and
//! [`ProviderCompat`], not here.
//!
//! Models: `fugu` (default), `fugu-ultra`, `fugu-ultra-20260615`.
//! Register via [`register_sakana_in`] against a [`ProviderRegistry`]. The id
//! is lowercased to `"sakana"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Sakana base URL (OpenAI-compat surface). Override via `base_url` in
/// config or `--base-url`.
pub const SAKANA_DEFAULT_BASE_URL: &str = "https://api.sakana.ai/v1";

/// Sakana ("Fugu") provider — delegates to [`OpenAIProvider`] over Sakana's
/// OpenAI-compatible endpoint.
pub struct SakanaProvider {
    inner: OpenAIProvider,
}

impl SakanaProvider {
    /// Construct with an explicit base URL.
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with Sakana's default base URL ([`SAKANA_DEFAULT_BASE_URL`]).
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, SAKANA_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for SakanaProvider {
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

/// Register a Sakana factory in the given registry under the lowercased id
/// `"sakana"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_sakana_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(SakanaProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("sakana", factory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::WaylandProviderRegistry;

    #[test]
    fn constructs_with_default_url() {
        let p = SakanaProvider::with_defaults(
            "fish_test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = SakanaProvider::new(
            "fish_test",
            "http://localhost:9999",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn default_base_url_is_sakana_v1() {
        assert_eq!(SAKANA_DEFAULT_BASE_URL, "https://api.sakana.ai/v1");
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path.
        let p = SakanaProvider::new(
            "fish_test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "fugu".into(),
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
        };
        let result = p.stream(&req).await;
        assert!(result.is_err(), "expected error from unreachable host");
    }

    #[test]
    fn register_uses_lowercase_id() {
        let mut r = WaylandProviderRegistry::new();
        register_sakana_in(
            &mut r,
            "fish_test".into(),
            SAKANA_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("sakana").is_some());
        assert!(r.get("Sakana").is_none());
        assert!(r.get("SAKANA").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_sakana_in(
            &mut r,
            "fish_test".into(),
            SAKANA_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_sakana_in(
            &mut r,
            "fish_other".into(),
            SAKANA_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(second, Err(RegistryError::DuplicateId(_))));
    }
}
