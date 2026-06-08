//! `SlackConfig` — TOML schema for one Slack channel instance.
//!
//! Secrets are NEVER stored here. The two `credential_handle_*` fields
//! are keys looked up in the `CredentialsStore` at `start()` time so
//! the bot token + signing secret only ever live in memory (or the
//! OS keychain).

use serde::{Deserialize, Serialize};

/// Default Slack Web API base. Tests inject a mockito URL.
pub const DEFAULT_API_BASE: &str = "https://slack.com";

/// Default attempts (including the initial request) before giving up.
pub const DEFAULT_MAX_RETRY_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SlackConfig {
    /// Human-readable workspace label — used in logs only.
    pub workspace_name: String,

    /// Fallback Slack channel/conversation ID used when an
    /// `OutgoingMessage::conversation_id` arrives empty.
    #[serde(default)]
    pub default_channel_id: String,

    /// CredentialsStore key for the bot user OAuth token (xoxb-...).
    pub credential_handle_bot_token: String,

    /// CredentialsStore key for the Slack app signing secret.
    pub credential_handle_signing_secret: String,

    /// Override the Web API base URL — tests point this at mockito.
    #[serde(default = "default_api_base")]
    pub api_base_url: String,

    /// Number of attempts (including the first) for transient failures.
    #[serde(default = "default_max_retry_attempts")]
    pub max_retry_attempts: u32,
}

fn default_api_base() -> String {
    DEFAULT_API_BASE.to_string()
}

fn default_max_retry_attempts() -> u32 {
    DEFAULT_MAX_RETRY_ATTEMPTS
}

impl SlackConfig {
    /// Convenience builder for tests.
    pub fn new_for_test(api_base: impl Into<String>) -> Self {
        Self {
            workspace_name: "test".to_string(),
            default_channel_id: String::new(),
            credential_handle_bot_token: "slack.test.bot_token".to_string(),
            credential_handle_signing_secret: "slack.test.signing_secret".to_string(),
            api_base_url: api_base.into(),
            max_retry_attempts: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_toml() {
        let body = r#"
workspace_name = "acme"
default_channel_id = "C0123"
credential_handle_bot_token = "slack.acme.bot_token"
credential_handle_signing_secret = "slack.acme.signing_secret"
api_base_url = "https://slack.com"
max_retry_attempts = 5
"#;
        let cfg: SlackConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.workspace_name, "acme");
        assert_eq!(cfg.default_channel_id, "C0123");
        assert_eq!(cfg.credential_handle_bot_token, "slack.acme.bot_token");
        assert_eq!(cfg.api_base_url, "https://slack.com");
        assert_eq!(cfg.max_retry_attempts, 5);

        let re = toml::to_string(&cfg).unwrap();
        let cfg2: SlackConfig = toml::from_str(&re).unwrap();
        assert_eq!(cfg, cfg2);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let body = r#"
workspace_name = "acme"
credential_handle_bot_token = "k1"
credential_handle_signing_secret = "k2"
"#;
        let cfg: SlackConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.api_base_url, DEFAULT_API_BASE);
        assert_eq!(cfg.max_retry_attempts, DEFAULT_MAX_RETRY_ATTEMPTS);
        assert!(cfg.default_channel_id.is_empty());
    }

    #[test]
    fn deny_unknown_fields_rejects_extras() {
        let body = r#"
workspace_name = "acme"
credential_handle_bot_token = "k1"
credential_handle_signing_secret = "k2"
bogus_field = "nope"
"#;
        let err = toml::from_str::<SlackConfig>(body).unwrap_err();
        assert!(
            err.to_string().contains("bogus_field") || err.to_string().contains("unknown"),
            "expected deny_unknown_fields rejection, got {err}"
        );
    }
}
