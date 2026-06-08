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
    fn missing_required_credential_handle_errors() {
        let err = toml::from_str::<TelegramConfig>("").expect_err("expected missing required");
        assert!(
            err.to_string().contains("credential_handle"),
            "error should mention credential_handle, got: {err}"
        );
    }
}
