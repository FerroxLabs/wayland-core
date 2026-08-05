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
/// DIRECT_MESSAGES (bit 12) — receive MESSAGE_CREATE in DM channels.
/// Discord delivers ZERO DM message events unless the connection IDENTIFYs
/// with this intent, so without it the bot is deaf to direct messages.
pub const INTENT_DIRECT_MESSAGES: u64 = 1 << 12;
/// Default intents — guild + DM message events plus message content, the
/// minimum for inbound text to arrive on both surfaces. = 37376
/// (512 | 32768 | 4096).
pub const DEFAULT_INTENTS: u64 =
    INTENT_GUILD_MESSAGES | INTENT_MESSAGE_CONTENT | INTENT_DIRECT_MESSAGES;

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

    /// Override the REST API base URL. Defaults to
    /// [`crate::DISCORD_API_BASE`] (`https://discord.com`).
    ///
    /// F24-C3-DISCORD. This exists for the same reason
    /// `TelegramConfig::api_base_url`, `SlackConfig::api_base_url`,
    /// `WhatsAppConfig::api_base_url` and `SmsConfig::api_base_url` already do.
    /// Discord was the last adapter with no config-level seam at all, and that
    /// is precisely why its inbound path stayed unmeasured across the whole of
    /// Phase 24: three separate lanes concluded a real vendor bot token was
    /// required, when what was actually missing was this field.
    ///
    /// A Rust-level seam already existed ([`crate::DiscordChannel::with_bases`])
    /// but is `#[doc(hidden)]` and reachable only from unit tests in-process.
    /// The SHIPPED BINARY is constructed by `wcore-channels-registry` through
    /// `DiscordChannel::new`, so without this field no out-of-process harness
    /// can point the real binary anywhere but `discord.com`.
    ///
    /// This is operator-owned configuration at the same trust level as
    /// `credential_handle`: whoever can write this file can already name the
    /// credential the adapter sends. It is NOT reachable from a message.
    #[serde(default = "default_api_base")]
    pub api_base_url: String,

    /// Override the Gateway WebSocket base URL. Defaults to
    /// [`crate::DISCORD_GATEWAY_BASE`] (`wss://gateway.discord.gg`).
    ///
    /// Discord needs TWO seam fields where the HTTP adapters need one, because
    /// its inbound arrives over a WebSocket rather than by polling REST.
    /// Overriding `api_base_url` alone would redirect outbound sends while
    /// leaving inbound pointed at production — the exact half-configured state
    /// that makes a fixture run look green while measuring nothing.
    #[serde(default = "default_gateway_url")]
    pub gateway_url: String,
}

fn default_intents() -> u64 {
    DEFAULT_INTENTS
}

fn default_heartbeat_grace_ms() -> u64 {
    5_000
}

fn default_api_base() -> String {
    crate::DISCORD_API_BASE.to_string()
}

fn default_gateway_url() -> String {
    crate::DISCORD_GATEWAY_BASE.to_string()
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
    fn default_intents_cover_guild_dm_and_content() {
        // Regression: DIRECT_MESSAGES (bit 12) was missing, so the default bot
        // received no DM events. All three surfaces must be present by default.
        assert_ne!(DEFAULT_INTENTS & INTENT_GUILD_MESSAGES, 0);
        assert_ne!(DEFAULT_INTENTS & INTENT_DIRECT_MESSAGES, 0);
        assert_ne!(DEFAULT_INTENTS & INTENT_MESSAGE_CONTENT, 0);
        assert_eq!(DEFAULT_INTENTS, 37376);
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

    // -----------------------------------------------------------------
    // F24-C3-DISCORD — the config-level base-URL seam.
    //
    // The CONTROL test is the load-bearing one: it proves that adding the
    // seam did not quietly move the production default. A seam that
    // redirects an operator's traffic away from discord.com by accident is
    // strictly worse than no seam.
    // -----------------------------------------------------------------

    #[test]
    fn control_absent_keys_still_reach_production_discord() {
        // CONTROL: no api_base_url, no gateway_url anywhere in the source.
        let src = r#"
credential_handle = "discord.acme.bot_token"
"#;
        assert!(
            !src.contains("api_base_url") && !src.contains("gateway_url"),
            "control precondition: the source must not name either key"
        );
        let cfg: DiscordConfig = toml::from_str(src).unwrap();
        assert_eq!(
            cfg.api_base_url,
            crate::DISCORD_API_BASE,
            "a config that does not name a REST base must reach production Discord"
        );
        assert_eq!(
            cfg.gateway_url,
            crate::DISCORD_GATEWAY_BASE,
            "a config that does not name a gateway must reach production Discord"
        );
        assert_eq!(cfg.api_base_url, "https://discord.com");
        assert_eq!(cfg.gateway_url, "wss://gateway.discord.gg");
    }

    #[test]
    fn backcompat_a_preexisting_full_config_still_parses() {
        // deny_unknown_fields makes ADDING a field safe (a missing field is
        // not an unknown field), but that is an assertion about serde that
        // this crate should hold itself to rather than assume. This is a
        // byte-for-byte pre-seam config: every key that existed before.
        let pre_seam = r#"
credential_handle = "discord.acme.bot_token"
allowed_channel_ids = ["111", "222"]
intents = 513
heartbeat_grace_ms = 8000
"#;
        let cfg: DiscordConfig =
            toml::from_str(pre_seam).expect("a pre-seam config must still parse unchanged");
        assert_eq!(cfg.credential_handle, "discord.acme.bot_token");
        assert_eq!(cfg.allowed_channel_ids, vec!["111", "222"]);
        assert_eq!(cfg.intents, 513);
        assert_eq!(cfg.heartbeat_grace_ms, 8_000);
        // ...and it still points at production.
        assert_eq!(cfg.api_base_url, crate::DISCORD_API_BASE);
        assert_eq!(cfg.gateway_url, crate::DISCORD_GATEWAY_BASE);
    }

    #[test]
    fn both_bases_are_independently_overridable() {
        let src = r#"
credential_handle = "discord.acme.bot_token"
api_base_url = "http://127.0.0.1:18211"
gateway_url = "ws://127.0.0.1:18212"
"#;
        let cfg: DiscordConfig = toml::from_str(src).unwrap();
        assert_eq!(cfg.api_base_url, "http://127.0.0.1:18211");
        assert_eq!(cfg.gateway_url, "ws://127.0.0.1:18212");

        // Overriding ONE must not drag the other along. Half-redirected is
        // the state where outbound hits a fixture and inbound silently
        // stays on production.
        let rest_only: DiscordConfig = toml::from_str(
            r#"
credential_handle = "x"
api_base_url = "http://127.0.0.1:18211"
"#,
        )
        .unwrap();
        assert_eq!(rest_only.api_base_url, "http://127.0.0.1:18211");
        assert_eq!(rest_only.gateway_url, crate::DISCORD_GATEWAY_BASE);

        let gw_only: DiscordConfig = toml::from_str(
            r#"
credential_handle = "x"
gateway_url = "ws://127.0.0.1:18212"
"#,
        )
        .unwrap();
        assert_eq!(gw_only.api_base_url, crate::DISCORD_API_BASE);
        assert_eq!(gw_only.gateway_url, "ws://127.0.0.1:18212");
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
