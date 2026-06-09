//! OpenRouter provider — OpenAI-API-compatible meta-router.
//!
//! Source: Forge `packages/core/src/providers/openrouter-provider.ts` (Apache-2.0).
//! OpenRouter aggregates 100+ models behind an OpenAI-compatible
//! chat-completions surface. Model ids follow the `vendor/model` format
//! (e.g. `"anthropic/claude-opus-4-7"`, `"openai/gpt-4o"`), and the API
//! itself is wire-compatible with OpenAI — so this adapter is a thin
//! newtype wrapper over [`OpenAIProvider`]. Vendor-specific quirks live in
//! [`OpenAIProvider`] and the [`ProviderCompat`] config; DO NOT add
//! hardcoded vendor conditionals here.
//!
//! Note on headers: Forge's OpenRouter wrapper sets custom HTTP-Referer
//! and X-Title headers for OpenRouter's analytics. We deliberately do
//! not replicate that in this lift — the underlying [`OpenAIProvider`]
//! sets only Authorization + Content-Type, which is the minimum OpenRouter
//! requires (the custom headers are recommended-but-not-mandatory). If
//! custom-header injection is needed later, it should be added at the
//! [`OpenAIProvider`] level (via [`ProviderCompat`]) so every adapter
//! benefits, not patched into this one.
//!
//! Register via [`register_openrouter_in`] against a [`ProviderRegistry`].
//! The id is lowercased to `"openrouter"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default OpenRouter base URL (OpenAI-compat surface).
///
/// F-006 fix: the base URL must NOT include `/v1` because the default
/// `api_path` in `ProviderCompat` is `/v1/chat/completions`. Concatenating
/// `…/api/v1` + `/v1/chat/completions` produced the double-prefix
/// `…/api/v1/v1/chat/completions` which returns a 404 on every request.
/// The correct URL is `…/api` so the final path is `…/api/v1/chat/completions`.
pub const OPENROUTER_DEFAULT_BASE_URL: &str = "https://openrouter.ai/api";

/// OpenRouter provider — delegates to [`OpenAIProvider`] over OpenRouter's
/// OpenAI-compatible endpoint. Model ids must use the `vendor/model`
/// format (e.g. `"anthropic/claude-opus-4-7"`).
pub struct OpenRouterProvider {
    inner: OpenAIProvider,
}

impl OpenRouterProvider {
    /// Construct with an explicit base URL (use [`OPENROUTER_DEFAULT_BASE_URL`]
    /// for production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with OpenRouter's default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, OPENROUTER_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for OpenRouterProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

/// Register an OpenRouter factory in the given registry under the lowercased
/// id `"openrouter"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_openrouter_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(OpenRouterProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("openrouter", factory)
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

    /// F-006 regression: OPENROUTER_DEFAULT_BASE_URL must NOT end with `/v1`.
    /// The default api_path is `/v1/chat/completions`, so ending the base URL
    /// with `/v1` would build `/v1/v1/chat/completions` → 404 on every request.
    #[test]
    fn default_base_url_does_not_end_with_v1() {
        assert!(
            !OPENROUTER_DEFAULT_BASE_URL.ends_with("/v1"),
            "OPENROUTER_DEFAULT_BASE_URL must not end with '/v1' to avoid the \
             double-prefix bug: base_url + api_path_default would produce \
             /v1/v1/chat/completions"
        );
    }

    #[test]
    fn default_url_plus_api_path_produces_single_v1() {
        // build_url mirrors openai.rs: format!("{}{}", base_url, compat.api_path())
        let base = OPENROUTER_DEFAULT_BASE_URL;
        let api_path = wcore_config::compat::ProviderCompat::default()
            .api_path()
            .to_string();
        let full = format!("{}{}", base, api_path);
        let v1_count = full.matches("/v1/").count();
        assert_eq!(
            v1_count, 1,
            "Constructed URL should contain exactly one '/v1/' segment, got: {full}"
        );
        assert!(
            full.ends_with("/v1/chat/completions"),
            "Constructed URL must end with '/v1/chat/completions', got: {full}"
        );
    }

    #[test]
    fn constructs_with_default_url() {
        let p = OpenRouterProvider::with_defaults(
            "sk-or-test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        // No panic; constructor accepts the default base URL.
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = OpenRouterProvider::new(
            "sk-or-test",
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
        let p = OpenRouterProvider::new(
            "sk-or-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "anthropic/claude-opus-4-7".into(),
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
        register_openrouter_in(
            &mut r,
            "sk-or-test".into(),
            OPENROUTER_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("openrouter").is_some());
        assert!(r.get("OpenRouter").is_none());
        assert!(r.get("OPENROUTER").is_none());
    }

    #[test]
    fn compat_config_passes_through() {
        // The wrapper must forward ProviderCompat to OpenAIProvider. Constructing
        // with a non-default compat must not panic and the registered factory
        // must return a usable provider.
        let mut r = WaylandProviderRegistry::new();
        register_openrouter_in(
            &mut r,
            "sk-or-test".into(),
            OPENROUTER_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("openrouter").is_some());
    }

    #[test]
    fn accepts_vendor_slashed_model_id() {
        // Provider-specific: OpenRouter model ids must use vendor/model format.
        // The adapter must not reject or rewrite slashed ids — the underlying
        // OpenAIProvider passes the model field through unchanged to the
        // wire-level body, so a vendor-slashed string round-trips fine.
        let p = OpenRouterProvider::new(
            "sk-or-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        // We can't peek into the request body without an HTTP roundtrip, but
        // calling stream() with a vendor-slashed model id must reach the
        // network layer (and fail there with a connection error), not panic
        // earlier inside the adapter.
        let req = LlmRequest {
            model: "anthropic/claude-opus-4-7".into(),
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
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(p.stream(&req));
        assert!(
            result.is_err(),
            "vendor-slashed model id should reach the network path and fail there"
        );
    }
}
