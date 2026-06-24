//! Shared provider-key recognizer and live key-validation.
//!
//! This module is the single source of truth for:
//!  * recognizing a provider from an API-key prefix ([`detect_provider`]),
//!  * scanning the process environment for provider keys ([`scan_env_keys`]),
//!  * picking a provider's read-only validation endpoint
//!    ([`validation_endpoint`]) and performing the live check
//!    ([`validate_key_blocking`]).
//!
//! Both the first-run onboarding surface (`tui::surfaces::onboarding`) and
//! the `wayland-core auth` subcommand (`crate::auth`) import from here so
//! there is exactly ONE recognizer — the prefix table, the env-var map,
//! and the per-provider endpoints never drift between the two surfaces.

use std::time::Duration;

/// Wall-clock budget for the live key-validation request. Generous
/// enough for a slow link, short enough that a hung endpoint does not
/// leave the caller staring at "validating…" forever.
pub const VALIDATE_TIMEOUT: Duration = Duration::from_secs(12);

/// A provider Wayland can connect to with an API key.
///
/// The variant set and the [`Provider::slug`] strings are kept aligned
/// with the engine's `wcore_config::ProviderType` / `parse_builtin_provider`
/// so a slug written here round-trips when the engine reads `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
    OpenRouter,
    Gemini,
    Groq,
    Xai,
    Mistral,
    DeepSeek,
    Fireworks,
    Together,
    Cerebras,
    Perplexity,
    Moonshot,
    Sakana,
}

impl Provider {
    /// Every provider, in picker display order. Used to render the
    /// provider picker shown for ambiguous / unrecognized keys.
    pub const ALL: [Provider; 14] = [
        Provider::Anthropic,
        Provider::OpenAi,
        Provider::OpenRouter,
        Provider::Gemini,
        Provider::Groq,
        Provider::Xai,
        Provider::Mistral,
        Provider::DeepSeek,
        Provider::Fireworks,
        Provider::Together,
        Provider::Cerebras,
        Provider::Perplexity,
        Provider::Moonshot,
        Provider::Sakana,
    ];

    /// Human-readable provider name. Shown on the detect line, the
    /// AddMore card, the Ready card, and the provider picker.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAi => "OpenAI",
            Provider::OpenRouter => "OpenRouter",
            Provider::Gemini => "Google Gemini",
            Provider::Groq => "Groq",
            Provider::Xai => "xAI",
            Provider::Mistral => "Mistral",
            Provider::DeepSeek => "DeepSeek",
            Provider::Fireworks => "Fireworks",
            Provider::Together => "Together",
            Provider::Cerebras => "Cerebras",
            Provider::Perplexity => "Perplexity",
            Provider::Moonshot => "Moonshot",
            Provider::Sakana => "Sakana",
        }
    }

    /// The lowercase identifier used in the config `[providers.<slug>]`
    /// table. Every slug here is one the engine's `parse_builtin_provider`
    /// recognizes (`wcore-config`).
    pub fn slug(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::OpenRouter => "openrouter",
            Provider::Gemini => "gemini",
            Provider::Groq => "groq",
            Provider::Xai => "xai",
            Provider::Mistral => "mistral",
            Provider::DeepSeek => "deepseek",
            Provider::Fireworks => "fireworks",
            Provider::Together => "together",
            Provider::Cerebras => "cerebras",
            Provider::Perplexity => "perplexity",
            Provider::Moonshot => "moonshot",
            Provider::Sakana => "sakana",
        }
    }

    /// Resolve a provider from its config slug (the inverse of
    /// [`Provider::slug`]). Returns `None` for an unrecognized slug.
    pub fn from_slug(slug: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.slug() == slug)
    }
}

/// The outcome of inspecting an API key's prefix.
///
/// The recognizer never *guesses*: a key whose prefix uniquely names a
/// provider resolves to [`Detected::One`]; a bare `sk-` (shared by
/// OpenAI, DeepSeek and Moonshot) is [`Detected::Ambiguous`]; anything
/// else is [`Detected::Unknown`]. The latter two route to a provider
/// picker so the user — not a coin toss — decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detected {
    /// The prefix uniquely identifies this provider.
    One(Provider),
    /// A bare `sk-` key matching no known shape — most likely a legacy
    /// OpenAI key, but unconfirmed.
    Ambiguous,
    /// The prefix matches no known provider.
    Unknown,
}

/// True if `key` is a DeepSeek API key: `sk-` followed by exactly 32
/// hexadecimal characters (DeepSeek's fixed key shape — precise enough
/// to detect without guessing, unlike a bare `sk-`).
pub fn is_deepseek_key(key: &str) -> bool {
    key.trim()
        .strip_prefix("sk-")
        .is_some_and(|rest| rest.len() == 32 && rest.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Recognize a provider from an API-key prefix.
///
/// Prefixes are checked **most-specific first** — the central fix for the
/// "`sk-or-v1-…` detected as OpenAI" bug. `sk-or-v1-` and `sk-proj-` are
/// both more specific than a bare `sk-`, so they must be ruled in before
/// the generic `sk-` branch is ever reached.
pub fn detect_provider(key: &str) -> Detected {
    let key = key.trim();
    // Most-specific prefixes first. Order matters: each `sk-…` long
    // prefix is tested before the bare `sk-` fallthrough below.
    if key.starts_with("sk-ant-") {
        Detected::One(Provider::Anthropic)
    } else if key.starts_with("sk-or-v1-") || key.starts_with("sk-or-") {
        Detected::One(Provider::OpenRouter)
    } else if key.starts_with("sk-proj-")
        || key.starts_with("sk-svcacct-")
        || key.starts_with("sk-admin-")
        || key.starts_with("sk-None-")
    {
        Detected::One(Provider::OpenAi)
    } else if key.starts_with("sk-kimi-") {
        Detected::One(Provider::Moonshot)
    } else if key.starts_with("AIza") {
        Detected::One(Provider::Gemini)
    } else if key.starts_with("gsk_") {
        Detected::One(Provider::Groq)
    } else if key.starts_with("xai-") {
        Detected::One(Provider::Xai)
    } else if key.starts_with("fw_") {
        Detected::One(Provider::Fireworks)
    } else if key.starts_with("pplx-") {
        Detected::One(Provider::Perplexity)
    } else if key.starts_with("csk-") {
        Detected::One(Provider::Cerebras)
    } else if key.starts_with("fish_") {
        // Sakana AI keys are prefixed `fish_` (verified live).
        Detected::One(Provider::Sakana)
    } else if is_deepseek_key(key) {
        // DeepSeek issues `sk-` + exactly 32 hex chars — a shape precise
        // enough to detect without guessing.
        Detected::One(Provider::DeepSeek)
    } else if key.starts_with("sk-") {
        // A bare `sk-…` key matching none of the shapes above — most
        // likely a legacy OpenAI key, but not certain. Let the user pick.
        Detected::Ambiguous
    } else {
        Detected::Unknown
    }
}

/// One provider API key found in the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvKey {
    /// The environment variable name (e.g. `ANTHROPIC_API_KEY`).
    pub var: &'static str,
    /// The provider that variable belongs to.
    pub provider: Provider,
    /// The variable's value — a real API key, never echoed to the screen.
    pub value: String,
}

/// The environment variable → provider map onboarding scans on entry.
/// Two variables map to Gemini (`GEMINI_API_KEY` and Google's older
/// `GOOGLE_API_KEY`); the first one set wins.
pub const ENV_VAR_MAP: [(&str, Provider); 14] = [
    ("ANTHROPIC_API_KEY", Provider::Anthropic),
    ("OPENAI_API_KEY", Provider::OpenAi),
    ("OPENROUTER_API_KEY", Provider::OpenRouter),
    ("GEMINI_API_KEY", Provider::Gemini),
    ("GOOGLE_API_KEY", Provider::Gemini),
    ("GROQ_API_KEY", Provider::Groq),
    ("XAI_API_KEY", Provider::Xai),
    ("MISTRAL_API_KEY", Provider::Mistral),
    ("DEEPSEEK_API_KEY", Provider::DeepSeek),
    ("FIREWORKS_API_KEY", Provider::Fireworks),
    ("TOGETHER_API_KEY", Provider::Together),
    ("CEREBRAS_API_KEY", Provider::Cerebras),
    ("PERPLEXITY_API_KEY", Provider::Perplexity),
    ("SAKANA_API_KEY", Provider::Sakana),
];

/// Scan the process environment for known provider API-key variables.
///
/// Returns one [`EnvKey`] per variable that is set to a non-blank value,
/// de-duplicated by *provider* (so `GEMINI_API_KEY` and `GOOGLE_API_KEY`
/// both being set yields a single Gemini entry — first wins). An empty
/// result means there is nothing to say about the environment.
pub fn scan_env_keys<F: Fn(&str) -> Option<String>>(get: F) -> Vec<EnvKey> {
    let mut found: Vec<EnvKey> = Vec::new();
    for (var, provider) in ENV_VAR_MAP {
        let Some(value) = get(var) else { continue };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        if found.iter().any(|e| e.provider == provider) {
            continue;
        }
        found.push(EnvKey {
            var,
            provider,
            value,
        });
    }
    found
}

/// The outcome of a live key-validation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
    /// The provider accepted the key (HTTP 200 from its models endpoint).
    Ok,
    /// The provider rejected the key — carries a short reason
    /// (`"key rejected (401)"`, `"network error"`, …).
    Failed(String),
}

/// How a provider's validation endpoint authenticates a request.
pub enum AuthStyle {
    /// Anthropic-style `x-api-key` + `anthropic-version` headers.
    AnthropicHeaders,
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// The key is already in the URL's query string — no auth header.
    QueryParam,
}

/// The validation endpoint + auth style for a provider.
///
/// Every URL is a cheap, read-only call that returns `200` for a good
/// key and `401`/`403` for a bad one — it never spends tokens. The
/// returned URL already includes the key for the [`AuthStyle::QueryParam`]
/// case (Gemini), so it must not be logged.
pub fn validation_endpoint(provider: Provider, key: &str) -> (String, AuthStyle) {
    match provider {
        Provider::Anthropic => (
            "https://api.anthropic.com/v1/models".to_string(),
            AuthStyle::AnthropicHeaders,
        ),
        Provider::OpenAi => (
            "https://api.openai.com/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
        // `/models` on OpenRouter is public — it would 200 for any key.
        // `/key` is the endpoint that actually authenticates the key.
        Provider::OpenRouter => (
            "https://openrouter.ai/api/v1/key".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Gemini => (
            format!("https://generativelanguage.googleapis.com/v1beta/models?key={key}"),
            AuthStyle::QueryParam,
        ),
        Provider::Groq => (
            "https://api.groq.com/openai/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Xai => ("https://api.x.ai/v1/models".to_string(), AuthStyle::Bearer),
        Provider::Mistral => (
            "https://api.mistral.ai/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::DeepSeek => (
            "https://api.deepseek.com/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Fireworks => (
            "https://api.fireworks.ai/inference/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Together => (
            "https://api.together.xyz/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Cerebras => (
            "https://api.cerebras.ai/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Perplexity => (
            "https://api.perplexity.ai/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Moonshot => (
            "https://api.moonshot.ai/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
        Provider::Sakana => (
            "https://api.sakana.ai/v1/models".to_string(),
            AuthStyle::Bearer,
        ),
    }
}

/// The terminal status of a single egress GET driven by [`egress_get_status`].
///
/// The async `EgressClient` response is consumed entirely on the worker
/// thread (only `.status()` is read); this small `Send` enum is what crosses
/// the thread/runtime boundary, so a non-`Send` `reqwest::Response` never has
/// to. Callers map it back to their own return type.
pub(crate) enum EgressGetStatus {
    /// The request completed; carries the HTTP status code.
    Status(u16),
    /// The egress policy or transport timed out.
    Timeout,
    /// Any other transport failure (DNS, TLS, connection reset) OR a policy
    /// `Deny`. The probes treat both as "unreachable / not validated".
    Failed,
}

/// Drive a single blocking GET through the `wcore_egress::EgressClient`
/// chokepoint from a synchronous context.
///
/// B8-1: the onboarding probes (`validate_key_blocking`, `probe_ollama_blocking`)
/// previously built a bare `reqwest::blocking::Client`, bypassing the egress
/// policy entirely. `EgressClient` is async-only, and these fns run in two
/// different sync contexts: a `tokio::task::spawn_blocking` worker (onboarding)
/// AND directly inside the async `run()` future on a runtime worker thread
/// (`wayland-core auth`). Driving a nested `Runtime` from the latter would
/// panic ("Cannot start a runtime from within a runtime"). The only bridge
/// safe for **both** is to run the async request on a fresh OS thread that
/// owns its own current-thread runtime — that thread is outside any ambient
/// runtime, so `block_on` is always legal.
///
/// `build` adds the URL, method-specific headers, and per-request timeout to
/// the `EgressClient::tool()` builder (finite request: connect + read + a
/// wall-clock cap, redirects disabled — the M-1 exfil-on-302 mitigation).
pub(crate) fn egress_get_status<F>(build: F) -> EgressGetStatus
where
    F: FnOnce(&wcore_egress::EgressClient) -> wcore_egress::EgressRequestBuilder + Send + 'static,
{
    let joined = std::thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(_) => return EgressGetStatus::Failed,
        };
        runtime.block_on(async move {
            let client = wcore_egress::EgressClient::tool();
            match build(&client).send().await {
                Ok(resp) => EgressGetStatus::Status(resp.status().as_u16()),
                Err(e) if e.is_timeout() => EgressGetStatus::Timeout,
                Err(_) => EgressGetStatus::Failed,
            }
        })
    })
    .join();
    // A panic on the worker thread (should not happen) is treated as a
    // failed request rather than propagated into the onboarding wizard.
    joined.unwrap_or(EgressGetStatus::Failed)
}

/// Perform the blocking key-validation HTTP request.
///
/// A lightweight GET against the *detected provider's* validation
/// endpoint: a `200` means the key works, a `401`/`403` means it was
/// rejected, anything else (timeout, DNS failure) is reported as a
/// network error. This is deliberately a minimal, read-only call — it
/// never spends tokens.
///
/// B8-1: routed through the `wcore_egress::EgressClient` chokepoint (see
/// [`egress_get_status`]); there is no bare `reqwest::blocking::Client` here.
pub fn validate_key_blocking(provider: Provider, key: &str) -> ValidationOutcome {
    let (url, auth) = validation_endpoint(provider, key);
    // Own the key for the worker thread; the request builder consumes it.
    let key = key.to_string();
    let status = egress_get_status(move |client| {
        let req = client.get(url).timeout(VALIDATE_TIMEOUT);
        match auth {
            AuthStyle::AnthropicHeaders => req
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
            AuthStyle::Bearer => req.header("authorization", format!("Bearer {key}")),
            // The key is already in the URL's query string (Gemini).
            AuthStyle::QueryParam => req,
        }
    });

    match status {
        EgressGetStatus::Status(code) if (200..300).contains(&code) => ValidationOutcome::Ok,
        EgressGetStatus::Status(code @ (401 | 403)) => {
            ValidationOutcome::Failed(format!("key rejected ({code})"))
        }
        EgressGetStatus::Status(code) => {
            ValidationOutcome::Failed(format!("unexpected response ({code})"))
        }
        EgressGetStatus::Timeout => {
            ValidationOutcome::Failed("timed out — check your connection".to_string())
        }
        EgressGetStatus::Failed => ValidationOutcome::Failed("network error".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizer_matches_most_specific_prefix_first() {
        // Every uniquely-prefixed provider resolves to exactly that one.
        assert_eq!(
            detect_provider("sk-ant-api03-xxx"),
            Detected::One(Provider::Anthropic)
        );
        // The headline bug: an OpenRouter key must NOT read as OpenAI.
        assert_eq!(
            detect_provider("sk-or-v1-abcdef"),
            Detected::One(Provider::OpenRouter)
        );
        assert_eq!(
            detect_provider("sk-or-abcdef"),
            Detected::One(Provider::OpenRouter)
        );
        assert_eq!(
            detect_provider("sk-proj-abcdef"),
            Detected::One(Provider::OpenAi)
        );
        // Google Gemini's `AIza…` format — previously unrecognized.
        assert_eq!(
            detect_provider("AIzaSyD-xxxxxxxxxxxxxxxxxxxx"),
            Detected::One(Provider::Gemini)
        );
        assert_eq!(detect_provider("gsk_abcdef"), Detected::One(Provider::Groq));
        assert_eq!(detect_provider("xai-abcdef"), Detected::One(Provider::Xai));
        assert_eq!(
            detect_provider("fw_abcdef"),
            Detected::One(Provider::Fireworks)
        );
        assert_eq!(
            detect_provider("pplx-abcdef"),
            Detected::One(Provider::Perplexity)
        );
        assert_eq!(
            detect_provider("csk-abcdef"),
            Detected::One(Provider::Cerebras)
        );
        // Sakana AI keys are prefixed `fish_`.
        assert_eq!(
            detect_provider("fish_f2570dfe4dac"),
            Detected::One(Provider::Sakana)
        );
    }

    #[test]
    fn recognizer_detects_deepseek_moonshot_and_openai_variants() {
        // Moonshot's `sk-kimi-` prefix — detected, not left ambiguous.
        assert_eq!(
            detect_provider("sk-kimi-abcdef"),
            Detected::One(Provider::Moonshot)
        );
        // OpenAI's other key shapes beyond `sk-proj-`.
        assert_eq!(
            detect_provider("sk-svcacct-abcdef"),
            Detected::One(Provider::OpenAi)
        );
        assert_eq!(
            detect_provider("sk-admin-abcdef"),
            Detected::One(Provider::OpenAi)
        );
        // DeepSeek: `sk-` + exactly 32 hex chars — detected by shape.
        assert_eq!(
            detect_provider("sk-0123456789abcdef0123456789abcdef"),
            Detected::One(Provider::DeepSeek)
        );
        // 31 hex, or 32-plus-a-non-hex, is NOT a DeepSeek key.
        assert_eq!(
            detect_provider("sk-0123456789abcdef0123456789abcde"),
            Detected::Ambiguous
        );
        assert_eq!(
            detect_provider("sk-0123456789abcdef0123456789abcdefz"),
            Detected::Ambiguous
        );
    }

    #[test]
    fn recognizer_flags_bare_sk_as_ambiguous_not_a_guess() {
        // A plain `sk-` key matching no specific shape (not 32-hex
        // DeepSeek, not `sk-kimi-` / `sk-proj-`) — must NOT be guessed.
        assert_eq!(detect_provider("sk-abcdef123456"), Detected::Ambiguous);
        assert_eq!(detect_provider("sk-xxxxxxxxxxxx"), Detected::Ambiguous);
    }

    #[test]
    fn recognizer_reports_unknown_for_unrecognized_prefixes() {
        assert_eq!(detect_provider("gibberish"), Detected::Unknown);
        assert_eq!(detect_provider(""), Detected::Unknown);
        assert_eq!(detect_provider("not-a-key"), Detected::Unknown);
    }

    #[test]
    fn recognizer_trims_surrounding_whitespace() {
        assert_eq!(
            detect_provider("  sk-or-v1-padded  "),
            Detected::One(Provider::OpenRouter)
        );
    }

    #[test]
    fn validation_endpoints_are_per_provider() {
        // The detected provider — not always OpenAI — picks the endpoint.
        let (url, _) = validation_endpoint(Provider::OpenRouter, "k");
        assert_eq!(url, "https://openrouter.ai/api/v1/key");
        let (url, _) = validation_endpoint(Provider::OpenAi, "k");
        assert_eq!(url, "https://api.openai.com/v1/models");
        let (url, _) = validation_endpoint(Provider::Anthropic, "k");
        assert_eq!(url, "https://api.anthropic.com/v1/models");
        let (url, _) = validation_endpoint(Provider::Groq, "k");
        assert_eq!(url, "https://api.groq.com/openai/v1/models");
        let (url, _) = validation_endpoint(Provider::Xai, "k");
        assert_eq!(url, "https://api.x.ai/v1/models");
        let (url, _) = validation_endpoint(Provider::Mistral, "k");
        assert_eq!(url, "https://api.mistral.ai/v1/models");
        let (url, _) = validation_endpoint(Provider::DeepSeek, "k");
        assert_eq!(url, "https://api.deepseek.com/models");
        let (url, _) = validation_endpoint(Provider::Fireworks, "k");
        assert_eq!(url, "https://api.fireworks.ai/inference/v1/models");
        let (url, _) = validation_endpoint(Provider::Together, "k");
        assert_eq!(url, "https://api.together.xyz/v1/models");
        let (url, _) = validation_endpoint(Provider::Cerebras, "k");
        assert_eq!(url, "https://api.cerebras.ai/v1/models");
    }

    #[test]
    fn gemini_endpoint_carries_the_key_in_the_query_string() {
        let (url, auth) = validation_endpoint(Provider::Gemini, "SECRETKEY");
        assert!(
            url.starts_with("https://generativelanguage.googleapis.com/v1beta/models?key="),
            "gemini endpoint wrong: {url}"
        );
        assert!(url.ends_with("key=SECRETKEY"), "gemini key not in query");
        assert!(
            matches!(auth, AuthStyle::QueryParam),
            "gemini must use query-param auth, not a header"
        );
    }

    #[test]
    fn validation_auth_styles_match_each_provider() {
        assert!(matches!(
            validation_endpoint(Provider::Anthropic, "k").1,
            AuthStyle::AnthropicHeaders
        ));
        assert!(matches!(
            validation_endpoint(Provider::OpenAi, "k").1,
            AuthStyle::Bearer
        ));
        assert!(matches!(
            validation_endpoint(Provider::OpenRouter, "k").1,
            AuthStyle::Bearer
        ));
    }

    #[test]
    fn provider_slugs_align_with_the_engine() {
        // Every slug must be one `wcore-config::parse_builtin_provider`
        // accepts, so a written `config.toml` round-trips.
        assert_eq!(Provider::Anthropic.slug(), "anthropic");
        assert_eq!(Provider::OpenAi.slug(), "openai");
        assert_eq!(Provider::OpenRouter.slug(), "openrouter");
        assert_eq!(Provider::Gemini.slug(), "gemini");
        assert_eq!(Provider::Groq.slug(), "groq");
        assert_eq!(Provider::Xai.slug(), "xai");
        assert_eq!(Provider::DeepSeek.slug(), "deepseek");
        assert_eq!(Provider::Fireworks.slug(), "fireworks");
        assert_eq!(Provider::Together.slug(), "together");
        assert_eq!(Provider::Cerebras.slug(), "cerebras");
        assert_eq!(Provider::Perplexity.slug(), "perplexity");
        assert_eq!(Provider::Moonshot.slug(), "moonshot");
    }

    #[test]
    fn from_slug_round_trips_every_provider() {
        for p in Provider::ALL {
            assert_eq!(Provider::from_slug(p.slug()), Some(p));
        }
        assert_eq!(Provider::from_slug("not-a-provider"), None);
    }

    #[test]
    fn env_scan_finds_set_keys_and_maps_them_to_providers() {
        let env = |var: &str| match var {
            "ANTHROPIC_API_KEY" => Some("sk-ant-env".to_string()),
            "OPENROUTER_API_KEY" => Some("sk-or-v1-env".to_string()),
            _ => None,
        };
        let found = scan_env_keys(env);
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .any(|e| e.var == "ANTHROPIC_API_KEY" && e.provider == Provider::Anthropic)
        );
        assert!(
            found
                .iter()
                .any(|e| e.var == "OPENROUTER_API_KEY" && e.provider == Provider::OpenRouter)
        );
    }

    #[test]
    fn env_scan_ignores_blank_values() {
        let env = |var: &str| match var {
            "GROQ_API_KEY" => Some("   ".to_string()),
            "XAI_API_KEY" => Some(String::new()),
            _ => None,
        };
        assert!(scan_env_keys(env).is_empty(), "blank env vars were kept");
    }

    #[test]
    fn env_scan_dedupes_gemini_across_both_variables() {
        // GEMINI_API_KEY and GOOGLE_API_KEY both map to Gemini — only the
        // first one set should yield an entry.
        let env = |var: &str| match var {
            "GEMINI_API_KEY" => Some("AIza-first".to_string()),
            "GOOGLE_API_KEY" => Some("AIza-second".to_string()),
            _ => None,
        };
        let found = scan_env_keys(env);
        assert_eq!(found.len(), 1, "Gemini was not de-duplicated");
        assert_eq!(found[0].provider, Provider::Gemini);
        assert_eq!(found[0].value, "AIza-first", "first-set should win");
    }

    #[test]
    fn env_scan_returns_empty_when_nothing_is_set() {
        assert!(scan_env_keys(|_| None).is_empty());
    }

    // B8-1: prove the blocking probes flow through the `wcore_egress`
    // chokepoint rather than a bare `reqwest::blocking::Client`. We stand up a
    // local wiremock server and drive `egress_get_status` (the shared bridge
    // both probes use) against it: a request that reaches the mock and comes
    // back classified by HTTP status is only possible if the call went through
    // `EgressClient::get(...).send()`. No process-global policy is installed in
    // the test binary, so `GlobalDefaultPolicy` falls back to allow-all and the
    // localhost request is permitted — exactly the seam documented in
    // `wcore_egress::policy::GlobalDefaultPolicy`.
    //
    // NOTE (coverage gap, flagged): this proves the *routing* (the request
    // traverses `EgressClient`) and the status→outcome classification. It does
    // NOT assert a `Deny` short-circuits the call, because the egress policy is
    // process-global and one-shot — installing a `DenyAll` here would poison
    // every other test in this binary that does egress. Enforcement-on-deny is
    // covered by `wcore-egress`'s own policy tests against an injected policy.
    #[cfg(feature = "remote-registry")]
    #[tokio::test]
    async fn validate_key_blocking_routes_through_egress_and_classifies_status() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // A 200 from the mock must classify as `Ok`.
        let ok_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&ok_server)
            .await;
        let ok_url = format!("{}/v1/models", ok_server.uri());
        // Drive the shared bridge directly with the mock URL + the same
        // Bearer-header shape `validate_key_blocking` builds for an OpenAI key.
        // `egress_get_status` runs the request on its own OS thread + runtime,
        // so calling it from inside this `#[tokio::test]` is safe — the bridge
        // never touches the test reactor.
        let ok_status = tokio::task::spawn_blocking(move || {
            egress_get_status(move |client| {
                client
                    .get(ok_url)
                    .timeout(VALIDATE_TIMEOUT)
                    .header("authorization", "Bearer test-key")
            })
        })
        .await
        .expect("bridge thread joins");
        assert!(
            matches!(ok_status, EgressGetStatus::Status(200)),
            "a 200 from the mock must come back as Status(200) — proving the \
             request flowed through EgressClient, not a bypass client"
        );

        // A 401 from the mock must classify as `key rejected`.
        let rejected_server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&rejected_server)
            .await;
        let rejected_url = format!("{}/v1/models", rejected_server.uri());
        let rejected_status = tokio::task::spawn_blocking(move || {
            egress_get_status(move |client| client.get(rejected_url).timeout(VALIDATE_TIMEOUT))
        })
        .await
        .expect("bridge thread joins");
        assert!(
            matches!(rejected_status, EgressGetStatus::Status(401)),
            "a 401 from the mock must come back as Status(401)"
        );
    }

    // B8-1: a transport failure (nothing listening) classifies as `Failed`,
    // which `probe_ollama_blocking` maps to "not reachable" and
    // `validate_key_blocking` maps to "network error" — preserving the prior
    // `reqwest::send().is_ok() == false` semantics through the egress bridge.
    #[cfg(feature = "remote-registry")]
    #[tokio::test]
    async fn egress_get_status_reports_failure_for_an_unreachable_host() {
        // Reserved-for-documentation TEST-NET-1 address that black-holes the
        // connection; the per-request timeout keeps the test bounded.
        let status = tokio::task::spawn_blocking(|| {
            egress_get_status(|client| {
                client
                    .get("http://192.0.2.1:9/v1/models")
                    .timeout(std::time::Duration::from_secs(2))
            })
        })
        .await
        .expect("bridge thread joins");
        assert!(
            matches!(status, EgressGetStatus::Failed | EgressGetStatus::Timeout),
            "an unreachable host must classify as Failed/Timeout, not a status"
        );
    }
}
