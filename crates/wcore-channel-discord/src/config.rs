//! `DiscordConfig` — per-channel options parsed from the `options`
//! table of a `ChannelConfig` TOML file.
//!
//! The bot token itself is NEVER stored in this struct. It lives in
//! the OS keychain (via `wcore-config::credentials`) and is fetched at
//! `start()` time using `credential_handle` as the lookup key.

use serde::{Deserialize, Serialize};

/// GUILD_MESSAGES (bit 9) — receive messages in guild text channels.
pub const INTENT_GUILD_MESSAGES: u64 = 1 << 9;
/// MESSAGE_CONTENT (bit 15) — receive the `content` field of every
/// message (privileged intent; must be enabled in the Discord
/// developer portal for the bot).
pub const INTENT_MESSAGE_CONTENT: u64 = 1 << 15;
/// Default intents — both message intents required for inbound message
/// text to arrive. = 33792.
pub const DEFAULT_INTENTS: u64 = INTENT_GUILD_MESSAGES | INTENT_MESSAGE_CONTENT;

/// Per-channel Discord config. Parsed from the `[options]` table of
/// `~/.wayland/channels/<name>.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiscordConfig {
    /// Credentials-store key for the bot token (e.g. `"discord.acme.bot_token"`).
    pub credential_handle: String,

    /// Optional allow-list of Discord channel IDs (snowflake strings).
    /// When non-empty, inbound MESSAGE_CREATE events whose `channel_id`
    /// is not in this list are dropped at the gateway layer.
    #[serde(default)]
    pub allowed_channel_ids: Vec<String>,

    /// Gateway intents bitmask. Defaults to GUILD_MESSAGES | MESSAGE_CONTENT.
    #[serde(default = "default_intents")]
    pub intents: u64,

    /// Grace window (ms) after sending a heartbeat for the
    /// HEARTBEAT_ACK to arrive before the connection is treated as dead.
    #[serde(default = "default_heartbeat_grace_ms")]
    pub heartbeat_grace_ms: u64,
}

fn default_intents() -> u64 {
    DEFAULT_INTENTS
}

fn default_heartbeat_grace_ms() -> u64 {
    5_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_uses_defaults() {
        let cfg: DiscordConfig = toml::from_str(
            r#"
credential_handle = "discord.acme.bot_token"
"#,
        )
        .unwrap();
        assert_eq!(cfg.credential_handle, "discord.acme.bot_token");
        assert!(cfg.allowed_channel_ids.is_empty());
        assert_eq!(cfg.intents, DEFAULT_INTENTS);
        assert_eq!(cfg.heartbeat_grace_ms, 5_000);
    }

    #[test]
    fn full_config_round_trips() {
        let src = r#"
credential_handle = "discord.acme.bot_token"
allowed_channel_ids = ["111", "222"]
intents = 513
heartbeat_grace_ms = 10000
"#;
        let cfg: DiscordConfig = toml::from_str(src).unwrap();
        assert_eq!(cfg.allowed_channel_ids, vec!["111", "222"]);
        assert_eq!(cfg.intents, 513);
        assert_eq!(cfg.heartbeat_grace_ms, 10_000);
    }

    #[test]
    fn unknown_field_rejected() {
        let src = r#"
credential_handle = "x"
unknown = "boom"
"#;
        let err = toml::from_str::<DiscordConfig>(src).expect_err("expected deny_unknown_fields");
        assert!(
            err.to_string().contains("unknown"),
            "error should mention unknown field, got: {err}"
        );
    }

    #[test]
    fn missing_required_credential_handle_errors() {
        let err = toml::from_str::<DiscordConfig>("").expect_err("expected missing required");
        assert!(
            err.to_string().contains("credential_handle"),
            "error should mention credential_handle, got: {err}"
        );
    }
}
