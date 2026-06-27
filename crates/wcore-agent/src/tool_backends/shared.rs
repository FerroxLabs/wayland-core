//! Shared helpers for `tool_backends/*` modules.
//!
//! Created in v0.9.0 Wave-1 B0 prep. Houses the canonical env-var
//! resolver (R-H2) and the `urlencode` helper used by multiple search
//! backends. Cross-backend imports go through this module.

use wcore_config::config::{Config, ProviderType};
use wcore_providers::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL;

/// Canonical env-var resolver. Returns `Some(key)` only when the env
/// var is set **and** its value is non-empty (closes R-H2: empty-string
/// `OPENAI_API_KEY=""` should NOT count as "configured"). Every new
/// Wave-1 backend resolves credentials through this helper so the
/// "key set but empty" pathology is handled in one place.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_env_key_returns_none_for_unset() {
        // Use a name unlikely to be set.
        // SAFETY: tests run sequentially with `serial_test` per-suite, but
        // this helper does not mutate process env, so a stray-set is fine.
        let v = std::env::var("WAYLAND_TEST_DEFINITELY_UNSET_12345").ok();
        assert!(v.is_none() || v.as_deref() == Some(""));
        assert!(read_env_key("WAYLAND_TEST_DEFINITELY_UNSET_12345").is_none());
    }

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
}
