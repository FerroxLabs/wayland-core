//! Together AI provider — OpenAI-API-compatible.
//!
//! Together AI hosts open-weight models (Llama, Mixtral, Qwen, and more)
//! behind an OpenAI-compatible chat-completions surface, so this adapter is a
//! thin newtype wrapper over [`OpenAIProvider`]. Provider-specific shaping
//! belongs in [`OpenAIProvider`] and the [`ProviderCompat`] config — DO NOT
//! add hardcoded provider conditionals here.
//!
//! Register via [`register_together_in`] against a [`ProviderRegistry`]. The
//! id is lowercased to `"together"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Together AI base URL (OpenAI-compat surface).
pub const TOGETHER_DEFAULT_BASE_URL: &str = "https://api.together.xyz/v1";

/// Together AI provider — delegates to [`OpenAIProvider`] over Together's
/// OpenAI-compatible endpoint.
pub struct TogetherProvider {
    inner: OpenAIProvider,
}

impl TogetherProvider {
    /// Construct with an explicit base URL (use [`TOGETHER_DEFAULT_BASE_URL`]
    /// for production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with Together AI's default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, TOGETHER_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for TogetherProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

/// Register a Together AI factory in the given registry under the lowercased
/// id `"together"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_together_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(TogetherProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("together", factory)
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
        let p = TogetherProvider::with_defaults(
            "tok_test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = TogetherProvider::new(
            "tok_test",
            "http://localhost:9999",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn default_base_url_is_together_v1() {
        assert_eq!(TOGETHER_DEFAULT_BASE_URL, "https://api.together.xyz/v1");
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path.
        let p = TogetherProvider::new(
            "tok_test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "meta-llama/Llama-3.3-70B-Instruct-Turbo".into(),
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
        register_together_in(
            &mut r,
            "tok_test".into(),
            TOGETHER_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("together").is_some());
        assert!(r.get("Together").is_none());
        assert!(r.get("TOGETHER").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_together_in(
            &mut r,
            "tok_test".into(),
            TOGETHER_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_together_in(
            &mut r,
            "tok_other".into(),
            TOGETHER_DEFAULT_BASE_URL.into(),
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
        register_together_in(
            &mut r,
            "tok_test".into(),
            TOGETHER_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("together").is_some());
    }
}
