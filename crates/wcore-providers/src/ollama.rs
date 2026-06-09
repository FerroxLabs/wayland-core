//! Ollama provider — IN-CORE native adapter for local Ollama servers.
//!
//! Source: Forge `packages/core/src/providers/ollama-provider.ts` (Apache-2.0).
//!
//! NOTE: This is a SEPARATE, in-core adapter that lives parallel to the
//! `wayland-ollama` plugin crate at `crates/wayland-ollama/`. The plugin
//! exposes Ollama via the plugin system; this in-core adapter exposes it via
//! the [`ProviderRegistry`] directly without going through the plugin loader.
//! Both can coexist — pick whichever your bootstrap wiring prefers.
//!
//! Ollama exposes an OpenAI-compatible chat-completions surface at
//! `<baseUrl>/v1`, so this adapter is a thin newtype wrapper over
//! [`OpenAIProvider`] — matching Forge's choice to delegate to the OpenAI
//! provider rather than implement the native `/api/chat` surface. The
//! API-key field is a sentinel value (`"ollama"`); the server does not
//! validate it.
//!
//! Register via [`register_ollama_in`] against a [`ProviderRegistry`]. The
//! id is lowercased to `"ollama"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Ollama base URL — the local server's OpenAI-compat surface.
pub const OLLAMA_DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";

/// Sentinel API key used by the Ollama OpenAI-compat surface. The local
/// server does not validate this value but the OpenAI client requires a
/// non-empty key in the `Authorization` header.
pub const OLLAMA_API_KEY_SENTINEL: &str = "ollama";

/// Ollama provider — delegates to [`OpenAIProvider`] over Ollama's
/// `/v1` OpenAI-compat endpoint.
pub struct OllamaProvider {
    inner: OpenAIProvider,
}

impl OllamaProvider {
    /// Construct with an explicit base URL (use [`OLLAMA_DEFAULT_BASE_URL`]
    /// for the standard local server).
    pub fn new(base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(OLLAMA_API_KEY_SENTINEL, base_url, compat, debug),
        }
    }

    /// Construct with Ollama's default local base URL.
    pub fn with_defaults(compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(OLLAMA_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

/// Register an Ollama factory in the given registry under the lowercased id
/// `"ollama"`. The factory captures the provided `base_url`, `compat`, and
/// `debug` and constructs a fresh provider per call. No API key argument —
/// Ollama uses a sentinel value internally.
pub fn register_ollama_in<R: ProviderRegistry>(
    registry: &mut R,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(OllamaProvider::new(
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("ollama", factory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::WaylandProviderRegistry;

    #[test]
    fn constructs_with_default_url() {
        let p = OllamaProvider::with_defaults(ProviderCompat::default(), DebugConfig::default());
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = OllamaProvider::new(
            "http://localhost:9999/v1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path.
        let p = OllamaProvider::new(
            "http://127.0.0.1:1/v1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "llama3.1".into(),
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
        register_ollama_in(
            &mut r,
            OLLAMA_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("ollama").is_some());
        assert!(r.get("Ollama").is_none());
        assert!(r.get("OLLAMA").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_ollama_in(
            &mut r,
            OLLAMA_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_ollama_in(
            &mut r,
            OLLAMA_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(second, Err(RegistryError::DuplicateId(_))));
    }
}
