//! Shared helpers for `tool_backends/*` modules.
//!
//! Created in v0.9.0 Wave-1 B0 prep. Houses the canonical env-var
//! resolver (R-H2) and the `urlencode` helper used by multiple search
//! backends. Cross-backend imports go through this module.

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
}
