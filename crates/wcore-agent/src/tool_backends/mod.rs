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
use wcore_config::config::{Config, ProviderType};
use wcore_egress::EgressClient as Client;
use wcore_tools::url_safety::{SsrfSafeResolver, ssrf_safe_redirect_policy};

// Trait imports for the four API-seam catalog backends — the
// `ApiToolBackends` struct holds `Arc<dyn _Backend>` for each.
use wcore_tools::github_tool::GitHubBackend;
use wcore_tools::gitlab_tool::GitLabBackend;
use wcore_tools::linear_tool::LinearBackend;
use wcore_tools::media_cost::MediaAccounting;
use wcore_tools::notion_tool::NotionBackend;
use wcore_tools::transcription_tools::{AudioFetcher, TranscriptionBackend};
use wcore_tools::vision_tools::{ImageFetcher, VisionBackend};
use wcore_tools::web_fetch::FetchBackend;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

// -- Sub-modules: one file per backend (v0.9.0 W1 B0 split). --
pub mod announcing_web;
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
pub use announcing_web::{AnnouncingWebBackend, WebNotice};
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
    build_ssrf_safe_tool_client_with_origin(wcore_egress::EgressOrigin::Product)
}

/// `build_ssrf_safe_tool_client`, but declaring who chose the destination.
///
/// wayland#1264. Almost every backend here is [`EgressOrigin::Product`]: its URL
/// is built by `format!` against a fixed API host with encoded path segments, so
/// the operator's allowlist entry for that host authorises exactly the traffic
/// that follows. `WebFetch` is not — its URL, host and query string included, is
/// taken verbatim out of tool input, so an allowlist entry for `github.com`
/// would otherwise authorise `https://github.com/?leak=<secret>`.
///
/// A new backend that fetches a model-supplied URL must pass
/// [`EgressOrigin::ModelDirected`] here. That is a deliberate opt-in and not
/// self-enforcing; `web_fetch_is_model_directed` pins the one call site that
/// exists today so a refactor cannot silently drop it.
pub(crate) fn build_ssrf_safe_tool_client_with_origin(
    origin: wcore_egress::EgressOrigin,
) -> Client {
    Client::builder()
        .origin(origin)
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
    /// The variable was set to something nothing recognises. Distinct from
    /// `Auto`: the user asked for a backend and did not get it, and collapsing
    /// the two is what made `WAYLAND_WEB_BACKEND=tavily` a silent no-op.
    Unknown,
}

/// The values `WAYLAND_WEB_BACKEND` actually accepts. Only these three plus
/// unset select anything; a key-named value (`tavily`, `brave`, …) is NOT a
/// selector — those backends are chosen by their key being present.
const WEB_BACKEND_VALUES: &str = "off | duckduckgo | parallel (or unset for auto)";

fn resolve_backend_choice(raw: Option<&str>) -> WebBackendChoice {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("auto") => WebBackendChoice::Auto,
        Some("off") | Some("none") | Some("disabled") => WebBackendChoice::Off,
        Some("duckduckgo") | Some("ddg") => WebBackendChoice::DuckDuckGo,
        Some("parallel") => WebBackendChoice::Parallel,
        _ => WebBackendChoice::Unknown,
    }
}

/// What to tell the user when they set the variable to a value that does
/// nothing. Fail-open — an unusable value must not make web search refuse to
/// run, which would turn a typo into a second dead end — but never silent.
fn unknown_backend_notice(raw: &str) -> String {
    format!(
        "web search: WAYLAND_WEB_BACKEND={raw} is not a recognised value and was ignored \
         (accepted: {WEB_BACKEND_VALUES}). Backends with keys are selected by setting the key \
         itself, not this variable: FIRECRAWL_API_KEY / PARALLEL_API_KEY / TAVILY_API_KEY / \
         EXA_API_KEY / SEARXNG_URL / BRAVE_SEARCH_API_KEY."
    )
}

/// One-time privacy disclosure for the anonymous Parallel default — emitted
/// the first time the keyless/`parallel` path is selected, not on every search.
const PARALLEL_DISCLOSURE: &str = "web search: no search key is set, so your search queries are sent to the free \
     anonymous search at parallel.ai. To stop that: WAYLAND_WEB_BACKEND=off disables web \
     search entirely, and WAYLAND_WEB_BACKEND=duckduckgo keeps queries on DuckDuckGo - but \
     that endpoint is a free HTML scrape which rate-limits by IP after roughly two queries, \
     with no fallback behind it, so it is not a durable answer. The durable one: get a free \
     Tavily API key at https://app.tavily.com (no credit card, 1,000 searches/month) and set \
     TAVILY_API_KEY. FIRECRAWL_API_KEY / EXA_API_KEY / SEARXNG_URL / BRAVE_SEARCH_API_KEY are \
     honoured too.";

/// Marker recording that the disclosure has been shown to this user once.
const PARALLEL_DISCLOSURE_MARKER: &str = ".parallel-disclosure-shown";

/// The keyless privacy disclosure, as a pending notice - or `None` if this
/// user has already been shown it.
///
/// gh#1080 established that the record alone is not a disclosure. What it got
/// wrong was the sink: an `eprintln!` here runs inside `Bootstrap::build()`,
/// and `main.rs` enters the TUI alt-screen BEFORE calling `build()`, so in the
/// default mode the notice was painted onto a buffer the splash overwrote and
/// `LeaveAlternateScreen` then discarded. Worse, the marker was written next
/// to that unseen `eprintln!`, so the first (invisible) launch spent the
/// once-per-user budget and suppressed every later headless run where stderr
/// would have worked.
///
/// So this only PREPARES the notice. Delivery - and the marker write that
/// records it - happens in [`AnnouncingWebBackend`], on the first search,
/// where every mode renders it. Every failure here degrades to showing the
/// notice rather than swallowing it: an unreadable config dir must never be
/// the reason a disclosure is skipped.
fn parallel_disclosure_notice() -> Option<WebNotice> {
    // The structured record always happens; it is what a support bundle reads
    // back, and it is independent of whether the user has seen the notice.
    tracing::info!("{PARALLEL_DISCLOSURE}");

    let marker = wcore_config::config::wayland_config_dir().join(PARALLEL_DISCLOSURE_MARKER);
    if marker.exists() {
        return None;
    }
    Some(WebNotice {
        text: PARALLEL_DISCLOSURE.to_string(),
        marker_on_delivery: Some(marker),
    })
}

/// Pick the active `WebBackend`. Explicit `WAYLAND_WEB_BACKEND` wins; otherwise
/// the first configured key (the provider preference order) is used. Every selected
/// primary is wrapped so it falls back to DuckDuckGo on failure - DDG is the
/// floor for all tiers except an explicit `off` and an explicit `duckduckgo`
/// (which IS the floor, and is left unchained deliberately: a user who asked to
/// keep queries on DuckDuckGo must not have them silently sent elsewhere).
///
/// Resolution order (first match wins):
/// * `WAYLAND_WEB_BACKEND` = `off` | `duckduckgo` | `parallel` (explicit override)
/// * `FIRECRAWL_API_KEY` -> Firecrawl
/// * `PARALLEL_API_KEY` -> Parallel (keyed REST)
/// * `TAVILY_API_KEY` -> Tavily
/// * `EXA_API_KEY` -> Exa
/// * `SEARXNG_URL` -> SearXNG (public instance; URL-gated)
/// * `BRAVE_SEARCH_API_KEY` -> Brave
/// * default -> Parallel free MCP -> DuckDuckGo
///
/// Anything this function needs to TELL the user about the choice it made is
/// returned as a [`WebNotice`] on the wrapper rather than printed here: this
/// runs inside `Bootstrap::build()`, after the TUI alt-screen is already up,
/// so nothing written to stdio at this point survives to be read.
pub fn build_web_search_backend() -> Arc<dyn WebBackend> {
    fn ddg() -> Arc<dyn WebBackend> {
        Arc::new(DuckDuckGoWebBackend::new())
    }
    fn chain(primary: Arc<dyn WebBackend>) -> Arc<dyn WebBackend> {
        Arc::new(ChainedWebBackend::new(primary, ddg()))
    }

    let raw = std::env::var("WAYLAND_WEB_BACKEND").ok();
    let mut notices: Vec<WebNotice> = Vec::new();

    // A. Explicit override always wins.
    match resolve_backend_choice(raw.as_deref()) {
        WebBackendChoice::Off => {
            tracing::info!("web search: disabled (WAYLAND_WEB_BACKEND=off)");
            return Arc::new(DisabledWebBackend);
        }
        WebBackendChoice::DuckDuckGo => {
            tracing::info!("web search: DuckDuckGo (WAYLAND_WEB_BACKEND=duckduckgo)");
            return ddg();
        }
        WebBackendChoice::Parallel => {
            notices.extend(parallel_disclosure_notice());
            return AnnouncingWebBackend::wrap(
                chain(Arc::new(ParallelWebBackend::free())),
                notices,
            );
        }
        WebBackendChoice::Unknown => {
            // Fail-open onto the ladder, but say so. A value that selects
            // nothing used to be indistinguishable from unset, so a user who
            // typed `tavily` got a different backend than they asked for and
            // was never told - the same dead end as a search that silently
            // returns nothing.
            let text = unknown_backend_notice(raw.as_deref().unwrap_or_default());
            tracing::warn!("{text}");
            notices.push(WebNotice {
                text,
                marker_on_delivery: None,
            });
        }
        WebBackendChoice::Auto => {}
    }

    // 1..6 - the provider preference order, first key present wins; each floors on DDG.
    if let Some(key) = read_env_key("FIRECRAWL_API_KEY") {
        tracing::info!("web search: Firecrawl (FIRECRAWL_API_KEY found)");
        return AnnouncingWebBackend::wrap(chain(Arc::new(FirecrawlWebBackend::new(key))), notices);
    }
    if let Some(key) = read_env_key("PARALLEL_API_KEY") {
        tracing::info!("web search: Parallel keyed (PARALLEL_API_KEY found)");
        return AnnouncingWebBackend::wrap(
            chain(Arc::new(ParallelWebBackend::keyed(key))),
            notices,
        );
    }
    if let Some(key) = read_env_key("TAVILY_API_KEY") {
        tracing::info!("web search: Tavily (TAVILY_API_KEY found)");
        return AnnouncingWebBackend::wrap(chain(Arc::new(TavilyWebBackend::new(key))), notices);
    }
    if let Some(key) = read_env_key("EXA_API_KEY") {
        tracing::info!("web search: Exa (EXA_API_KEY found)");
        return AnnouncingWebBackend::wrap(chain(Arc::new(ExaWebBackend::new(key))), notices);
    }
    if let Some(url) = read_env_key("SEARXNG_URL") {
        tracing::info!("web search: SearXNG (SEARXNG_URL found)");
        return AnnouncingWebBackend::wrap(chain(Arc::new(SearxngWebBackend::new(url))), notices);
    }
    if let Some(key) = read_env_key("BRAVE_SEARCH_API_KEY") {
        tracing::info!("web search: Brave (BRAVE_SEARCH_API_KEY found)");
        return AnnouncingWebBackend::wrap(chain(Arc::new(BraveWebBackend::new(key))), notices);
    }

    // 7 - keyless default: Parallel free -> DuckDuckGo, with privacy disclosure.
    notices.extend(parallel_disclosure_notice());
    AnnouncingWebBackend::wrap(chain(Arc::new(ParallelWebBackend::free())), notices)
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

/// Vision arm for the **active OpenAI-wire provider** when that provider is
/// FluxRouter. `flux-auto` is the router's own alias (the same one the
/// completion path uses); a live round-trip proved it serves
/// `/chat/completions` with an `image_url` base64 `data:` block in exactly the
/// shape [`OpenAiVisionBackend`] already builds, recovering three independent
/// ground truths from a fixture image.
pub const FLUX_ROUTER_VISION_MODEL: &str = "flux-auto";

/// Vision arm for native OpenAI (and the `OPENAI_API_KEY` fallback).
pub const OPENAI_VISION_MODEL: &str = "gpt-4o";

/// Pick the best available vision backend.
///
/// Order (first match wins):
/// 1. `ANTHROPIC_API_KEY` → Claude vision
/// 2. `OPENAI_API_KEY` → GPT-4o vision
/// 3. `GEMINI_API_KEY` → Gemini 2.5 Flash vision
/// 4. **Active OpenAI-wire provider** (native OpenAI or FluxRouter) — resolved
///    key + `base_url` from `Config`, so a configured
///    `[providers.flux-router].base_url` is honoured and the key is never sent
///    to the wrong host (the #310 class of bug).
/// 5. `FLUX_API_KEY` → FluxRouter at its default base URL, for the case where
///    the key is in the environment but FluxRouter is not the active provider.
///
/// **Arms 4 and 5 close `BL-F24-C3-H7`.** Before this, the resolver took no
/// `&Config` at all and read only the three env keys, so inbound vision was
/// unreachable for a FluxRouter user by **code absence, not capability
/// absence** — the capability was measured present on the wire. Worse, the
/// obvious workaround (`OPENAI_API_KEY=<flux key>`) was actively unsafe,
/// because [`OpenAiVisionBackend`] hardcoded `api.openai.com`: it would have
/// **misdirected the credential to a third party** rather than failing closed.
/// Arms 4 and 5 resolve key and host together, so that substitution is never
/// the user's only option.
///
/// **Why 4 and 5 are appended rather than given priority**, mirroring
/// [`build_transcription_backend`] exactly: arms 1-3 are the pre-existing
/// resolution order, and putting the active provider first would silently move
/// every existing Anthropic/OpenAI/Gemini vision user onto a different (and
/// possibly billed) arm. **Arms 4 and 5 are strictly additive — no
/// previously-resolving configuration changes backend.**
pub fn build_vision_backend(config: &Config) -> Option<Arc<dyn VisionBackend>> {
    build_vision_backend_with_accounting(config, &MediaAccounting::default())
}

/// [`build_vision_backend`], with the session cost ledger and the operator's
/// rate card bound to whichever arm resolves.
///
/// Separate entry point rather than a changed signature because the resolver
/// has several callers and only the ones that own a session have anything to
/// bind; the rest get [`MediaAccounting::default`], which records units and
/// `unpriced` exactly as before.
pub fn build_vision_backend_with_accounting(
    config: &Config,
    accounting: &MediaAccounting,
) -> Option<Arc<dyn VisionBackend>> {
    if let Some(key) = read_env_key("ANTHROPIC_API_KEY") {
        tracing::info!("vision: using Anthropic (ANTHROPIC_API_KEY found)");
        return Some(Arc::new(
            AnthropicVisionBackend::new(key).with_accounting(accounting.clone()),
        ));
    }
    if let Some(key) = read_env_key("OPENAI_API_KEY") {
        tracing::info!("vision: using OpenAI (OPENAI_API_KEY found)");
        return Some(Arc::new(
            OpenAiVisionBackend::new(key).with_accounting(accounting.clone()),
        ));
    }
    if let Some(key) = read_env_key("GEMINI_API_KEY") {
        tracing::info!("vision: using Gemini (GEMINI_API_KEY found)");
        return Some(Arc::new(
            GeminiVisionBackend::new(key).with_accounting(accounting.clone()),
        ));
    }
    // 4. Active OpenAI-wire provider (native OpenAI or FluxRouter).
    if let Some(backend) = vision_backend_from_config(config) {
        tracing::info!(
            "vision: using {} at {} (active OpenAI-wire provider)",
            backend.model(),
            backend.endpoint()
        );
        return Some(Arc::new(backend.with_accounting(accounting.clone())));
    }
    // 5. FLUX_API_KEY in the environment without FluxRouter being active.
    if let Some(key) = read_env_key("FLUX_API_KEY") {
        tracing::info!("vision: using FluxRouter {FLUX_ROUTER_VISION_MODEL} (FLUX_API_KEY found)");
        return Some(Arc::new(
            OpenAiVisionBackend::with_endpoint(
                key,
                shared::join_openai_endpoint(
                    wcore_providers::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL,
                    "chat/completions",
                ),
                FLUX_ROUTER_VISION_MODEL.to_string(),
                "flux-router",
            )
            .with_accounting(accounting.clone()),
        ));
    }
    tracing::warn!(
        "vision: no API key found (ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY / \
         FLUX_API_KEY, and no OpenAI-wire provider configured) — vision tool will be hidden"
    );
    None
}

/// Arm 4 of [`build_vision_backend`] — the active OpenAI-wire provider (native
/// OpenAI or FluxRouter), resolved from `Config`.
///
/// Returns the concrete backend (not a trait object) so the resolved endpoint
/// and model are unit-assertable without a network round-trip, mirroring
/// [`transcription_backend_from_config`]. That matters more here than for
/// transcription: the property under test is "the credential leaves with the
/// host it belongs to", which is exactly what a network-free endpoint
/// assertion checks.
pub(crate) fn vision_backend_from_config(config: &Config) -> Option<OpenAiVisionBackend> {
    if config.api_key.trim().is_empty() {
        return None;
    }
    let base = shared::openai_wire_media_base(config)?;
    let (model, backend_id) = match config.provider {
        ProviderType::FluxRouter => (FLUX_ROUTER_VISION_MODEL, "flux-router"),
        _ => (OPENAI_VISION_MODEL, "openai"),
    };
    // `WAYLAND_VISION_MODEL` keeps working as the operator override on this
    // arm too, matching `OpenAiVisionBackend::new`.
    let model = read_env_key("WAYLAND_VISION_MODEL").unwrap_or_else(|| model.to_string());
    Some(OpenAiVisionBackend::with_endpoint(
        config.api_key.clone(),
        shared::join_openai_endpoint(&base, "chat/completions"),
        model,
        backend_id,
    ))
}

/// Speech-to-text arm used when the **active OpenAI-wire provider is
/// FluxRouter**. A live round-trip proved `flux-voice-fast` serves
/// `/audio/transcriptions` in the exact `verbose_json` shape
/// [`OpenAiCompatWhisperBackend`] already sends (verbatim transcript, with a
/// silence negative control returning different text, so the match is real
/// and not an echo).
pub const FLUX_ROUTER_STT_MODEL: &str = "flux-voice-fast";

/// Speech-to-text arm for native OpenAI (and the `OPENAI_API_KEY` fallback).
pub const OPENAI_STT_MODEL: &str = "whisper-1";

/// Pick the best available transcription backend.
///
/// Order (first match wins):
/// 1. `GROQ_API_KEY` → Groq Whisper Large v3 Turbo (free tier; fast)
/// 2. `OPENAI_API_KEY` → OpenAI whisper-1 (paid)
/// 3. **Active OpenAI-wire provider** (native OpenAI or FluxRouter) — resolved
///    key + `base_url` from `Config`, so a configured
///    `[providers.flux-router].base_url` is honoured and the key is never sent
///    to the wrong host (the #310 class of bug).
/// 4. `FLUX_API_KEY` → FluxRouter at its default base URL, for the case where
///    the key is in the environment but FluxRouter is not the active provider.
///
/// **Why 3 and 4 are appended rather than given priority** (this deviates from
/// [`image_gen::build_image_gen_backend`], which puts the active provider
/// first): Groq's arm is a *free* tier, while a Flux transcription is billed at
/// `$0.016670` with a **10-second floor**. Putting the active provider first
/// would silently move every existing Groq user onto a paid arm. Arms 3 and 4
/// are therefore strictly additive — no previously-resolving configuration
/// changes backend.
pub fn build_transcription_backend(config: &Config) -> Option<Arc<dyn TranscriptionBackend>> {
    build_transcription_backend_with_accounting(config, &MediaAccounting::default())
}

/// [`build_transcription_backend`], with the session cost ledger and the
/// operator's rate card bound to whichever arm resolves.
///
/// The rate card matters here specifically: before this existed, `bootstrap.rs`
/// bound one to image generation and to nothing else, so an operator who had
/// filled in `[tools.media_pricing]` had it silently ignored for every
/// transcription they paid for.
pub fn build_transcription_backend_with_accounting(
    config: &Config,
    accounting: &MediaAccounting,
) -> Option<Arc<dyn TranscriptionBackend>> {
    if let Some(backend) = transcription_backend_from_env(accounting) {
        return Some(backend);
    }
    // 3. Active OpenAI-wire provider (native OpenAI or FluxRouter).
    if let Some(backend) = transcription_backend_from_config(config) {
        tracing::info!(
            "transcription: using {} at {} (active OpenAI-wire provider)",
            backend.model(),
            backend.endpoint()
        );
        return Some(Arc::new(backend.with_accounting(accounting.clone())));
    }
    // 4. FLUX_API_KEY in the environment without FluxRouter being active.
    if let Some(key) = read_env_key("FLUX_API_KEY") {
        tracing::info!(
            "transcription: using FluxRouter {FLUX_ROUTER_STT_MODEL} (FLUX_API_KEY found)"
        );
        return Some(Arc::new(
            OpenAiCompatWhisperBackend::new(
                key,
                shared::join_openai_endpoint(
                    wcore_providers::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL,
                    "audio/transcriptions",
                ),
                FLUX_ROUTER_STT_MODEL.to_string(),
                "flux-router",
            )
            .with_accounting(accounting.clone()),
        ));
    }
    tracing::warn!(
        "transcription: no API key found (GROQ_API_KEY / OPENAI_API_KEY / FLUX_API_KEY, and no \
         OpenAI-wire provider configured) — tool hidden"
    );
    None
}

/// Arm 3 of [`build_transcription_backend`] — the active OpenAI-wire provider
/// (native OpenAI or FluxRouter), resolved from `Config`.
///
/// Returns the concrete backend (not a trait object) so the resolved endpoint
/// and model are unit-assertable without a network round-trip, mirroring
/// [`image_gen::dalle_backend_from_config`].
pub(crate) fn transcription_backend_from_config(
    config: &Config,
) -> Option<OpenAiCompatWhisperBackend> {
    if config.api_key.trim().is_empty() {
        return None;
    }
    let base = shared::openai_wire_media_base(config)?;
    let (model, backend_id) = match config.provider {
        ProviderType::FluxRouter => (FLUX_ROUTER_STT_MODEL, "flux-router"),
        _ => (OPENAI_STT_MODEL, "openai"),
    };
    Some(OpenAiCompatWhisperBackend::new(
        config.api_key.clone(),
        shared::join_openai_endpoint(&base, "audio/transcriptions"),
        model.to_string(),
        backend_id,
    ))
}

/// Arms 1-2 of [`build_transcription_backend`] — the env-only chain. Split out
/// so the config-aware resolver can try it first without duplicating it.
fn transcription_backend_from_env(
    accounting: &MediaAccounting,
) -> Option<Arc<dyn TranscriptionBackend>> {
    if let Some(key) = read_env_key("GROQ_API_KEY") {
        tracing::info!("transcription: using Groq Whisper (GROQ_API_KEY found, free tier)");
        return Some(Arc::new(
            OpenAiCompatWhisperBackend::new(
                key,
                "https://api.groq.com/openai/v1/audio/transcriptions".to_string(),
                "whisper-large-v3-turbo".to_string(),
                "groq",
            )
            .with_accounting(accounting.clone()),
        ));
    }
    if let Some(key) = read_env_key("OPENAI_API_KEY") {
        tracing::info!("transcription: using OpenAI Whisper (OPENAI_API_KEY found)");
        return Some(Arc::new(
            OpenAiCompatWhisperBackend::new(
                key,
                "https://api.openai.com/v1/audio/transcriptions".to_string(),
                OPENAI_STT_MODEL.to_string(),
                "openai",
            )
            .with_accounting(accounting.clone()),
        ));
    }
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
mod parallel_disclosure_tests {
    use super::*;

    /// gh#1080. The notice must name the host, the consequence, and a way out.
    /// Asserting on the CONTENT rather than merely that a string exists: a
    /// disclosure that omits where the queries go is not a disclosure.
    #[test]
    fn the_disclosure_names_the_destination_and_an_opt_out() {
        assert!(
            PARALLEL_DISCLOSURE.contains("parallel.ai"),
            "must name the third party receiving the queries"
        );
        assert!(
            PARALLEL_DISCLOSURE.contains("search queries are sent"),
            "must state what leaves, not merely which backend is active"
        );
        assert!(
            PARALLEL_DISCLOSURE.contains("WAYLAND_WEB_BACKEND=duckduckgo")
                && PARALLEL_DISCLOSURE.contains("=off"),
            "must give the user a way to stop it"
        );
    }

    /// The control for the test above: a message that merely mentions the
    /// backend would satisfy a naive "is it non-empty" check, so pin the
    /// property that actually matters — this is not a generic status line.
    #[test]
    fn the_disclosure_is_not_a_bare_status_line() {
        assert!(
            PARALLEL_DISCLOSURE.len() > 120,
            "a one-line 'using Parallel' status is not a privacy disclosure"
        );
        assert_ne!(
            PARALLEL_DISCLOSURE.trim(),
            "web search: using Parallel.ai free search (anonymous).",
            "the consequence and the opt-out must survive any future edit"
        );
    }

    /// The marker is what makes this once-per-user instead of once-per-process.
    /// If the name ever collides with a real config file the notice would be
    /// suppressed on a fresh install, which is the one outcome that must not
    /// happen silently.
    #[test]
    fn the_marker_is_a_dotfile_that_cannot_collide_with_config() {
        assert!(PARALLEL_DISCLOSURE_MARKER.starts_with('.'));
        assert!(PARALLEL_DISCLOSURE_MARKER.contains("disclosure"));
        assert!(
            !PARALLEL_DISCLOSURE_MARKER.ends_with(".toml"),
            "must not look like a config file the loader might rewrite"
        );
    }
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

    /// The defect this closes: with a working FluxRouter credential present the
    /// product said "transcription: no API key found (GROQ_API_KEY or
    /// OPENAI_API_KEY) — tool hidden", because the resolver recognised only
    /// those two env vars. FluxRouter serves `/v1/audio/transcriptions` in the
    /// exact `verbose_json` shape this backend already sends (proven by a live
    /// verbatim round-trip with a silence negative control).
    #[test]
    fn flux_router_config_resolves_a_transcription_backend() {
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux".into(),
            base_url: String::new(), // Tier-2 newtype supplies the default
            ..Config::default()
        };
        let backend = transcription_backend_from_config(&cfg)
            .expect("a FluxRouter config with a key must resolve an STT backend");
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/audio/transcriptions"
        );
        assert_eq!(backend.model(), FLUX_ROUTER_STT_MODEL);
        assert_eq!(backend.backend_id(), "flux-router");
    }

    /// A configured `[providers.flux-router].base_url` must be honoured rather
    /// than the key being sent to a hardcoded host (the #310 bug class).
    #[test]
    fn transcription_honours_a_configured_base_url() {
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux".into(),
            base_url: "https://flux.internal.example/v1".into(),
            ..Config::default()
        };
        let backend = transcription_backend_from_config(&cfg).expect("must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://flux.internal.example/v1/audio/transcriptions"
        );
    }

    /// Native OpenAI keeps `whisper-1`; only FluxRouter gets the flux arm.
    #[test]
    fn openai_wire_config_keeps_whisper_one() {
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-o".into(),
            base_url: "https://api.openai.com".into(),
            ..Config::default()
        };
        let backend = transcription_backend_from_config(&cfg).expect("must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(backend.model(), OPENAI_STT_MODEL);
    }

    /// Providers that do not serve the OpenAI-wire media routes must NOT be
    /// routed transcription (it would 404), and an empty key resolves nothing.
    #[test]
    fn non_media_provider_and_empty_key_resolve_nothing() {
        for p in [
            ProviderType::Anthropic,
            ProviderType::Gemini,
            ProviderType::Groq,
        ] {
            let cfg = Config {
                provider: p,
                api_key: "k".into(),
                ..Config::default()
            };
            assert!(
                transcription_backend_from_config(&cfg).is_none(),
                "{p:?} has no OpenAI-wire transcription route"
            );
        }
        let empty = Config {
            provider: ProviderType::FluxRouter,
            api_key: "   ".into(),
            ..Config::default()
        };
        assert!(transcription_backend_from_config(&empty).is_none());
    }

    // -- Vision config seam (BL-F24-C3-H7) -------------------------------

    /// The defect this closes: `build_vision_backend()` took **no `&Config`**
    /// and read only ANTHROPIC / OPENAI / GEMINI, so inbound vision was
    /// unreachable for a FluxRouter user by code absence — while the capability
    /// was live on the wire (HTTP 200, three ground truths recovered).
    #[test]
    fn flux_router_config_resolves_a_vision_backend() {
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux".into(),
            base_url: String::new(), // Tier-2 newtype supplies the default
            ..Config::default()
        };
        let backend = vision_backend_from_config(&cfg)
            .expect("a FluxRouter config with a key must resolve a vision backend");
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/chat/completions"
        );
        assert_eq!(backend.backend_id(), "flux-router");
    }

    /// **The misdirection guard.** This is the assertion that matters most in
    /// this file. `OpenAiVisionBackend` used to hardcode
    /// `https://api.openai.com/v1/chat/completions` while taking a
    /// caller-supplied key, so pointing it at FluxRouter would have shipped a
    /// FluxRouter credential to OpenAI — a third party — rather than failing
    /// closed. Key and host must now always be resolved together: a Flux key
    /// must never resolve an `openai.com` endpoint.
    ///
    /// Checkable with no network, which is the point — the property is about
    /// where the credential is *addressed*, not whether the call succeeds.
    #[test]
    fn a_flux_credential_never_resolves_an_openai_host() {
        for base in [
            "",
            "https://api.fluxrouter.ai/v1",
            "https://flux.internal/v1",
        ] {
            let cfg = Config {
                provider: ProviderType::FluxRouter,
                api_key: "sk-flux-secret".into(),
                base_url: base.into(),
                ..Config::default()
            };
            let backend = vision_backend_from_config(&cfg).expect("must resolve");
            assert!(
                !backend.endpoint().contains("openai.com"),
                "a FluxRouter credential resolved endpoint {} — that would misdirect the key \
                 to a third party (BL-F24-C3-H7)",
                backend.endpoint()
            );
        }
    }

    /// A configured `base_url` must be honoured rather than the key being sent
    /// to a hardcoded host (the #310 bug class, now closed for vision too).
    #[test]
    fn vision_honours_a_configured_base_url() {
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux".into(),
            base_url: "https://flux.internal.example/v1".into(),
            ..Config::default()
        };
        let backend = vision_backend_from_config(&cfg).expect("must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://flux.internal.example/v1/chat/completions"
        );
    }

    /// Native OpenAI keeps `gpt-4o` and its own host; only FluxRouter gets the
    /// flux arm. Proves arm 4 did not smear one provider's model onto another.
    #[test]
    fn openai_wire_config_keeps_gpt4o() {
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-o".into(),
            base_url: "https://api.openai.com".into(),
            ..Config::default()
        };
        let backend = vision_backend_from_config(&cfg).expect("must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(backend.model(), OPENAI_VISION_MODEL);
        assert_eq!(backend.backend_id(), "openai");
    }

    /// Providers that do not serve the OpenAI-wire chat route must NOT be
    /// routed vision, and an empty key resolves nothing. Without this, arm 4
    /// would be a credential-misrouting machine for every other provider.
    #[test]
    fn non_openai_wire_provider_and_empty_key_resolve_no_vision() {
        for p in [
            ProviderType::Anthropic,
            ProviderType::Gemini,
            ProviderType::Groq,
        ] {
            let cfg = Config {
                provider: p,
                api_key: "k".into(),
                ..Config::default()
            };
            assert!(
                vision_backend_from_config(&cfg).is_none(),
                "{p:?} has no OpenAI-wire vision route"
            );
        }
        let empty = Config {
            provider: ProviderType::FluxRouter,
            api_key: "   ".into(),
            ..Config::default()
        };
        assert!(vision_backend_from_config(&empty).is_none());
    }

    /// The `OpenAiVisionBackend::new` env arm must be unchanged by the
    /// refactor — arms 1-3 are pre-existing behaviour and this change is
    /// strictly additive.
    #[test]
    fn openai_env_arm_still_targets_openai() {
        let b = crate::tool_backends::openai_vision::OpenAiVisionBackend::new("sk-o".into());
        assert_eq!(b.endpoint(), "https://api.openai.com/v1/chat/completions");
        assert_eq!(b.backend_id(), "openai");
    }

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
        // A value nothing recognises is NOT the same as unset. Collapsing the
        // two is the defect: `WAYLAND_WEB_BACKEND=tavily` selected a different
        // backend than the user asked for and said nothing about it.
        assert_eq!(
            resolve_backend_choice(Some("garbage")),
            WebBackendChoice::Unknown
        );
        assert_eq!(resolve_backend_choice(None), WebBackendChoice::Auto);
        assert_eq!(resolve_backend_choice(Some("")), WebBackendChoice::Auto);
        assert_eq!(resolve_backend_choice(Some("auto")), WebBackendChoice::Auto);
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

#[cfg(test)]
mod web_dead_end_tests {
    use super::*;

    /// RED ARM. `_ => Auto` silently discards five of the eight backend names.
    /// A user who types the value the product's own disclosure text taught them
    /// gets no error, no warning, and a different backend than they asked for.
    #[test]
    fn an_unrecognized_backend_value_is_not_silently_discarded() {
        for raw in ["tavily", "brave", "exa", "firecrawl", "searxng", "typo"] {
            assert_ne!(
                resolve_backend_choice(Some(raw)),
                WebBackendChoice::Auto,
                "`WAYLAND_WEB_BACKEND={raw}` must not resolve to the same thing as unset"
            );
        }
        // Control: unset and the blessed values still resolve as before.
        assert_eq!(resolve_backend_choice(None), WebBackendChoice::Auto);
        assert_eq!(resolve_backend_choice(Some("off")), WebBackendChoice::Off);
        assert_eq!(
            resolve_backend_choice(Some("duckduckgo")),
            WebBackendChoice::DuckDuckGo
        );
    }

    /// RED ARM. The disclosure offers `WAYLAND_WEB_BACKEND=duckduckgo` as a
    /// plain privacy alternative. Measured: that path is UNCHAINED and the free
    /// HTML endpoint locks an IP out after ~two queries for minutes, so the
    /// advice lands the user on the one configuration where failure is
    /// unrecoverable. Whatever it recommends, it must not recommend it silently.
    #[test]
    fn the_disclosure_does_not_recommend_duckduckgo_without_its_limit() {
        let d = PARALLEL_DISCLOSURE.to_ascii_lowercase();
        assert!(d.contains("duckduckgo"), "control: the text names DDG");
        assert!(
            d.contains("rate-limit") || d.contains("rate limit"),
            "recommending the unchained scraped endpoint without saying it \
             rate-limits by IP is the defect: {PARALLEL_DISCLOSURE}"
        );
        assert!(
            d.contains("app.tavily.com"),
            "the disclosure must name the concrete no-card remedy: {PARALLEL_DISCLOSURE}"
        );
    }
}
