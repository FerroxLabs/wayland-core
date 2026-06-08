//! `EmailConfig` — per-channel options parsed from the `options` table
//! of a `ChannelConfig` TOML file.
//!
//! Credentials (SMTP/IMAP usernames + passwords) are NEVER stored in
//! this struct. They live in the OS keychain (via
//! `wcore-config::credentials`) and are fetched at `start()` time using
//! the `*_credential_handle` keys.

use serde::{Deserialize, Serialize};

/// Per-channel email config. Parsed from the `[options]` table of
/// `~/.wayland/channels/<name>.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmailConfig {
    /// RFC 5322 mailbox address used as the `From:` header on outbound
    /// messages.
    pub from_address: String,

    /// SMTP outbound config. Required — channels with no outbound path
    /// don't make sense.
    pub smtp: SmtpConfig,

    /// Optional IMAP inbound config. When absent, the channel is
    /// outbound-only (no poll task is spawned, `poll_events` returns
    /// any queued connection-state events only).
    #[serde(default)]
    pub imap: Option<ImapConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SmtpConfig {
    pub host: String,
    #[serde(default = "default_smtp_port")]
    pub port: u16,
    /// Credentials-store key for the SMTP username.
    pub user_credential_handle: String,
    /// Credentials-store key for the SMTP password.
    pub password_credential_handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImapConfig {
    pub host: String,
    #[serde(default = "default_imap_port")]
    pub port: u16,
    pub user_credential_handle: String,
    pub password_credential_handle: String,
    #[serde(default = "default_mailbox")]
    pub mailbox: String,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u32,

    /// Optional allow-list of sender email addresses (case-insensitive,
    /// compared against the bare `addr-spec` extracted from the inbound
    /// `From:` header). When non-empty, inbound messages whose `From:`
    /// address is not on this list are dropped before they reach the
    /// event stream.
    ///
    /// SECURITY: the `From:` header is **not** an authenticated principal.
    /// SMTP does not bind the envelope/header sender to the connecting
    /// party, and this crate performs no SPF/DKIM/DMARC verification, so
    /// `From:` is trivially spoofable by anyone who can deliver mail to the
    /// connected mailbox. This allow-list is a coarse delivery-side filter,
    /// not authentication. For a meaningful trust boundary, point the
    /// channel at a mailbox whose provider enforces inbound DMARC (so
    /// forged `From:` is rejected upstream), and never treat the resulting
    /// `author` as a verified identity downstream.
    #[serde(default)]
    pub allowed_senders: Vec<String>,
}

fn default_smtp_port() -> u16 {
    587
}
fn default_imap_port() -> u16 {
    993
}
fn default_mailbox() -> String {
    "INBOX".to_string()
}
fn default_poll_interval_secs() -> u32 {
    30
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_outbound_only_config_uses_defaults() {
        let cfg: EmailConfig = toml::from_str(
            r#"
from_address = "bot@acme.com"
[smtp]
host = "smtp.acme.com"
user_credential_handle = "email.acme.smtp_user"
password_credential_handle = "email.acme.smtp_pass"
"#,
        )
        .unwrap();
        assert_eq!(cfg.from_address, "bot@acme.com");
        assert_eq!(cfg.smtp.port, 587);
        assert!(cfg.imap.is_none());
    }

    #[test]
    fn full_config_round_trips() {
        let src = r#"
from_address = "bot@acme.com"

[smtp]
host = "smtp.acme.com"
port = 465
user_credential_handle = "email.acme.smtp_user"
password_credential_handle = "email.acme.smtp_pass"

[imap]
host = "imap.acme.com"
port = 993
user_credential_handle = "email.acme.imap_user"
password_credential_handle = "email.acme.imap_pass"
mailbox = "INBOX"
poll_interval_secs = 60
"#;
        let cfg: EmailConfig = toml::from_str(src).unwrap();
        assert_eq!(cfg.smtp.port, 465);
        let imap = cfg.imap.expect("imap section present");
        assert_eq!(imap.host, "imap.acme.com");
        assert_eq!(imap.mailbox, "INBOX");
        assert_eq!(imap.poll_interval_secs, 60);
        // Allow-list defaults to empty (no filtering) when omitted.
        assert!(imap.allowed_senders.is_empty());
    }

    #[test]
    fn imap_allowed_senders_parses() {
        let src = r#"
from_address = "bot@acme.com"
[smtp]
host = "smtp.acme.com"
user_credential_handle = "u"
password_credential_handle = "p"
[imap]
host = "imap.acme.com"
user_credential_handle = "iu"
password_credential_handle = "ip"
allowed_senders = ["Alice@Acme.com", "ops@acme.com"]
"#;
        let cfg: EmailConfig = toml::from_str(src).unwrap();
        let imap = cfg.imap.expect("imap section present");
        assert_eq!(imap.allowed_senders, vec!["Alice@Acme.com", "ops@acme.com"]);
    }

    #[test]
    fn unknown_field_rejected() {
        let src = r#"
from_address = "bot@acme.com"
unknown = "boom"
[smtp]
host = "s"
user_credential_handle = "u"
password_credential_handle = "p"
"#;
        let err = toml::from_str::<EmailConfig>(src).expect_err("expected deny_unknown_fields");
        assert!(
            err.to_string().contains("unknown"),
            "error should mention unknown field, got: {err}"
        );
    }
}
