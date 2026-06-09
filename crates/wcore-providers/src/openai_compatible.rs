//! OpenAI-compatible catch-all provider.
//!
//! Source: Forge `packages/core/src/providers/openai-compatible-provider.ts`
//! (Apache-2.0). This is the generic adapter for any endpoint that speaks the
//! OpenAI `/chat/completions` wire format but is not covered by a named
//! provider — self-hosted llama.cpp, vLLM, LM Studio, third-party gateways,
//! and anything else OpenAI-API-shaped that the user explicitly points us at.
//!
//! Differs from [`OpenAIProvider`] (and from [`DeepSeekProvider`] /
//! [`OpenRouterProvider`]) in one way: the user MUST supply an explicit
//! `base_url`. There is no default — the whole point of this adapter is that
//! we cannot know which endpoint to talk to. Construction with an empty URL
//! is rejected at registration time so the failure surface is loud and early
//! rather than a silent connect-to-localhost-and-time-out.
//!
//! Note on API keys: some self-hosted endpoints do not require auth at all.
//! Forge's reference passes `'no-key'` as a placeholder in that case; we
//! mirror that behavior by accepting whatever string the caller supplies
//! (including a sentinel like `"no-key"`) — the underlying
//! [`OpenAIProvider`] always sets an `Authorization: Bearer …` header, but
//! servers that do not authenticate simply ignore it.
//!
//! Register via [`register_openai_compatible_in`] against a
//! [`ProviderRegistry`]. The id is lowercased to `"openai-compatible"`.
//!
//! [`DeepSeekProvider`]: crate::deepseek::DeepSeekProvider
//! [`OpenRouterProvider`]: crate::openrouter::OpenRouterProvider

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Generic OpenAI-API-compatible provider. The caller MUST supply a base URL;
/// there is no default. Useful for self-hosted, third-party, or unknown
/// providers that speak the OpenAI chat-completions wire format.
pub struct OpenAICompatibleProvider {
    inner: OpenAIProvider,
}

impl OpenAICompatibleProvider {
    /// Construct with an explicit base URL. The base URL is required — there
    /// is no production default for this provider. Use `"no-key"` as the
    /// `api_key` if the target endpoint does not require auth.
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatibleProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

/// Register an OpenAI-compatible factory in the given registry under the
/// lowercased id `"openai-compatible"`. The factory captures the provided
/// `api_key`, `base_url`, `compat`, and `debug` and constructs a fresh
/// provider per call.
///
/// Returns [`RegistryError::EmptyId`] if `base_url` is empty after trimming —
/// the OpenAI-compatible adapter is the catch-all and only useful when
/// pointed at a concrete endpoint, so we surface that misconfiguration loudly
/// rather than letting it become a silent connect-to-empty-host at request
/// time. (We reuse `EmptyId` rather than adding a new variant to keep the
/// surface stable; the message in the resulting error trace is unambiguous
/// in context because this is the only call site that can produce it from a
/// non-empty id.)
pub fn register_openai_compatible_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    if base_url.trim().is_empty() {
        return Err(RegistryError::EmptyId);
    }
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(OpenAICompatibleProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("openai-compatible", factory)
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
    fn constructs_with_explicit_url() {
        // No default URL — the constructor always takes one explicitly.
        let p = OpenAICompatibleProvider::new(
            "no-key",
            "http://localhost:11434/v1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Point at an unroutable port — stream() must surface a ProviderError,
        // not panic, and not hang. Reuses OpenAIProvider's error path.
        let p = OpenAICompatibleProvider::new(
            "no-key",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "local-model".into(),
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
        register_openai_compatible_in(
            &mut r,
            "no-key".into(),
            "http://localhost:11434/v1".into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("openai-compatible").is_some());
        assert!(r.get("OpenAI-Compatible").is_none());
        assert!(r.get("openai_compatible").is_none());
    }

    #[test]
    fn compat_config_passes_through() {
        // ProviderCompat must round-trip through the wrapper.
        let mut r = WaylandProviderRegistry::new();
        register_openai_compatible_in(
            &mut r,
            "no-key".into(),
            "http://localhost:11434/v1".into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("openai-compatible").is_some());
    }

    #[test]
    fn register_rejects_empty_base_url() {
        // Provider-specific: openai-compatible has NO default URL. Registering
        // with an empty (or whitespace-only) base_url must fail loudly at
        // registration time, not silently produce a provider that connects
        // to nothing at request time.
        let mut r = WaylandProviderRegistry::new();

        let empty = register_openai_compatible_in(
            &mut r,
            "no-key".into(),
            String::new(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(empty, Err(RegistryError::EmptyId)));

        let whitespace = register_openai_compatible_in(
            &mut r,
            "no-key".into(),
            "   \t  ".into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(whitespace, Err(RegistryError::EmptyId)));

        // The registry stays clean — no factory registered.
        assert!(r.get("openai-compatible").is_none());
    }
}
