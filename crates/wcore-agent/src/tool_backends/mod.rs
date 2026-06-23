//! v0.6.3 D.0 — real HTTP backends for the API-seam catalog tools.
//!
//! The `wcore-tools` crate ships **no HTTP client** by design: GitHub /
//! GitLab / Linear / Notion tools build a fully-described request
//! (`*Request`) and hand it to a host-supplied `*Backend` trait object.
//! Without a real backend the tools register with their `Null*Backend`,
//! which fails loud — schema-visible but inert.
//!
//! This module supplies the real backends. Each performs the resolved
//! request over a `reqwest::Client` built via the local
//! [`build_ssrf_safe_tool_client`] — same non-streaming HTTP policy as
//! [`wcore_providers::http_client::build_tool_client`] (connect + read
//! timeouts PLUS a request-level wall-clock cap, AUDIT B-5) plus the
//! SSRF-resistant redirect policy from
//! [`wcore_tools::url_safety::ssrf_safe_redirect_policy`] (#279 / F-019)
//! that re-validates each redirect hop with `is_safe_url`. The backend
//! maps the HTTP response into the tool's `*Outcome` enum.
//!
//! Auth is *not* this module's concern: the tools already resolve tokens
//! (from the tool input or the relevant env var) and embed them in the
//! request's `headers` (`Authorization` / `PRIVATE-TOKEN`). A backend
//! just replays what it is handed. When no credential resolved, the
//! upstream service returns `401`/`403` and the backend surfaces it as a
//! clean `HttpError` — an honest runtime error, never a silent stub.
//!
//! v0.9.0 Wave-1 B0 (2026-05-27): split the monolith file into one file
//! per backend so parallel sub-agents adding new backends do not collide
//! on shared lines (R-B1 structural fix).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use wcore_egress::EgressClient as Client;
use wcore_tools::url_safety::{SsrfSafeResolver, ssrf_safe_redirect_policy};

// Trait imports for the four API-seam catalog backends — the
// `ApiToolBackends` struct holds `Arc<dyn _Backend>` for each.
use wcore_tools::github_tool::GitHubBackend;
use wcore_tools::gitlab_tool::GitLabBackend;
use wcore_tools::linear_tool::LinearBackend;
use wcore_tools::notion_tool::NotionBackend;
use wcore_tools::transcription_tools::{AudioFetcher, TranscriptionBackend};
use wcore_tools::vision_tools::{ImageFetcher, VisionBackend};
use wcore_tools::web_fetch::FetchBackend;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

// -- Sub-modules: one file per backend (v0.9.0 W1 B0 split). --
pub mod anthropic_vision;
pub mod brave_web;
pub mod chained_web;
pub mod duckduckgo_web;
pub mod exa_web;
pub mod firecrawl_web;
pub mod gemini_vision;
pub mod http_fetch;
pub mod http_github;
pub mod http_gitlab;
pub mod http_linear;
pub mod http_notion;
pub mod openai_compat_whisper;
pub mod openai_vision;
pub mod parallel_web;
pub mod searxng_web;
pub mod shared;
pub mod tavily_web;

// -- v0.9.0 W1 sub-agent B-tasks (B1-B5/B7-B12): one file per new backend. --
pub mod cron;
pub mod discord;
pub mod google_meet;
pub mod homeassistant;
pub mod image_gen;
pub mod introspection;
pub mod piper;
pub mod postgres_schema;
pub mod tts;
pub mod video_analyze;
// v0.9.0 W1 B10 — cpal-backed audio recorder + OS-shell player.
// Issue #14 — gated behind the off-by-default `voice` feature so the default
// binary does not pull cpal → libasound.so.2 (ALSA) on Linux.
#[cfg(feature = "voice")]
pub mod voice_mode;

// -- Re-exports so existing consumers keep using `wcore_agent::tool_backends::X`. --
pub use anthropic_vision::AnthropicVisionBackend;
pub use brave_web::BraveWebBackend;
pub use chained_web::ChainedWebBackend;
pub use duckduckgo_web::DuckDuckGoWebBackend;
pub use exa_web::ExaWebBackend;
pub use firecrawl_web::FirecrawlWebBackend;
pub use gemini_vision::GeminiVisionBackend;
pub use http_fetch::HttpFetchBackend;
pub use http_github::HttpGitHubBackend;
pub use http_gitlab::HttpGitLabBackend;
pub use http_linear::HttpLinearBackend;
pub use http_notion::HttpNotionBackend;
pub use openai_compat_whisper::OpenAiCompatWhisperBackend;
pub use openai_vision::OpenAiVisionBackend;
pub use parallel_web::ParallelWebBackend;
pub use searxng_web::SearxngWebBackend;
pub use shared::read_env_key;
pub use tavily_web::TavilyWebBackend;

/// Parse an HTTP response body as JSON, falling back to wrapping the raw
/// text under a `"raw"` key when the body is not valid JSON (some APIs
/// return empty `204` bodies or plain text on error).
pub(crate) fn parse_json_or_raw(text: &str) -> Value {
    if text.trim().is_empty() {
        return Value::Null;
    }
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// Extract a human-readable message from a parsed error payload — most
/// of these APIs put it under a top-level `"message"` field.
pub(crate) fn error_message(payload: &Value, fallback: &str) -> String {
    payload
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

/// Build a `reqwest::Client` for tool backends with the
/// SSRF-resistant redirect policy from
/// [`wcore_tools::url_safety::ssrf_safe_redirect_policy`].
///
/// Same connect/read/request timeouts as
/// [`wcore_providers::http_client::build_tool_client`] (AUDIT B-5) —
/// the only difference is the custom redirect policy, which re-validates
/// every redirect target via `is_safe_url` so an attacker-controlled
/// `302` to `169.254.169.254` / `10.x.x.x` / `127.0.0.1` / `[fd00::]`
/// is refused mid-chain instead of being silently followed.
///
/// F-019 (WebFetch) + #279 (github_api / linear / notion / gitlab) both
/// route through this single helper so the redirect policy is one edit,
/// not five.
pub(crate) fn build_ssrf_safe_tool_client() -> Client {
    Client::builder()
        .connect_timeout(wcore_providers::http_client::CONNECT_TIMEOUT)
        .read_timeout(wcore_providers::http_client::READ_TIMEOUT)
        .timeout(wcore_providers::http_client::TOOL_REQUEST_TIMEOUT)
        .redirect(ssrf_safe_redirect_policy())
        // H-1-broad: the redirect policy re-checks each hop's URL but reqwest
        // re-resolves the host at connect time, so a TTL=0 rebind could still
        // land on the metadata IP. `SsrfSafeResolver` makes reqwest dial only
        // validated public IPs (initial request AND every redirect hop), with
        // no separate check→connect resolution — closing the rebind for this
        // long-lived, multi-host client (WebFetch + the API backends).
        .dns_resolver(Arc::new(SsrfSafeResolver))
        .build()
        .expect("reqwest TLS backend must initialize at startup")
}

// ---------------------------------------------------------------------
// Convenience constructors — used by `bootstrap.rs`.
// ---------------------------------------------------------------------

/// Build all four real API-tool backends as trait objects, ready to wire
/// into the tool registry. Each shares the non-streaming HTTP timeout
/// policy (AUDIT B-5 — connect + read + request-level cap) PLUS the
/// SSRF-resistant redirect policy (#279 / F-019) — see
/// [`build_ssrf_safe_tool_client`] — but holds its own `reqwest::Client`.
pub fn build_api_tool_backends() -> ApiToolBackends {
    ApiToolBackends {
        github: Arc::new(HttpGitHubBackend::new()),
        gitlab: Arc::new(HttpGitLabBackend::new()),
        linear: Arc::new(HttpLinearBackend::new()),
        notion: Arc::new(HttpNotionBackend::new()),
    }
}

/// Build the real `WebFetch` backend. Mirrors `build_api_tool_backends`.
pub fn build_fetch_backend() -> Arc<dyn FetchBackend> {
    Arc::new(HttpFetchBackend::new())
}

/// Explicit web-backend selection via `WAYLAND_WEB_BACKEND`.
///
/// This is an EXPLICIT override layered ON TOP of the key-presence priority
/// ladder below — distinct from the vision/transcription builders, which are
/// key-presence-only. `auto` (the default / unset / unrecognized) runs the
/// full ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebBackendChoice {
    Auto,
    Parallel,
    DuckDuckGo,
    Off,
}

fn resolve_backend_choice(raw: Option<&str>) -> WebBackendChoice {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("off") | Some("none") | Some("disabled") => WebBackendChoice::Off,
        Some("duckduckgo") | Some("ddg") => WebBackendChoice::DuckDuckGo,
        Some("parallel") => WebBackendChoice::Parallel,
        _ => WebBackendChoice::Auto,
    }
}

/// One-time privacy disclosure for the anonymous Parallel default — emitted
/// the first time the keyless/`parallel` path is selected, not on every search.
fn disclose_parallel_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::info!(
            "web search: using Parallel.ai free search (anonymous). Your search queries are sent \
             to parallel.ai. Set WAYLAND_WEB_BACKEND=duckduckgo to keep queries on DuckDuckGo, \
             =off to disable, or set FIRECRAWL_API_KEY / TAVILY_API_KEY / EXA_API_KEY / \
             SEARXNG_URL / BRAVE_SEARCH_API_KEY for a configured provider."
        );
    });
}

/// Pick the active `WebBackend`. Explicit `WAYLAND_WEB_BACKEND` wins; otherwise
/// the first configured key (the provider preference order) is used. Every selected
/// primary is wrapped so it falls back to DuckDuckGo on failure — DDG is the
/// floor for all tiers except an explicit `off`.
///
/// Resolution order (first match wins):
/// * `WAYLAND_WEB_BACKEND` = `off` | `duckduckgo` | `parallel` (explicit override)
/// * `FIRECRAWL_API_KEY` → Firecrawl
/// * `PARALLEL_API_KEY` → Parallel (keyed REST)
/// * `TAVILY_API_KEY` → Tavily
/// * `EXA_API_KEY` → Exa
/// * `SEARXNG_URL` → SearXNG (public instance; URL-gated)
/// * `BRAVE_SEARCH_API_KEY` → Brave
/// * default → Parallel free MCP → DuckDuckGo
pub fn build_web_search_backend() -> Arc<dyn WebBackend> {
    fn ddg() -> Arc<dyn WebBackend> {
        Arc::new(DuckDuckGoWebBackend::new())
    }
    fn chain(primary: Arc<dyn WebBackend>) -> Arc<dyn WebBackend> {
        Arc::new(ChainedWebBackend::new(primary, ddg()))
    }

    // A. Explicit override always wins.
    match resolve_backend_choice(std::env::var("WAYLAND_WEB_BACKEND").ok().as_deref()) {
        WebBackendChoice::Off => {
            tracing::info!("web search: disabled (WAYLAND_WEB_BACKEND=off)");
            return Arc::new(DisabledWebBackend);
        }
        WebBackendChoice::DuckDuckGo => {
            tracing::info!("web search: DuckDuckGo (WAYLAND_WEB_BACKEND=duckduckgo)");
            return ddg();
        }
        WebBackendChoice::Parallel => {
            disclose_parallel_once();
            return chain(Arc::new(ParallelWebBackend::free()));
        }
        WebBackendChoice::Auto => {}
    }

    // 1..6 — the provider preference order, first key present wins; each floors on DDG.
    if let Some(key) = read_env_key("FIRECRAWL_API_KEY") {
        tracing::info!("web search: Firecrawl (FIRECRAWL_API_KEY found)");
        return chain(Arc::new(FirecrawlWebBackend::new(key)));
    }
    if let Some(key) = read_env_key("PARALLEL_API_KEY") {
        tracing::info!("web search: Parallel keyed (PARALLEL_API_KEY found)");
        return chain(Arc::new(ParallelWebBackend::keyed(key)));
    }
    if let Some(key) = read_env_key("TAVILY_API_KEY") {
        tracing::info!("web search: Tavily (TAVILY_API_KEY found)");
        return chain(Arc::new(TavilyWebBackend::new(key)));
    }
    if let Some(key) = read_env_key("EXA_API_KEY") {
        tracing::info!("web search: Exa (EXA_API_KEY found)");
        return chain(Arc::new(ExaWebBackend::new(key)));
    }
    if let Some(url) = read_env_key("SEARXNG_URL") {
        tracing::info!("web search: SearXNG (SEARXNG_URL found)");
        return chain(Arc::new(SearxngWebBackend::new(url)));
    }
    if let Some(key) = read_env_key("BRAVE_SEARCH_API_KEY") {
        tracing::info!("web search: Brave (BRAVE_SEARCH_API_KEY found)");
        return chain(Arc::new(BraveWebBackend::new(key)));
    }

    // 7 — keyless default: Parallel free → DuckDuckGo, with privacy disclosure.
    disclose_parallel_once();
    chain(Arc::new(ParallelWebBackend::free()))
}

/// `WebBackend` returned when `WAYLAND_WEB_BACKEND=off` — every call fails
/// loudly so the model knows web search is intentionally disabled.
pub struct DisabledWebBackend;

#[async_trait]
impl WebBackend for DisabledWebBackend {
    async fn search(&self, _query: &str, _limit: u32) -> WebOutcome {
        disabled_err()
    }
    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        disabled_err()
    }
    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        disabled_err()
    }
    fn backend_id(&self) -> &str {
        "disabled"
    }
}

fn disabled_err() -> WebOutcome {
    WebOutcome::Err {
        message: "web search is disabled (WAYLAND_WEB_BACKEND=off). Unset it or set it to \
                  `auto`/`duckduckgo`/`parallel` to re-enable."
            .to_string(),
    }
}

/// Pick the best available vision backend from env keys.
///
/// Order:
/// 1. `ANTHROPIC_API_KEY` → Claude vision
/// 2. `OPENAI_API_KEY` → GPT-4o vision
/// 3. `GEMINI_API_KEY` → Gemini 2.5 Flash vision
pub fn build_vision_backend() -> Option<Arc<dyn VisionBackend>> {
    if let Some(key) = read_env_key("ANTHROPIC_API_KEY") {
        tracing::info!("vision: using Anthropic (ANTHROPIC_API_KEY found)");
        return Some(Arc::new(AnthropicVisionBackend::new(key)));
    }
    if let Some(key) = read_env_key("OPENAI_API_KEY") {
        tracing::info!("vision: using OpenAI (OPENAI_API_KEY found)");
        return Some(Arc::new(OpenAiVisionBackend::new(key)));
    }
    if let Some(key) = read_env_key("GEMINI_API_KEY") {
        tracing::info!("vision: using Gemini (GEMINI_API_KEY found)");
        return Some(Arc::new(GeminiVisionBackend::new(key)));
    }
    tracing::warn!(
        "vision: no API key found (ANTHROPIC/OPENAI/GEMINI) — vision tool will be hidden"
    );
    None
}

/// Pick the best available transcription backend.
///
/// Order:
/// 1. `GROQ_API_KEY` → Groq Whisper Large v3 Turbo (free tier; fast)
/// 2. `OPENAI_API_KEY` → OpenAI whisper-1 (paid)
pub fn build_transcription_backend() -> Option<Arc<dyn TranscriptionBackend>> {
    if let Some(key) = read_env_key("GROQ_API_KEY") {
        tracing::info!("transcription: using Groq Whisper (GROQ_API_KEY found, free tier)");
        return Some(Arc::new(OpenAiCompatWhisperBackend::new(
            key,
            "https://api.groq.com/openai/v1/audio/transcriptions".to_string(),
            "whisper-large-v3-turbo".to_string(),
            "groq",
        )));
    }
    if let Some(key) = read_env_key("OPENAI_API_KEY") {
        tracing::info!("transcription: using OpenAI Whisper (OPENAI_API_KEY found)");
        return Some(Arc::new(OpenAiCompatWhisperBackend::new(
            key,
            "https://api.openai.com/v1/audio/transcriptions".to_string(),
            "whisper-1".to_string(),
            "openai",
        )));
    }
    tracing::warn!(
        "transcription: no API key found (GROQ_API_KEY or OPENAI_API_KEY) — tool hidden"
    );
    None
}

/// Real `ImageFetcher` over reqwest. Reuses the SSRF-safe client so
/// `private` / `internal` networks are rejected before the GET fires.
pub struct HttpImageFetcher {
    client: Client,
}

impl HttpImageFetcher {
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for HttpImageFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageFetcher for HttpImageFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(20))
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (compatible; wayland-core/Vision)",
            )
            .header(reqwest::header::ACCEPT, "image/*,*/*;q=0.8")
            .send()
            .await
            .map_err(|e| format!("image fetch failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "image fetch returned HTTP {}",
                resp.status().as_u16()
            ));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("image body read failed: {e}"))?;
        Ok(bytes.to_vec())
    }
}

/// Constructor for [`HttpImageFetcher`].
pub fn build_image_fetcher() -> Arc<dyn ImageFetcher> {
    Arc::new(HttpImageFetcher::new())
}

/// Real `AudioFetcher` over reqwest, mirroring `HttpImageFetcher`.
pub struct HttpAudioFetcher {
    client: Client,
}

impl HttpAudioFetcher {
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for HttpAudioFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AudioFetcher for HttpAudioFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(30))
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (compatible; wayland-core/Transcribe)",
            )
            .send()
            .await
            .map_err(|e| format!("audio fetch failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "audio fetch returned HTTP {}",
                resp.status().as_u16()
            ));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("audio body read failed: {e}"))?;
        Ok(bytes.to_vec())
    }
}

pub fn build_audio_fetcher() -> Arc<dyn AudioFetcher> {
    Arc::new(HttpAudioFetcher::new())
}

/// The four real backends for the API-seam catalog tools.
pub struct ApiToolBackends {
    pub github: Arc<dyn GitHubBackend>,
    pub gitlab: Arc<dyn GitLabBackend>,
    pub linear: Arc<dyn LinearBackend>,
    pub notion: Arc<dyn NotionBackend>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_tools::github_tool::{GitHubOutcome, GitHubRequest, HttpMethod as GhMethod};
    use wcore_tools::gitlab_tool::{GitLabOutcome, GitLabRequest, HttpMethod as GlMethod};
    use wcore_tools::linear_tool::{LinearOutcome, LinearRequest};
    use wcore_tools::notion_tool::{HttpMethod as NoMethod, NotionOutcome, NotionRequest};
    use wcore_tools::url_safety::is_safe_url;
    use wcore_tools::web_fetch::{FetchOutcome, FetchRequest};

    #[test]
    fn resolve_backend_choice_maps_overrides() {
        assert_eq!(resolve_backend_choice(Some("off")), WebBackendChoice::Off);
        assert_eq!(
            resolve_backend_choice(Some("DISABLED")),
            WebBackendChoice::Off
        );
        assert_eq!(
            resolve_backend_choice(Some(" DuckDuckGo ")),
            WebBackendChoice::DuckDuckGo
        );
        assert_eq!(
            resolve_backend_choice(Some("ddg")),
            WebBackendChoice::DuckDuckGo
        );
        assert_eq!(
            resolve_backend_choice(Some("parallel")),
            WebBackendChoice::Parallel
        );
        assert_eq!(
            resolve_backend_choice(Some("garbage")),
            WebBackendChoice::Auto
        );
        assert_eq!(resolve_backend_choice(None), WebBackendChoice::Auto);
    }

    #[tokio::test]
    async fn disabled_backend_errors_on_every_op() {
        let b = DisabledWebBackend;
        assert!(matches!(b.search("q", 5).await, WebOutcome::Err { .. }));
    }

    #[test]
    fn parse_json_or_raw_handles_json() {
        let v = parse_json_or_raw(r#"{"a":1}"#);
        assert_eq!(v.get("a").and_then(Value::as_i64), Some(1));
    }

    #[test]
    fn parse_json_or_raw_handles_plain_text() {
        let v = parse_json_or_raw("not json");
        assert_eq!(v.as_str(), Some("not json"));
    }

    #[test]
    fn parse_json_or_raw_handles_empty() {
        assert_eq!(parse_json_or_raw(""), Value::Null);
        assert_eq!(parse_json_or_raw("   "), Value::Null);
    }

    #[test]
    fn error_message_prefers_message_field() {
        let v = serde_json::json!({"message": "bad credentials"});
        assert_eq!(error_message(&v, "fallback"), "bad credentials");
    }

    #[test]
    fn error_message_falls_back() {
        let v = serde_json::json!({"other": "x"});
        assert_eq!(error_message(&v, "fallback"), "fallback");
    }

    #[test]
    fn build_api_tool_backends_constructs_all_four() {
        let backends = build_api_tool_backends();
        assert_eq!(Arc::strong_count(&backends.github), 1);
        assert_eq!(Arc::strong_count(&backends.gitlab), 1);
        assert_eq!(Arc::strong_count(&backends.linear), 1);
        assert_eq!(Arc::strong_count(&backends.notion), 1);
    }

    #[test]
    fn ssrf_safe_client_constructs_without_panic() {
        let _client = build_ssrf_safe_tool_client();
    }

    #[test]
    fn redirect_to_aws_metadata_blocked_by_policy() {
        assert!(
            !is_safe_url("http://169.254.169.254/latest/meta-data/iam/security-credentials/"),
            "AWS metadata endpoint must be rejected"
        );
        assert!(
            !is_safe_url("http://169.254.170.2/v2/credentials/"),
            "ECS task metadata endpoint must be rejected"
        );
        assert!(
            !is_safe_url("http://10.0.0.1/internal"),
            "RFC1918 private IP must be rejected"
        );
        assert!(
            !is_safe_url("http://192.168.1.1/router"),
            "RFC1918 192.168.x.x must be rejected"
        );
    }

    #[test]
    fn legitimate_http_to_https_redirect_allowed_by_policy() {
        assert!(
            is_safe_url("https://93.184.216.34/"),
            "public IP should be allowed through redirect policy"
        );
    }

    #[tokio::test]
    async fn fetch_backend_refuses_redirect_to_cloud_metadata() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let backend = HttpFetchBackend::new();
        let req = FetchRequest {
            url: server.uri(),
            timeout_ms: 5_000,
            readable: false,
        };
        let outcome = backend.fetch(&req).await;
        match outcome {
            FetchOutcome::Err { message } => {
                assert!(
                    message.contains("redirect") || message.contains("blocked"),
                    "expected redirect-blocked error, got: {message}"
                );
            }
            other => panic!("expected FetchOutcome::Err for SSRF redirect, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_backend_refuses_redirect_to_private_ip() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "http://10.0.0.1/secret"),
            )
            .mount(&server)
            .await;

        let backend = HttpFetchBackend::new();
        let req = FetchRequest {
            url: server.uri(),
            timeout_ms: 5_000,
            readable: false,
        };
        let outcome = backend.fetch(&req).await;
        match outcome {
            FetchOutcome::Err { message } => {
                assert!(
                    message.contains("redirect") || message.contains("blocked"),
                    "expected redirect-blocked error, got: {message}"
                );
            }
            other => panic!("expected FetchOutcome::Err for private-IP redirect, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn github_backend_refuses_redirect_to_cloud_metadata() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let backend = HttpGitHubBackend::new();
        let req = GitHubRequest {
            method: GhMethod::Get,
            url: format!("{}/repos/owner/repo", server.uri()),
            headers: vec![("Accept".into(), "application/vnd.github+json".into())],
            body: None,
        };
        // Need explicit trait import to dispatch
        use wcore_tools::github_tool::GitHubBackend as _;
        let outcome = backend.dispatch(&req).await;
        match outcome {
            GitHubOutcome::Err { message } => {
                assert!(
                    message.contains("redirect") || message.contains("blocked"),
                    "expected redirect-blocked error from GitHub backend, got: {message}"
                );
            }
            other => panic!("expected GitHubOutcome::Err for SSRF redirect, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn gitlab_backend_refuses_redirect_to_cloud_metadata() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let backend = HttpGitLabBackend::new();
        let req = GitLabRequest {
            action: "get_issue".to_string(),
            method: GlMethod::Get,
            url: format!("{}/api/v4/projects/1/issues/1", server.uri()),
            private_token: String::new(),
            body: None,
        };
        use wcore_tools::gitlab_tool::GitLabBackend as _;
        let outcome = backend.dispatch(&req).await;
        match outcome {
            GitLabOutcome::Err { message, .. } => {
                assert!(
                    message.contains("redirect") || message.contains("blocked"),
                    "expected redirect-blocked error from GitLab backend, got: {message}"
                );
            }
            other => panic!("expected GitLabOutcome::Err for SSRF redirect, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn linear_backend_refuses_redirect_to_private_ip() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "http://10.0.0.1/internal"),
            )
            .mount(&server)
            .await;

        let backend = HttpLinearBackend::new();
        let req = LinearRequest {
            url: server.uri(),
            headers: vec![("Authorization".into(), "Bearer test".into())],
            body: serde_json::json!({"query": "{ viewer { id } }", "variables": {}}),
        };
        use wcore_tools::linear_tool::LinearBackend as _;
        let outcome = backend.dispatch(&req).await;
        match outcome {
            LinearOutcome::Err { message } => {
                assert!(
                    message.contains("redirect") || message.contains("blocked"),
                    "expected redirect-blocked error from Linear backend, got: {message}"
                );
            }
            other => panic!("expected LinearOutcome::Err for SSRF redirect, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn notion_backend_refuses_redirect_to_cloud_metadata() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let backend = HttpNotionBackend::new();
        let req = NotionRequest {
            method: NoMethod::Get,
            url: format!("{}/v1/pages/abc", server.uri()),
            headers: vec![
                ("Authorization".into(), "Bearer test".into()),
                ("Notion-Version".into(), "2022-06-28".into()),
            ],
            body: None,
        };
        use wcore_tools::notion_tool::NotionBackend as _;
        let outcome = backend.dispatch(&req).await;
        match outcome {
            NotionOutcome::Err { message } => {
                assert!(
                    message.contains("redirect") || message.contains("blocked"),
                    "expected redirect-blocked error from Notion backend, got: {message}"
                );
            }
            other => panic!("expected NotionOutcome::Err for SSRF redirect, got: {other:?}"),
        }
    }
}
