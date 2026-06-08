//! `SignalConfig` — per-channel options parsed from the `options`
//! table of a `ChannelConfig` TOML file.
//!
//! The adapter spawns `signal-cli -a <account> jsonRpc` as a child
//! process and exchanges JSON-RPC frames over stdio. No secrets live
//! in this struct; `signal-cli` manages its own state directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignalConfig {
    /// Path to the `signal-cli` executable. Defaults to looking up
    /// `signal-cli` on `$PATH`.
    #[serde(default = "default_signal_cli_path")]
    pub signal_cli_path: PathBuf,

    /// Signal account identifier (typically the registered phone
    /// number, e.g. `+15551234567`). Passed to `signal-cli -a`.
    pub account: String,

    /// Per-request timeout (seconds) for outbound send_message
    /// JSON-RPC round-trips.
    #[serde(default = "default_send_timeout_secs")]
    pub send_timeout_secs: u64,
}

fn default_signal_cli_path() -> PathBuf {
    PathBuf::from("signal-cli")
}

fn default_send_timeout_secs() -> u64 {
    10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_uses_defaults() {
        let cfg: SignalConfig = toml::from_str(
            r#"
account = "+15551234567"
"#,
        )
        .unwrap();
        assert_eq!(cfg.account, "+15551234567");
        assert_eq!(cfg.signal_cli_path, PathBuf::from("signal-cli"));
        assert_eq!(cfg.send_timeout_secs, 10);
    }

    #[test]
    fn full_config_round_trips() {
        let src = r#"
signal_cli_path = "/usr/local/bin/signal-cli"
account = "+15551234567"
send_timeout_secs = 30
"#;
        let cfg: SignalConfig = toml::from_str(src).unwrap();
        assert_eq!(
            cfg.signal_cli_path,
            PathBuf::from("/usr/local/bin/signal-cli")
        );
        assert_eq!(cfg.account, "+15551234567");
        assert_eq!(cfg.send_timeout_secs, 30);
    }

    #[test]
    fn unknown_field_rejected() {
        let src = r#"
account = "+1"
unknown = "boom"
"#;
        let err = toml::from_str::<SignalConfig>(src).expect_err("expected deny_unknown_fields");
        assert!(
            err.to_string().contains("unknown"),
            "error should mention unknown field, got: {err}"
        );
    }

    #[test]
    fn missing_required_account_errors() {
        let err = toml::from_str::<SignalConfig>("").expect_err("expected missing required field");
        assert!(
            err.to_string().contains("account"),
            "error should mention `account`, got: {err}"
        );
    }
}
