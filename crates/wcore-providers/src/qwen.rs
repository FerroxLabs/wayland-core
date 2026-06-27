//! Alibaba Qwen (DashScope) provider — OpenAI-API-compatible.
//!
//! Alibaba Cloud exposes the Qwen model family (`qwen-max`, `qwen-plus`,
//! `qwen-turbo`, `qwen2.5-72b-instruct`, …) via two surfaces:
//!   1. A native DashScope API at `dashscope.aliyuncs.com/api/v1/services/...`
//!      with its own request/response shape.
//!   2. An OpenAI-compatible mode at `/compatible-mode/v1` that speaks standard
//!      OpenAI chat-completions.
//!
//! We use the compat mode — production callers who need DashScope-native shape
//! can opt in later via a separate provider. Provider-specific shaping belongs
//! in [`OpenAIProvider`] and the [`ProviderCompat`] config — DO NOT add
//! hardcoded provider conditionals here.
//!
//! Register via [`register_qwen_in`] against a [`ProviderRegistry`]. The id is
//! lowercased to `"qwen"`.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::OpenAIProvider;
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::{LlmProvider, ProviderError};

/// Default Qwen (DashScope) base URL — OpenAI-compat mode.
///
/// Targets the INTERNATIONAL host (`dashscope-intl.aliyuncs.com`). Mainland-China
/// users reach the `dashscope.aliyuncs.com` endpoint via the `alibaba-cn`
/// catalog alias.
pub const QWEN_DEFAULT_BASE_URL: &str = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1";

/// Qwen provider — delegates to [`OpenAIProvider`] over DashScope's
/// OpenAI-compatible endpoint.
pub struct QwenProvider {
    inner: OpenAIProvider,
}

impl QwenProvider {
    /// Construct with an explicit base URL (use [`QWEN_DEFAULT_BASE_URL`] for
    /// production).
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            inner: OpenAIProvider::new(api_key, base_url, compat, debug),
        }
    }

    /// Construct with DashScope's default OpenAI-compat base URL.
    pub fn with_defaults(api_key: &str, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self::new(api_key, QWEN_DEFAULT_BASE_URL, compat, debug)
    }
}

#[async_trait]
impl LlmProvider for QwenProvider {
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

/// Register a Qwen factory in the given registry under the lowercased id
/// `"qwen"`. The factory captures the provided `api_key`, `base_url`,
/// `compat`, and `debug` and constructs a fresh provider per call.
pub fn register_qwen_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(QwenProvider::new(
            &api_key,
            &base_url,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("qwen", factory)
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
    fn default_base_url_is_dashscope_intl_compat_v1() {
        // Bearer-token auth over the OpenAI-compat surface at the INTERNATIONAL
        // host dashscope-intl.aliyuncs.com/compatible-mode/v1. China users use
        // the alibaba-cn alias.
        assert_eq!(
            QWEN_DEFAULT_BASE_URL,
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
        );
    }

    #[test]
    fn constructs_with_default_url() {
        let p = QwenProvider::with_defaults(
            "sk-test",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let _ = p;
    }

    #[test]
    fn constructs_with_custom_url() {
        let p = QwenProvider::new(
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
        let p = QwenProvider::new(
            "sk-test",
            "http://127.0.0.1:1",
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let req = LlmRequest {
            model: "qwen-plus".into(),
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
        register_qwen_in(
            &mut r,
            "sk-test".into(),
            QWEN_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("qwen").is_some());
        assert!(r.get("Qwen").is_none());
        assert!(r.get("QWEN").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_qwen_in(
            &mut r,
            "sk-test".into(),
            QWEN_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_qwen_in(
            &mut r,
            "sk-other".into(),
            QWEN_DEFAULT_BASE_URL.into(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert!(matches!(second, Err(RegistryError::DuplicateId(_))));
    }

    #[test]
    fn compat_config_passes_through() {
        let mut r = WaylandProviderRegistry::new();
        register_qwen_in(
            &mut r,
            "sk-test".into(),
            QWEN_DEFAULT_BASE_URL.into(),
            compat_with_max_tokens_field("max_completion_tokens"),
            DebugConfig::default(),
        )
        .unwrap();
        assert!(r.get("qwen").is_some());
    }
}
