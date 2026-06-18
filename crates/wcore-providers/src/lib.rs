pub mod anthropic;
pub mod anthropic_shared;
pub mod azure_openai;
pub mod bedrock;
pub mod cache_observation;
// Generic data-driven catalog provider (wraps OpenAIProvider per bundled entry).
pub mod catalog;
pub mod cerebras;
pub mod chain;
pub mod classify;
pub mod cohere;
pub mod cooldown;
pub mod deepseek;
pub mod failover;
pub mod fingerprint;
pub mod fireworks;
pub mod flux_router;
pub mod gemini;
pub mod groq;
pub mod http_client;
pub mod key_rotation;
pub mod key_validation;
// litellm, lmstudio, vllm: deleted per DECISIONS.md §D3 (Sean, 2026-05-23).
// These were framework-shaped local-inference adapters shipped as code but
// never wired to ProviderType arms. Revivable as plugins if needed.
pub mod mistral;
pub mod model_catalog;
pub mod moonshot;
pub mod nvidia;
pub mod openai;
pub mod openai_chatgpt;
pub mod openai_compat;
pub mod openai_compatible;
pub mod openai_responses;
pub mod openrouter;
pub mod paste_detect;
pub mod perplexity;
pub mod qwen;
pub mod registry;
pub mod resilient;
pub mod retry;
pub mod routing;
pub mod together;
pub mod vertex;
pub mod xai;

pub use cache_observation::{CacheRetention, InvalidationCause, PromptCacheObservation};
pub use catalog::{CatalogProviderConfig, provider_for_entry, register_catalog};
// W8 v0.6.3: CacheTier moved to wcore-types to break the wcore-providers ↔
// wcore-types cycle that blocked adding `cache_tier: Option<CacheTier>` to
// `LlmRequest`. Re-exported here for backward compatibility.
pub use chain::{ProviderChain, ProviderSlot};
pub use classify::classify_failover;
pub use cooldown::{CooldownClass, CooldownState, CooldownTracker};
pub use failover::{FailoverError, FailoverReason, wrap_provider_error};
pub use key_rotation::{KeyPool, split_keys};
pub use openai::{AsyncTokenSource, OpenAIProvider};
pub use openai_chatgpt::{AsyncBearerSource, BearerCreds, OpenAIChatGptProvider};
pub use registry::{ProviderFactory, ProviderRegistry, RegistryError, WaylandProviderRegistry};
pub use resilient::{
    CircuitBreaker, CircuitConfig, CircuitReporter, CircuitState, NoOpCircuitReporter,
    ResilientProvider,
};
pub use routing::{
    RequestShape, RoutingDecision, RoutingHeuristics, RoutingTier, route, select_tier,
};
pub use wcore_types::cache_tier;
pub use wcore_types::cache_tier::{CacheTier, CacheTierConfig, pick_cache_tier, pick_with_config};

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_config::config::{AzureAuthMode, Config, ProviderType};
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

/// A provider-neutral model descriptor for the `/model` picker.
///
/// `id` is the literal string a request carries in `LlmRequest::model`;
/// `display` is the human-facing label. When a provider has no richer
/// display name (e.g. OpenAI's `/v1/models` returns only ids) `display`
/// mirrors `id`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display: String,
}

impl ModelInfo {
    /// Build a descriptor whose display label mirrors its id.
    pub fn from_id(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            display: id.clone(),
            id,
        }
    }
}

/// Map a provider's static alias catalog (`models_for_provider`) into the
/// provider-neutral [`ModelInfo`] shape. This is the fallback every provider
/// returns from [`LlmProvider::list_models`] when a live fetch is unavailable
/// or fails. `provider` is the alias key (e.g. `"anthropic"`, `"openai"`);
/// an unknown key yields an empty list (the user can still type a literal id).
pub fn alias_models(provider: &str) -> Vec<ModelInfo> {
    wcore_types::model_aliases::models_for_provider(provider)
        .iter()
        .map(|(short, resolved)| ModelInfo {
            id: (*resolved).to_string(),
            display: (*short).to_string(),
        })
        .collect()
}

/// Unified interface for LLM API providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(&self, request: &LlmRequest)
    -> Result<mpsc::Receiver<LlmEvent>, ProviderError>;

    /// The alias-catalog key for this provider (e.g. `"anthropic"`).
    ///
    /// Drives the default [`list_models`](LlmProvider::list_models) fallback:
    /// the default impl maps `models_for_provider(key)` to [`ModelInfo`]. The
    /// blanket default returns `""` (no alias catalog → empty fallback); a
    /// provider with a static catalog overrides this to its key.
    fn alias_key(&self) -> &str {
        ""
    }

    /// List the models this provider offers, most-capable first.
    ///
    /// The default implementation returns the static alias catalog for
    /// [`alias_key`](LlmProvider::alias_key) — every provider therefore has a
    /// working fallback and the trait stays object-safe. Providers that can
    /// query a live models endpoint (Anthropic, OpenAI-compatible) override
    /// this; on any HTTP/parse failure they MUST fall back to the alias list
    /// rather than erroring so the `/model` picker never hard-fails.
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(alias_models(self.alias_key()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("egress error: {0}")]
    Egress(#[from] wcore_egress::EgressError),
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("SSE parse error: {0}")]
    Parse(String),
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("Prompt too long: {0}")]
    PromptTooLong(String),
    #[error("Connection error: {0}")]
    Connection(String),
    #[error(
        "No API key. Set an api_key via --api-key, the config file, or an API-key environment variable."
    )]
    MissingApiKey,
}

impl ProviderError {
    /// True for errors a retry (same request, possibly after backoff) may
    /// resolve.
    ///
    /// Retryable: `RateLimited`, `Connection`, and transient HTTP `Api`
    /// errors — 5xx server errors plus 408 (request timeout) and 429.
    /// E-H4: a transient 503 from an overloaded provider is `Api{status:503}`
    /// and MUST be retried; before this fix only `RateLimited`/`Connection`
    /// were, so a 5xx aborted the turn immediately.
    ///
    /// Not retryable: 4xx client errors other than 408/429 (auth, validation,
    /// malformed request), `Parse` (structural provider bug), `PromptTooLong`
    /// (won't shrink on retry), and `Http` (covers redirect/decode failures —
    /// transient reqwest connect/timeout errors are mapped to `Connection`
    /// before they reach here).
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::RateLimited { .. } | ProviderError::Connection(_) => true,
            ProviderError::Api { status, .. } => crate::retry::is_retryable_http_status(*status),
            ProviderError::Http(_)
            | ProviderError::Parse(_)
            | ProviderError::PromptTooLong(_)
            // Missing credential is terminal — no retry will conjure a token.
            | ProviderError::MissingApiKey => false,
            // Egress transport timeouts/connects are pre-mapped to Connection
            // by provider code (like Http); a Denied is terminal. Both false here.
            ProviderError::Egress(_) => false,
        }
    }
}

/// Write the request body to the configured dump path (if set).
///
/// This is a shared helper called by each provider's `stream()` method.
/// Errors are silently ignored — debug output must never break requests.
pub fn dump_request_body(debug: &DebugConfig, body: &serde_json::Value) {
    if let Some(path) = &debug.dump_request_path {
        let pretty = serde_json::to_string_pretty(body).unwrap_or_default();
        let _ = std::fs::write(path, &pretty); // ephemeral debug dump; best-effort, not durable
    }
}

/// Truncate the response dump file at the start of a new request.
pub fn reset_response_dump(debug: &DebugConfig) {
    if let Some(path) = &debug.dump_response_path {
        let _ = std::fs::write(path, ""); // ephemeral debug dump; best-effort, not durable
    }
}

/// Append a raw SSE line to the response dump file.
pub fn dump_response_chunk(debug: &DebugConfig, chunk: &str) {
    if let Some(path) = &debug.dump_response_path {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{chunk}");
        }
    }
}

/// Create a provider from resolved config.
///
/// E-H2: the returned provider is **always** wrapped in a
/// [`ResilientProvider`] — circuit-breaking is on by default, not behind an
/// opt-in flag. After `fail_threshold` consecutive provider-side failures
/// the breaker opens and `stream()` fails fast for the cooldown window
/// instead of hammering a wedged or rate-limited endpoint. The wrap is
/// `LlmProvider`-transparent, so every caller (`AgentEngine::new`,
/// `bootstrap`, sub-agents) gets resilience for free.
///
/// The wrap carries no fallback chain — a single configured provider has no
/// alternate to fail over to — but circuit-breaking + fail-fast is the
/// load-bearing half of resilience and is now live for every request.
pub fn create_provider(config: &Config) -> Arc<dyn LlmProvider> {
    let inner = create_native_provider(config);
    let cfg = resilient::CircuitConfig {
        fail_threshold: config.provider_chain.failure_threshold as usize,
        window: std::time::Duration::from_secs(config.provider_chain.recovery_timeout_secs),
        cooldown: std::time::Duration::from_secs(config.provider_chain.recovery_timeout_secs),
    };
    Arc::new(resilient::ResilientProvider::new(
        config.provider_label.clone(),
        inner,
        Vec::new(),
        cfg,
        Arc::new(resilient::NoOpCircuitReporter),
    ))
}

/// Build the bare native provider for `config.provider` with no resilience
/// wrapping. `create_provider` wraps the result; `bootstrap` uses this
/// directly when it needs to apply its own protocol-aware circuit reporter.
pub fn create_native_provider(config: &Config) -> Arc<dyn LlmProvider> {
    let compat = config.compat.clone();
    let debug = config.debug.clone();

    match config.provider {
        ProviderType::Anthropic => Arc::new(
            anthropic::AnthropicProvider::new(&config.api_key, &config.base_url, compat, debug)
                .with_cache(config.prompt_caching),
        ),
        ProviderType::OpenAI => Arc::new(openai::OpenAIProvider::new(
            &config.api_key,
            &config.base_url,
            compat,
            debug,
        )),
        ProviderType::Bedrock => {
            let bc = config.bedrock.clone().unwrap_or_default();
            let region = bc
                .region
                .clone()
                .or_else(|| std::env::var("AWS_REGION").ok())
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| "us-east-1".to_string());
            let credentials = bedrock::credentials_from_config(&bc);
            Arc::new(bedrock::BedrockProvider::new(
                &region,
                credentials,
                config.prompt_caching,
                compat,
                debug,
            ))
        }
        ProviderType::Vertex => {
            let vc = config.vertex.clone().unwrap_or_default();
            let project_id = vc.project_id.clone().unwrap_or_default();
            let region = vc
                .region
                .clone()
                .unwrap_or_else(|| "us-central1".to_string());
            let auth = vertex::auth_from_config(&vc);
            Arc::new(vertex::VertexProvider::new(
                &project_id,
                &region,
                auth,
                config.prompt_caching,
                compat,
                debug,
            ))
        }
        // W11: native Gemini via the Generative Language API.
        // Distinct from `Vertex` — no GCP OAuth, just an API key.
        ProviderType::Gemini => Arc::new(gemini::GeminiProvider::new(
            &config.api_key,
            &config.base_url,
            compat,
            debug,
        )),
        // v0.6.3 Tier-2 OpenAI-compatible providers (D.1 Round 1 cleanup).
        // Each is a thin newtype over `OpenAIProvider`. When `base_url` is
        // empty the provider falls back to its own `*_DEFAULT_BASE_URL`;
        // a non-empty `base_url` (CLI/config override) is honored.
        ProviderType::Together => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => together::TogetherProvider::new(&config.api_key, url, compat, debug),
            None => together::TogetherProvider::with_defaults(&config.api_key, compat, debug),
        })),
        ProviderType::Fireworks => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => fireworks::FireworksProvider::new(&config.api_key, url, compat, debug),
            None => fireworks::FireworksProvider::with_defaults(&config.api_key, compat, debug),
        })),
        ProviderType::Nvidia => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => nvidia::NvidiaProvider::new(&config.api_key, url, compat, debug),
            None => nvidia::NvidiaProvider::with_defaults(&config.api_key, compat, debug),
        })),
        ProviderType::Perplexity => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => perplexity::PerplexityProvider::new(&config.api_key, url, compat, debug),
            None => perplexity::PerplexityProvider::with_defaults(&config.api_key, compat, debug),
        })),
        ProviderType::Cerebras => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => cerebras::CerebrasProvider::new(&config.api_key, url, compat, debug),
            None => cerebras::CerebrasProvider::with_defaults(&config.api_key, compat, debug),
        })),
        // v0.8.1 U10a — OpenRouter (existing aggregator). Model ids must use
        // the `vendor/model` format (e.g. `anthropic/claude-opus-4-7`).
        ProviderType::OpenRouter => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => openrouter::OpenRouterProvider::new(&config.api_key, url, compat, debug),
            None => openrouter::OpenRouterProvider::with_defaults(&config.api_key, compat, debug),
        })),
        // v0.8.1 U10a — Flux Router (Sean's product). Default base URL is a
        // placeholder until production endpoint is finalized; override via
        // `base_url` in config or `--base-url`.
        ProviderType::FluxRouter => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => flux_router::FluxRouterProvider::new(&config.api_key, url, compat, debug),
            None => flux_router::FluxRouterProvider::with_defaults(&config.api_key, compat, debug),
        })),
        // v0.8.1 U10b: 3 more OpenAI-compatible providers — DeepSeek, xAI, Groq.
        ProviderType::Deepseek => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => deepseek::DeepSeekProvider::new(&config.api_key, url, compat, debug),
            None => deepseek::DeepSeekProvider::with_defaults(&config.api_key, compat, debug),
        })),
        ProviderType::Xai => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => xai::XaiProvider::new(&config.api_key, url, compat, debug),
            None => xai::XaiProvider::with_defaults(&config.api_key, compat, debug),
        })),
        ProviderType::Groq => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => groq::GroqProvider::new(&config.api_key, url, compat, debug),
            None => groq::GroqProvider::with_defaults(&config.api_key, compat, debug),
        })),
        // v0.8.1 U10e: native OpenAI-compat wrappers for the two major
        // Chinese model APIs. Both speak standard OpenAI chat-completions.
        ProviderType::Moonshot => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => moonshot::MoonshotProvider::new(&config.api_key, url, compat, debug),
            None => moonshot::MoonshotProvider::with_defaults(&config.api_key, compat, debug),
        })),
        ProviderType::Qwen => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => qwen::QwenProvider::new(&config.api_key, url, compat, debug),
            None => qwen::QwenProvider::with_defaults(&config.api_key, compat, debug),
        })),
        // F-025: Mistral + Cohere wired from orphan modules to reachable match arms.
        // These were declared in lib.rs and had full implementations but were
        // unreachable because ProviderType had no enum arms for them.
        ProviderType::Mistral => Arc::new(new_openai_compat(&config.base_url, |b| match b {
            Some(url) => mistral::MistralProvider::new(&config.api_key, url, compat, debug),
            None => mistral::MistralProvider::with_defaults(&config.api_key, compat, debug),
        })),
        // Cohere has its own native API (not OpenAI-compat). It ignores `compat`
        // and `base_url` overrides are honored when non-empty.
        ProviderType::Cohere => {
            const DEFAULT_COHERE_MODEL: &str = "command-r-plus-08-2024";
            let base_url = config.base_url.trim();
            Arc::new(if base_url.is_empty() {
                cohere::CohereProvider::with_defaults(&config.api_key, DEFAULT_COHERE_MODEL, debug)
            } else {
                cohere::CohereProvider::new(&config.api_key, base_url, DEFAULT_COHERE_MODEL, debug)
            })
        }
        // Azure OpenAI routes by deployment, not base path: `base_url`
        // carries the resource endpoint (`https://{resource}.openai.azure.com`)
        // and `model` is the deployment name. We extract the resource
        // subdomain from `base_url`; an unparseable/empty value yields an
        // empty resource, which Azure's `stream()` surfaces as a loud
        // connect error (honest failure over a wrong-host request).
        ProviderType::AzureOpenAI => {
            let resource = azure_resource_from_base_url(&config.base_url);
            // R77: honor the configured Azure auth mode. `aad-bearer` swaps the
            // `api-key` header for an Entra-ID bearer token; the token itself
            // comes from AZURE_AD_TOKEN (the crate owns no acquisition/refresh —
            // the caller supplies the closure). Defaults to api-key auth.
            // `azure_auth_mode` is Copy, so this read does not move `compat`.
            match compat.azure_auth_mode.unwrap_or_default() {
                AzureAuthMode::AadBearer => {
                    let token_source: azure_openai::AzureTokenSource = Arc::new(|| {
                        std::env::var("AZURE_AD_TOKEN").map_err(|_| ProviderError::MissingApiKey)
                    });
                    Arc::new(azure_openai::AzureOpenAIProvider::with_auth(
                        azure_openai::AzureAuth::AadBearer(token_source),
                        &resource,
                        &config.model, // Azure deployment name == configured model
                        azure_openai::AZURE_OPENAI_DEFAULT_API_VERSION,
                        compat,
                        debug,
                    ))
                }
                AzureAuthMode::ApiKey => {
                    Arc::new(azure_openai::AzureOpenAIProvider::with_defaults(
                        &config.api_key,
                        &resource,
                        &config.model, // Azure deployment name == configured model
                        compat,
                        debug,
                    ))
                }
            }
        }
        // "Sign in with ChatGPT" cannot be built here: it needs an OAuth-backed
        // async bearer source whose token store (`OAuthStorage`) lives in
        // `wcore-agent`, which `wcore-providers` must NOT depend on (layering).
        // `wcore-agent::bootstrap` special-cases this variant BEFORE calling
        // `create_native_provider` and constructs `OpenAIChatGptProvider`
        // directly with a bearer closure over `ChatGptTokenManager` (Phase 5).
        // Reaching this arm means that special-case was bypassed — a bug.
        ProviderType::OpenAIChatGpt => {
            panic!(
                "OpenAIChatGpt is constructed in bootstrap (with an OAuth bearer \
                 source), not create_native_provider — this dispatch should never \
                 be reached for the chatgpt provider"
            )
        }
        // MiniMax's `/anthropic` endpoint speaks the native Anthropic wire
        // protocol, so it reuses `AnthropicProvider` verbatim — only the base
        // URL (from `default_base_url_for`, overridable), the `provider_type`
        // cost label (via `minimax_defaults`), and the offline model-fallback
        // key differ. Caching is off: MiniMax's support for the Anthropic
        // prompt-caching beta header is unverified, so we don't send it.
        ProviderType::MiniMax => Arc::new(
            anthropic::AnthropicProvider::new(&config.api_key, &config.base_url, compat, debug)
                .with_cache(false)
                .with_alias_key("minimax"),
        ),
    }
}

/// Helper: build an OpenAI-compatible Tier-2 provider, passing the configured
/// `base_url` as `Some` when non-empty (a CLI/config override) and `None`
/// otherwise (let the provider use its own default URL).
fn new_openai_compat<P>(base_url: &str, build: impl FnOnce(Option<&str>) -> P) -> P {
    let trimmed = base_url.trim();
    build(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    })
}

/// Extract the Azure resource name (the `{resource}` in
/// `{resource}.openai.azure.com`) from a configured `base_url`.
///
/// Accepts a full endpoint URL (`https://my-res.openai.azure.com`) or a bare
/// resource name (`my-res`). Returns an empty string when `base_url` is empty
/// or unparseable — the caller passes that through to the Azure provider,
/// which then fails loudly at request time rather than silently hitting a
/// wrong host.
fn azure_resource_from_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Full URL form: pull the host and strip the `.openai.azure.com` suffix.
    if let Ok(parsed) = url::Url::parse(trimmed)
        && let Some(host) = parsed.host_str()
    {
        return host
            .strip_suffix(".openai.azure.com")
            .unwrap_or(host)
            .to_string();
    }
    // Bare-host form (no scheme): strip a trailing azure suffix if present.
    trimmed
        .strip_suffix(".openai.azure.com")
        .unwrap_or(trimmed)
        .to_string()
}

#[cfg(test)]
mod list_models_default_tests {
    use super::*;

    /// A provider that only implements `stream` (the blanket default
    /// `alias_key` => "") yields an EMPTY fallback list — no alias catalog.
    struct BareProvider;

    #[async_trait]
    impl LlmProvider for BareProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            unreachable!("not exercised by the list_models default test")
        }
    }

    /// A provider that overrides `alias_key` => "anthropic" must, with NO
    /// `list_models` override, fall back to the static alias catalog mapped
    /// into `ModelInfo`.
    struct AliasOnlyProvider;

    #[async_trait]
    impl LlmProvider for AliasOnlyProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            unreachable!("not exercised by the list_models default test")
        }
        fn alias_key(&self) -> &str {
            "anthropic"
        }
    }

    #[tokio::test]
    async fn default_list_models_empty_for_no_alias_key() {
        let got = BareProvider
            .list_models()
            .await
            .expect("default never errs");
        assert!(
            got.is_empty(),
            "a provider with no alias key yields no fallback models"
        );
    }

    #[tokio::test]
    async fn default_list_models_maps_alias_catalog() {
        let got = AliasOnlyProvider
            .list_models()
            .await
            .expect("default never errs");
        let aliases = wcore_types::model_aliases::models_for_provider("anthropic");
        assert_eq!(
            got.len(),
            aliases.len(),
            "the default must map every alias entry"
        );
        // The mapping: id = resolved canonical string, display = short form.
        assert_eq!(got[0].id, aliases[0].1, "id is the resolved canonical id");
        assert_eq!(got[0].display, aliases[0].0, "display is the short form");
    }

    #[test]
    fn model_info_from_id_mirrors_display() {
        let m = ModelInfo::from_id("claude-opus-4-6");
        assert_eq!(m.id, "claude-opus-4-6");
        assert_eq!(m.display, "claude-opus-4-6");
    }
}

#[cfg(test)]
mod create_provider_tests {
    use super::*;

    // --- HIGH-5 v0.6.3: provider-registry gap — azure resource parsing -----

    #[test]
    fn azure_resource_parsed_from_full_endpoint_url() {
        assert_eq!(
            azure_resource_from_base_url("https://my-res.openai.azure.com"),
            "my-res"
        );
        assert_eq!(
            azure_resource_from_base_url("https://my-res.openai.azure.com/"),
            "my-res"
        );
    }

    #[test]
    fn azure_resource_parsed_from_bare_host() {
        assert_eq!(
            azure_resource_from_base_url("my-res.openai.azure.com"),
            "my-res"
        );
    }

    #[test]
    fn azure_resource_parsed_from_bare_name() {
        // A bare resource name (no suffix, no scheme) passes through.
        assert_eq!(azure_resource_from_base_url("my-res"), "my-res");
    }

    #[test]
    fn azure_resource_empty_for_empty_base_url() {
        assert_eq!(azure_resource_from_base_url(""), "");
        assert_eq!(azure_resource_from_base_url("   "), "");
    }

    #[test]
    fn new_openai_compat_passes_some_for_non_empty_base_url() {
        let got = new_openai_compat("https://x.example/v1", |b| b.map(str::to_string));
        assert_eq!(got.as_deref(), Some("https://x.example/v1"));
    }

    #[test]
    fn new_openai_compat_passes_none_for_empty_base_url() {
        let got = new_openai_compat("", |b| b.map(str::to_string));
        assert_eq!(got, None);
        let got = new_openai_compat("  ", |b| b.map(str::to_string));
        assert_eq!(got, None);
    }
}
