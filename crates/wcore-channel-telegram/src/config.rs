//! `TelegramConfig` — per-channel options parsed from the `options`
//! table of a `ChannelConfig` TOML file.
//!
//! The bot token itself is NEVER stored in this struct. It lives in
//! the OS keychain (via `wcore-config::credentials`) and is fetched at
//! `start()` time using `credential_handle` as the lookup key.

use serde::{Deserialize, Serialize};

/// Per-channel Telegram config. Parsed from the `[options]` table of
/// `~/.wayland/channels/<name>.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    /// Credentials-store key for the bot token (e.g. `"telegram.acme.bot_token"`).
    pub credential_handle: String,

    /// Optional allow-list of chat IDs (as strings — Telegram chat_ids
    /// fit in i64 but stringify so negative supergroup ids round-trip).
    /// When non-empty, inbound updates whose `chat.id` is not in this
    /// list are dropped at the long-poll layer.
    #[serde(default)]
    pub allowed_chat_ids: Vec<String>,

    /// Long-poll wait seconds passed to `getUpdates?timeout=`. Capped at
    /// 120 (Telegram's own ceiling); 0 means short-poll.
    #[serde(default = "default_long_poll_timeout_secs")]
    pub long_poll_timeout_secs: u32,

    /// Default `parse_mode` for outbound messages.
    #[serde(default = "default_parse_mode")]
    pub parse_mode: ParseMode,
}

fn default_long_poll_timeout_secs() -> u32 {
    30
}

fn default_parse_mode() -> ParseMode {
    ParseMode::MarkdownV2
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum ParseMode {
    MarkdownV2,
    #[serde(rename = "HTML")]
    Html,
    Markdown,
}

impl ParseMode {
    pub fn as_api_str(&self) -> &'static str {
        match self {
            ParseMode::MarkdownV2 => "MarkdownV2",
            ParseMode::Html => "HTML",
            ParseMode::Markdown => "Markdown",
        }
    }
}

/// Escape every character Telegram reserves under MarkdownV2 by prefixing
/// it with a backslash.
///
/// Telegram's MarkdownV2 spec requires these characters be escaped in ALL
/// text (not just inside entities):
/// `_ * [ ] ( ) ~ ` > # + - = | { } . !`
///
/// v1 semantics: we escape the FULL message text, so the body is always
/// delivered as valid literal text and Telegram never returns
/// `400 Bad Request: can't parse entities`. The tradeoff is that any
/// model-emitted Markdown formatting (`*bold*`, `[label](url)`) is rendered
/// literally rather than interpreted. That is the correct, safe default for
/// v1 — never 400. Formatting fidelity via selective escaping (escaping only
/// the reserved characters that are NOT part of intended markup) is a future
/// enhancement.
///
/// This is MarkdownV2-specific. It must NOT be applied to `HTML` parse mode
/// (HTML has its own escaping rules — `< > &` — out of scope here) nor to the
/// legacy `Markdown` parse mode (which has a different, smaller reserved set:
/// `_ * ` [`). Callers gate on `parse_mode == MarkdownV2`.
pub fn escape_markdown_v2(s: &str) -> String {
    // Reserved set per https://core.telegram.org/bots/api#markdownv2-style
    const RESERVED: &[char] = &[
        '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
    ];
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if RESERVED.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Escape the three characters Telegram reserves under `HTML` parse mode so
/// agent text is delivered as literal content rather than triggering a
/// `400 Bad Request: can't parse entities` (which silently drops the reply).
///
/// Per <https://core.telegram.org/bots/api#html-style>, only `<`, `>`, and `&`
/// must be escaped. `&` is replaced FIRST — otherwise the `&` introduced by
/// `&lt;` / `&gt;` would be double-escaped into `&amp;lt;`.
///
/// As with [`escape_markdown_v2`], this is the safe v1 default: any
/// model-emitted HTML markup (`<b>`, `<a href>`) renders literally rather than
/// being interpreted. Never 400 beats occasional formatting fidelity.
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_uses_defaults() {
        let cfg: TelegramConfig = toml::from_str(
            r#"
credential_handle = "telegram.acme.bot_token"
"#,
        )
        .unwrap();
        assert_eq!(cfg.credential_handle, "telegram.acme.bot_token");
        assert!(cfg.allowed_chat_ids.is_empty());
        assert_eq!(cfg.long_poll_timeout_secs, 30);
        assert_eq!(cfg.parse_mode, ParseMode::MarkdownV2);
    }

    #[test]
    fn full_config_round_trips() {
        let src = r#"
credential_handle = "telegram.acme.bot_token"
allowed_chat_ids = ["123", "-100456"]
long_poll_timeout_secs = 60
parse_mode = "HTML"
"#;
        let cfg: TelegramConfig = toml::from_str(src).unwrap();
        assert_eq!(cfg.allowed_chat_ids, vec!["123", "-100456"]);
        assert_eq!(cfg.long_poll_timeout_secs, 60);
        assert_eq!(cfg.parse_mode, ParseMode::Html);
    }

    #[test]
    fn unknown_field_rejected() {
        let src = r#"
credential_handle = "x"
unknown = "boom"
"#;
        let err = toml::from_str::<TelegramConfig>(src).expect_err("expected deny_unknown_fields");
        assert!(
            err.to_string().contains("unknown"),
            "error should mention unknown field, got: {err}"
        );
    }

    #[test]
    fn escape_markdown_v2_escapes_every_reserved_char() {
        for ch in [
            '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.',
            '!',
        ] {
            let input = ch.to_string();
            let escaped = escape_markdown_v2(&input);
            assert_eq!(
                escaped,
                format!("\\{ch}"),
                "reserved char {ch:?} should be backslash-escaped"
            );
        }
    }

    #[test]
    fn escape_markdown_v2_realistic_reply() {
        // `Hello! I'm here. (ready)` -> `!`, `.`, `(`, `)` get escaped.
        // The apostrophe is NOT reserved and stays as-is.
        let input = "Hello! I'm here. (ready)";
        let escaped = escape_markdown_v2(input);
        assert_eq!(escaped, "Hello\\! I'm here\\. \\(ready\\)");
    }

    #[test]
    fn escape_markdown_v2_plain_alphanumeric_unchanged() {
        let input = "abc 123 XYZ";
        assert_eq!(escape_markdown_v2(input), input);
    }

    #[test]
    fn escape_markdown_v2_backslash_prefix_is_correct() {
        // A literal backslash is itself NOT in the reserved set, so it is
        // left untouched; the reserved char following it still gets its own
        // backslash. Verifies we only prepend `\` to reserved chars.
        let escaped = escape_markdown_v2("a\\b.c");
        assert_eq!(escaped, "a\\b\\.c");
    }

    #[test]
    fn missing_required_credential_handle_errors() {
        let err = toml::from_str::<TelegramConfig>("").expect_err("expected missing required");
        assert!(
            err.to_string().contains("credential_handle"),
            "error should mention credential_handle, got: {err}"
        );
    }
}
