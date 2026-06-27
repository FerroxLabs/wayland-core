//! Fireworks AI provider — OpenAI-API-compatible.
//!
//! Fireworks AI hosts open-weight models (Llama, Mixtral, Qwen, DeepSeek and
//! others) behind an OpenAI-compatible chat-completions surface, so this
//! adapter is a thin newtype wrapper over [`OpenAIProvider`]. Provider-specific
//! shaping belongs in [`OpenAIProvider`] and the [`ProviderCompat`] config —
//! DO NOT add hardcoded provider conditionals here.
//!
//! Auth is a standard `Authorization: Bearer …` header carrying the Fireworks
//! API key, handled by [`OpenAIProvider`]. One Fireworks-specific quirk worth
//! noting: model ids are namespaced in the form
//! `accounts/fireworks/models/<name>` (e.g.
//! `accounts/fireworks/models/llama-v3p1-70b-instruct`). The full namespaced
//! string is passed through verbatim as `LlmRequest::model` — Fireworks expects
//! it that way and this wrapper does no rewriting.
//!
//! Register via [`register_fireworks_in`] against a [`ProviderRegistry`]. The
//! id is lowercased to `"fireworks"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Fireworks AI base URL (OpenAI-compat surface).
pub const FIREWORKS_DEFAULT_BASE_URL: &str = "https://api.fireworks.ai/inference/v1";

/// Fireworks AI provider — delegates to [`OpenAIProvider`] over Fireworks'
/// OpenAI-compatible endpoint.
pub struct FireworksProvider {
    inner: OpenAIProvider,
}

impl FireworksProvider {
    /// Construct with an explicit base URL (use [`FIREWORKS_DEFAULT_BASE_URL`]
    /// for production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with Fireworks' default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, FIREWORKS_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for FireworksProvider {
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

/// Register a Fireworks factory in the given registry under the lowercased id
/// `"fireworks"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_fireworks_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(FireworksProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("fireworks", factory)
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
        let p = FireworksProvider::with_defaults(
            "fw_test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn default_base_url_is_fireworks_inference_v1() {
        // The production base URL must point at Fireworks' OpenAI-compat
        // inference surface — not the root host.
        assert_eq!(
            FIREWORKS_DEFAULT_BASE_URL,
            "https://api.fireworks.ai/inference/v1"
        );
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = FireworksProvider::new(
            "fw_test",
            "http://localhost:9999",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path. The
        // model id uses Fireworks' namespaced `accounts/…` form to exercise
        // request-body construction with a realistic id.
        let p = FireworksProvider::new(
            "fw_test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "accounts/fireworks/models/llama-v3p1-70b-instruct".into(),
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
        register_fireworks_in(
            &mut r,
            "fw_test".into(),
            FIREWORKS_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("fireworks").is_some());
        assert!(r.get("Fireworks").is_none());
        assert!(r.get("FIREWORKS").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_fireworks_in(
            &mut r,
            "fw_test".into(),
            FIREWORKS_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_fireworks_in(
            &mut r,
            "fw_other".into(),
            FIREWORKS_DEFAULT_BASE_URL.into(),
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
        register_fireworks_in(
            &mut r,
            "fw_test".into(),
            FIREWORKS_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("fireworks").is_some());
    }
}
