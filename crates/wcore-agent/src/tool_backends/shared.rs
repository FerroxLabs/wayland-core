//! Shared helpers for `tool_backends/*` modules.
//!
//! Created in v0.9.0 Wave-1 B0 prep. Houses the canonical env-var
//! resolver (R-H2) and the `urlencode` helper used by multiple search
//! backends. Cross-backend imports go through this module.

use wcore_config::config::{Config, ProviderType};
use wcore_providers::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL;
use wcore_tools::media_cost::ReportedCost;

/// Response headers that carry a per-call dollar figure.
///
/// Phase 27 captured `x-flux-cost-usd` on a live FluxRouter transcription and
/// on a live chat call, while the same account's image call returned no figure
/// in any channel. Vision is a chat call, so the header is expected there too.
///
/// Lives here rather than in `openai_compat_whisper.rs`, where it started: five
/// billable backends need it and three of them were dropping `resp.headers()`
/// on the floor entirely.
pub const COST_HEADERS: &[&str] = &["x-flux-cost-usd", "x-cost-usd", "x-openai-cost-usd"];

/// JSON pointers at which an OpenAI-wire provider may report a per-call cost.
/// FluxRouter returns `usage.cost_usd` on chat completions.
const COST_BODY_POINTERS: &[&str] = &["/usage/cost_usd", "/usage/total_cost_usd", "/cost_usd"];

/// Read a provider-reported cost out of the response headers.
///
/// Returns `None` when no header is present or the value does not parse —
/// **never a zero**. An unparseable header means we do not know the cost, and
/// "unknown" and "free" are not the same claim.
pub fn cost_from_headers(headers: &reqwest::header::HeaderMap) -> Option<ReportedCost> {
    for name in COST_HEADERS {
        if let Some(usd) = headers
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
        {
            return Some(ReportedCost::from_header(*name, usd));
        }
    }
    None
}

/// The user-facing detail for a non-2xx from an OpenAI-wire media endpoint.
///
/// FluxRouter folds its paid-only gating into a 402 on **every** media surface,
/// not just images. #938: the image path ran that body through
/// [`wcore_providers::openai::parse_flux_402`] and told the user which plan they
/// needed, while transcription, vision and speech pasted the provider's JSON
/// envelope into the message — same provider, same status, two different
/// products.
///
/// `capability` names the surface the user was locked out of and is what the
/// typed message renders. An unrecognised body — any non-Flux provider, or a
/// 402 Flux did not send — keeps the truncated raw body it always had, so no
/// diagnostic detail is lost.
///
/// Callers keep their own `returned HTTP {status}` prefix: `video_analyze`
/// classifies `InsufficientCredits` by string-matching the status out of the
/// backend's message, so the status must survive this rewrite.
pub fn http_error_detail(capability: &str, status: u16, body: &str) -> String {
    if status == 402
        && let Some(err) = wcore_providers::openai::parse_flux_402(capability, body)
    {
        return err.to_string();
    }
    body.chars().take(400).collect()
}

/// Read a provider-reported cost out of a parsed JSON response body.
///
/// Used for the chat-wire shapes (vision), where FluxRouter reports the figure
/// in `usage.cost_usd` as well as in the header. Same discipline as
/// [`cost_from_headers`]: a missing or non-numeric field yields `None`, never
/// a zero.
pub fn cost_from_body(parsed: &serde_json::Value) -> Option<ReportedCost> {
    for pointer in COST_BODY_POINTERS {
        if let Some(usd) = parsed
            .pointer(pointer)
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite() && *v >= 0.0)
        {
            // Strip the leading '/' and render as a dotted path, which is how
            // an operator reading the record would name the field.
            let field = pointer.trim_start_matches('/').replace('/', ".");
            return Some(ReportedCost::from_body(field, usd));
        }
    }
    None
}

/// Resolve a provider-reported cost from a response, header first then body.
///
/// The header wins because it is present on every FluxRouter shape measured so
/// far, including ones whose body is not JSON at all (speech synthesis returns
/// raw audio bytes).
pub fn reported_cost(
    headers: &reqwest::header::HeaderMap,
    body: Option<&serde_json::Value>,
) -> Option<ReportedCost> {
    cost_from_headers(headers).or_else(|| body.and_then(cost_from_body))
}

/// Canonical env-var resolver. Returns `Some(key)` only when the env
/// var is set **and** its value is non-empty (closes R-H2: empty-string
/// `OPENAI_API_KEY=""` should NOT count as "configured"). Every new
/// Wave-1 backend resolves credentials through this helper so the
/// "key set but empty" pathology is handled in one place.
/// The single next step every web-search dead end points at.
///
/// Verified 2026-08-26 against Tavily's own pricing page and quickstart:
/// 1,000 API credits per month, refilling monthly, "No credit card required".
/// It is the only option in the surveyed free-tier field a user can complete
/// without a payment form - Brave removed its keyless free tier in Feb 2026
/// (a card is now mandatory and its ToS requires a "POWERED BY BRAVE"
/// attribution), and Google's Custom Search JSON API is closed to new
/// customers. Do NOT widen this text with an unverified URL or quota; a
/// remedy the user cannot complete is worse than no remedy, because they
/// spend the attempt before finding out.
pub const WEB_SEARCH_KEY_REMEDY: &str = "Next step: get a free Tavily API key at https://app.tavily.com (no credit card, \
     1,000 searches/month), then set TAVILY_API_KEY and start wayland-core again.";

pub fn read_env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// Canonical OpenAI API base URL. Used as the fallback for the
/// OpenAI-family tool backends (`image_generate`, `text_to_speech`) when
/// no provider `base_url` is available from `Config` — preserves the
/// pre-#310 behavior of talking directly to `api.openai.com`.
pub const OPENAI_API_BASE: &str = "https://api.openai.com/v1";

/// Join an OpenAI-wire `base_url` (e.g. `https://api.fluxrouter.ai/v1`)
/// with an API sub-path (e.g. `images/generations`) into a full
/// endpoint. Tolerates a trailing slash on the base and a leading slash
/// on the path so callers can pass either form. (#310)
pub fn join_openai_endpoint(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Resolve the OpenAI-wire API root (guaranteed to end in `/v1`) for the
/// providers that actually serve the OpenAI-wire media endpoints
/// (`/images/generations`, `/audio/speech`): native **OpenAI** and our
/// **FluxRouter**. Every other OpenAI-compatible provider (Groq, Together,
/// Deepseek, …) is LLM-completion-only — routing media to them would 404 —
/// and Azure OpenAI uses a deployment-scoped URL scheme, so all of them
/// return `None` here and the caller falls through to the env-key media
/// backends.
///
/// Fills the provider default when `config.base_url` is empty (Tier-2
/// newtypes such as FluxRouter leave it empty and supply the default
/// themselves), then normalizes to a `/v1` root.
///
/// #310 follow-up: the original gate compared `config.provider ==
/// ProviderType::OpenAI`, which never matches `ProviderType::FluxRouter`, so
/// the fix was a silent no-op in a real Flux session (`"flux-router"` parses
/// to `ProviderType::FluxRouter`). FluxRouter is now handled explicitly, and
/// native OpenAI gets the required `/v1` even though its default
/// `config.base_url` is `https://api.openai.com` (no `/v1`).
pub fn openai_wire_media_base(config: &Config) -> Option<String> {
    let raw = match config.provider {
        ProviderType::OpenAI => {
            let b = config.base_url.trim();
            if b.is_empty() {
                "https://api.openai.com"
            } else {
                b
            }
        }
        ProviderType::FluxRouter => {
            let b = config.base_url.trim();
            if b.is_empty() {
                FLUX_ROUTER_DEFAULT_BASE_URL
            } else {
                b
            }
        }
        _ => return None,
    };
    ensure_v1_root(raw)
}

/// Normalize an OpenAI-wire base URL to a `…/v1` root. Returns `None` when
/// the URL is unusable: empty, missing an `http(s)://` scheme, or carrying
/// userinfo (`user:pass@host`) — the latter a credential-confusion / SSRF
/// exfil vector when `base_url` comes from a hostile config. A base that
/// already ends in `/v1` is preserved; otherwise `/v1` is appended.
fn ensure_v1_root(base: &str) -> Option<String> {
    let trimmed = base.trim().trim_end_matches('/');
    let after_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))?;
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    if trimmed.ends_with("/v1") {
        Some(trimmed.to_string())
    } else {
        Some(format!("{trimmed}/v1"))
    }
}

/// Minimal `application/x-www-form-urlencoded` encoder.
///
/// Moved from the monolith `tool_backends.rs` during v0.9.0 B0 prep so
/// `duckduckgo_web` and `brave_web` (and any future search backend)
/// share one copy. The full RFC is overkill — we just need to handle
/// the characters that appear in real-world search queries (spaces,
/// punctuation, accents).
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Map raw search rows into the `{title,url,snippet}` shape, dropping any row
/// that carries no usable information.
///
/// gh#452 — a row needs a non-empty title and an `http(s)` url to be worth
/// showing. Callers MUST treat an empty return as `WebOutcome::Err`, never as
/// an empty success: `ChainedWebBackend` takes any `Ok` as final, so an
/// `Ok{web:[]}` silently disables the DuckDuckGo floor and the user is shown a
/// successful search with nothing in it and no error explaining why.
///
/// `snippet_key` names the per-provider field holding the result text
/// (`content` for Tavily, `description` for Brave).
pub fn map_validated_rows(
    raw_results: &[serde_json::Value],
    snippet_key: &str,
) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    for r in raw_results {
        let title = r
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let url = r
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        let snippet = r
            .get(snippet_key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        results.push(serde_json::json!({ "title": title, "url": url, "snippet": snippet }));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn read_env_key_returns_none_for_unset() {
        // Use a name unlikely to be set.
        // SAFETY: tests run sequentially with `serial_test` per-suite, but
        // this helper does not mutate process env, so a stray-set is fine.
        let v = std::env::var("WAYLAND_TEST_DEFINITELY_UNSET_12345").ok();
        assert!(v.is_none() || v.as_deref() == Some(""));
        assert!(read_env_key("WAYLAND_TEST_DEFINITELY_UNSET_12345").is_none());
    }

    #[serial]
    #[test]
    fn read_env_key_returns_none_for_empty() {
        // SAFETY: tests in this module run on isolated threads; we never
        // assume cross-test env hygiene.
        unsafe { std::env::set_var("WAYLAND_TEST_EMPTY_KEY_VAR", "") };
        assert_eq!(read_env_key("WAYLAND_TEST_EMPTY_KEY_VAR"), None);
        unsafe { std::env::set_var("WAYLAND_TEST_EMPTY_KEY_VAR", "   ") };
        assert_eq!(read_env_key("WAYLAND_TEST_EMPTY_KEY_VAR"), None);
        unsafe { std::env::remove_var("WAYLAND_TEST_EMPTY_KEY_VAR") };
    }

    #[serial]
    #[test]
    fn read_env_key_returns_some_for_set_nonempty() {
        unsafe { std::env::set_var("WAYLAND_TEST_NONEMPTY_KEY_VAR", "secret123") };
        assert_eq!(
            read_env_key("WAYLAND_TEST_NONEMPTY_KEY_VAR"),
            Some("secret123".to_string())
        );
        unsafe { std::env::remove_var("WAYLAND_TEST_NONEMPTY_KEY_VAR") };
    }

    #[test]
    fn join_openai_endpoint_tolerates_slashes() {
        // No trailing/leading slash.
        assert_eq!(
            join_openai_endpoint("https://api.openai.com/v1", "images/generations"),
            "https://api.openai.com/v1/images/generations"
        );
        // Trailing slash on base.
        assert_eq!(
            join_openai_endpoint("https://api.fluxrouter.ai/v1/", "audio/speech"),
            "https://api.fluxrouter.ai/v1/audio/speech"
        );
        // Leading slash on path.
        assert_eq!(
            join_openai_endpoint("https://api.fluxrouter.ai/v1", "/images/generations"),
            "https://api.fluxrouter.ai/v1/images/generations"
        );
    }

    #[test]
    fn urlencode_handles_spaces_and_punctuation() {
        assert_eq!(urlencode("hello world"), "hello+world");
        assert_eq!(urlencode("foo=bar&baz"), "foo%3Dbar%26baz");
        assert_eq!(urlencode("a.b-c_d~e"), "a.b-c_d~e");
    }

    #[test]
    fn media_base_native_openai_appends_v1() {
        // Native OpenAI's resolved base_url is `https://api.openai.com` (no
        // `/v1`); the media root must add it (else the endpoint 404s).
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-o".into(),
            base_url: "https://api.openai.com".into(),
            ..Config::default()
        };
        assert_eq!(
            openai_wire_media_base(&cfg).as_deref(),
            Some("https://api.openai.com/v1")
        );
    }

    #[test]
    fn media_base_flux_router_uses_default_when_base_empty() {
        // Real Flux session shape: provider = FluxRouter, base_url empty (the
        // newtype supplies the default). #310 must fire here.
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux".into(),
            base_url: String::new(),
            ..Config::default()
        };
        assert_eq!(
            openai_wire_media_base(&cfg).as_deref(),
            Some("https://api.fluxrouter.ai/v1")
        );
    }

    #[test]
    fn media_base_none_for_non_openai_wire_media_providers() {
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
                openai_wire_media_base(&cfg).is_none(),
                "{p:?} has no OpenAI-wire media endpoint and must fall through"
            );
        }
    }

    #[test]
    fn media_base_rejects_userinfo_exfil() {
        // A hostile config base_url with userinfo would exfiltrate the key to
        // attacker.com (the @ makes api.openai.com the path, not the host).
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-o".into(),
            base_url: "https://attacker.com@api.openai.com/v1".into(),
            ..Config::default()
        };
        assert!(openai_wire_media_base(&cfg).is_none());
    }

    #[test]
    fn media_base_preserves_explicit_v1_and_trailing_slash() {
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "k".into(),
            base_url: "https://api.fluxrouter.ai/v1/".into(),
            ..Config::default()
        };
        assert_eq!(
            openai_wire_media_base(&cfg).as_deref(),
            Some("https://api.fluxrouter.ai/v1")
        );
    }

    /// #938, BOTH DIRECTIONS. A recognised FluxRouter 402 becomes the typed
    /// entitlement message and names the surface the caller asked for; anything
    /// else keeps the raw body verbatim.
    #[test]
    fn flux_402_becomes_the_typed_message_and_everything_else_stays_raw() {
        let flux = r#"{"error":{"message":"paid plans only","code":"premium_locked"}}"#;

        let stt = http_error_detail("speech-to-text", 402, flux);
        assert_eq!(
            stt,
            "speech-to-text requires a paid Flux plan: paid plans only"
        );
        // The capability is the CALLER's, not a constant: the image path and
        // the speech path must not describe each other's lock.
        let img = http_error_detail("image generation", 402, flux);
        assert_eq!(
            img,
            "image generation requires a paid Flux plan: paid plans only"
        );
        assert_ne!(stt, img);

        // Same body, a status Flux does not gate on -> untouched.
        assert_eq!(http_error_detail("speech-to-text", 500, flux), flux);
        // A 402 from a provider that is not Flux -> untouched.
        let other = "quota exhausted, top up your balance";
        assert_eq!(http_error_detail("speech-to-text", 402, other), other);
        // A 402 carrying no JSON at all -> untouched.
        assert_eq!(
            http_error_detail("vision", 402, "gateway said no"),
            "gateway said no"
        );
    }

    /// The 400-char truncation the raw path has always applied must survive:
    /// this helper replaced four `chars().take(400)` call sites and a
    /// regression here would put an unbounded provider body in front of the
    /// user.
    #[test]
    fn an_unrecognised_body_is_still_truncated_at_400_chars() {
        let long = "x".repeat(5_000);
        assert_eq!(http_error_detail("vision", 402, &long).chars().count(), 400);
        // LIVENESS CONTROL: a short body is not padded or mangled.
        assert_eq!(http_error_detail("vision", 402, "xx"), "xx");
    }
}
