//! xAI (Grok) provider — OpenAI-API-compatible.
//!
//! Source: Forge `packages/core/src/providers/xai-provider.ts` (Apache-2.0).
//! xAI exposes an OpenAI-compatible chat-completions surface, so this adapter
//! is a thin newtype wrapper over [`OpenAIProvider`]. Provider-specific quirks
//! (if any) live in [`OpenAIProvider`] and the [`ProviderCompat`] config — DO
//! NOT add hardcoded provider conditionals here.
//!
//! Register via [`register_xai_in`] against a [`ProviderRegistry`]. The id is
//! lowercased to `"xai"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default xAI base URL (OpenAI-compat surface).
pub const XAI_DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";

/// xAI (Grok) provider — delegates to [`OpenAIProvider`] over xAI's
/// OpenAI-compatible endpoint.
pub struct XaiProvider {
    inner: OpenAIProvider,
}

impl XaiProvider {
    /// Construct with an explicit base URL (use [`XAI_DEFAULT_BASE_URL`] for
    /// production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with xAI's default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, XAI_DEFAULT_BASE_URL, compat, debug)
    }

    /// Construct over an async OAuth bearer source (Grok "Sign in with X").
    /// The token is refreshed near expiry on every turn, so a Grok session
    /// survives the ~6h access-token lifetime without the host re-spawning —
    /// matching how the engine already handles "Sign in with ChatGPT".
    pub fn with_bearer(
        bearer: crate::AsyncTokenSource,
        base_url: &str,
        compat: ProviderCompat,
        debug: DebugConfig,
    ) -> Self {
        Self {
            inner: OpenAIProvider::with_bearer(bearer, base_url, compat, debug),
        }
    }
}

#[async_trait]
impl LlmProvider for XaiProvider {
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

/// Register an xAI factory in the given registry under the lowercased id
/// `"xai"`. The factory captures the provided `api_key`, `base_url`, `compat`,
/// and `debug` and constructs a fresh provider per call.
pub fn register_xai_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(XaiProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("xai", factory)
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
        let p = XaiProvider::with_defaults(
            "sk-test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        // No panic; wrapper alive. Custom-URL test below verifies override path.
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = XaiProvider::new(
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
        // not panic, and not hang. Reuses OpenAIProvider's error path (which
        // is where 401 / malformed-request mapping also lives).
        let p = XaiProvider::new(
            "sk-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "grok-4-0709".into(),
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
        register_xai_in(
            &mut r,
            "sk-test".into(),
            XAI_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("xai").is_some());
        assert!(r.get("Xai").is_none());
        assert!(r.get("XAI").is_none());
    }

    #[test]
    fn compat_config_passes_through() {
        // The wrapper must forward ProviderCompat to OpenAIProvider. We can't
        // peek inside, but constructing with a non-default compat must not
        // panic and the registered factory must return a usable provider.
        let mut r = WaylandProviderRegistry::new();
        register_xai_in(
            &mut r,
            "sk-test".into(),
            XAI_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("xai").is_some());
    }
}
