//! Moonshot (Kimi) provider — OpenAI-API-compatible.
//!
//! Moonshot AI hosts the Kimi model family (`moonshot-v1-8k`, `-32k`, `-128k`)
//! behind an OpenAI-compatible chat-completions surface, so this adapter is a
//! thin newtype wrapper over [`OpenAIProvider`]. Provider-specific shaping
//! (max-tokens field, reasoning surfaces, …) belongs in [`OpenAIProvider`] and
//! the [`ProviderCompat`] config — DO NOT add hardcoded provider conditionals
//! here.
//!
//! Register via [`register_moonshot_in`] against a [`ProviderRegistry`]. The
//! id is lowercased to `"moonshot"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Moonshot base URL (OpenAI-compat surface).
pub const MOONSHOT_DEFAULT_BASE_URL: &str = "https://api.moonshot.cn/v1";

/// Moonshot (Kimi) provider — delegates to [`OpenAIProvider`] over Moonshot's
/// OpenAI-compatible endpoint.
pub struct MoonshotProvider {
    inner: OpenAIProvider,
}

impl MoonshotProvider {
    /// Construct with an explicit base URL (use [`MOONSHOT_DEFAULT_BASE_URL`]
    /// for production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with Moonshot's default base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, MOONSHOT_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for MoonshotProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

/// Register a Moonshot factory in the given registry under the lowercased id
/// `"moonshot"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_moonshot_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(MoonshotProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("moonshot", factory)
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
    fn default_base_url_is_moonshot_v1() {
        // Bearer-token auth over the OpenAI-compat surface at api.moonshot.cn/v1.
        assert_eq!(MOONSHOT_DEFAULT_BASE_URL, "https://api.moonshot.cn/v1");
    }

    #[test]
    fn constructs_with_default_url() {
        let p = MoonshotProvider::with_defaults(
            "sk-test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = MoonshotProvider::new(
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
        let p = MoonshotProvider::new(
            "sk-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "moonshot-v1-8k".into(),
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
        register_moonshot_in(
            &mut r,
            "sk-test".into(),
            MOONSHOT_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("moonshot").is_some());
        assert!(r.get("Moonshot").is_none());
        assert!(r.get("MOONSHOT").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_moonshot_in(
            &mut r,
            "sk-test".into(),
            MOONSHOT_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_moonshot_in(
            &mut r,
            "sk-other".into(),
            MOONSHOT_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(second, Err(RegistryError::DuplicateId(_))));
    }

    #[test]
    fn compat_config_passes_through() {
        let mut r = WaylandProviderRegistry::new();
        register_moonshot_in(
            &mut r,
            "sk-test".into(),
            MOONSHOT_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("moonshot").is_some());
    }
}
