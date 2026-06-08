//! `IMessageConfig` — per-channel iMessage options.
//!
//! No credentials needed in the config. Access is OS-gated: Full Disk Access
//! (to read chat.db) and macOS Automation TCC consent (to send via osascript).

use serde::{Deserialize, Serialize};

const DEFAULT_POLL_INTERVAL_MS: u64 = 2_000;
const MIN_POLL_INTERVAL_MS: u64 = 500;
const MAX_POLL_INTERVAL_MS: u64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IMessageConfig {
    /// Polling interval in milliseconds. Clamped to [500, 60000]. Default 2000.
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,

    /// Optional allow-list of phone numbers / email addresses (case-insensitive).
    /// When non-empty, only messages from these handles are surfaced.
    #[serde(default)]
    pub allowed_handles: Vec<String>,
}

impl IMessageConfig {
    /// Return the poll interval clamped to safe bounds.
    pub fn clamped_poll_interval_ms(&self) -> u64 {
        self.poll_interval_ms
            .clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS)
    }
}

fn default_poll_interval_ms() -> u64 {
    DEFAULT_POLL_INTERVAL_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_from_empty() {
        let cfg: IMessageConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.poll_interval_ms, DEFAULT_POLL_INTERVAL_MS);
        assert!(cfg.allowed_handles.is_empty());
    }

    #[test]
    fn full_config_round_trips() {
        let raw = r#"
poll_interval_ms = 5000
allowed_handles = ["+15551234567", "user@example.com"]
"#;
        let cfg: IMessageConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.poll_interval_ms, 5_000);
        assert_eq!(
            cfg.allowed_handles,
            vec!["+15551234567", "user@example.com"]
        );
    }

    #[test]
    fn clamp_enforced() {
        let mut cfg: IMessageConfig = toml::from_str("poll_interval_ms = 1").unwrap();
        assert_eq!(cfg.clamped_poll_interval_ms(), MIN_POLL_INTERVAL_MS);
        cfg.poll_interval_ms = 999_999;
        assert_eq!(cfg.clamped_poll_interval_ms(), MAX_POLL_INTERVAL_MS);
    }
}
