//! `EmailConfig` — per-channel options parsed from the `options` table
//! of a `ChannelConfig` TOML file.
//!
//! Credentials (SMTP/IMAP usernames + passwords) are NEVER stored in
//! this struct. They live in the OS keychain (via
//! `wcore-config::credentials`) and are fetched at `start()` time using
//! the `*_credential_handle` keys.

use serde::{Deserialize, Serialize};

/// Transport security for one mail connection.
///
/// Before this existed the connector hardcoded exactly one mode per path:
/// IMAP always opened with implicit TLS (`imap::connect`, a ClientHello at
/// byte 0) and SMTP always demanded STARTTLS (`Tls::Required`). Neither was
/// configurable, so a STARTTLS-only IMAP server was unreachable — the client
/// spoke TLS onto a plaintext port and the server reset every attempt — and a
/// loopback development or test relay was unreachable on both paths.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailSecurity {
    /// Infer the mode: see [`MailSecurity::resolve`].
    #[default]
    Auto,
    /// Wrap the socket in TLS before the greeting (IMAPS 993 / SMTPS 465).
    Implicit,
    /// Connect in the clear, then upgrade with `STARTTLS` before authenticating.
    Starttls,
    /// No TLS at all. Refused at connect time for a non-loopback host.
    Plaintext,
}

impl MailSecurity {
    /// Resolve [`MailSecurity::Auto`] to a concrete mode.
    ///
    /// * A loopback host gets `Plaintext`. The bytes never leave the machine,
    ///   so there is no network to protect them from, and requiring TLS there
    ///   only makes a local relay unreachable.
    /// * Otherwise the conventional implicit-TLS port (`implicit_port`: 993
    ///   for IMAP, 465 for SMTP) gets `Implicit`, and every other port gets
    ///   `Starttls`. Both keep certificate and hostname verification on.
    ///
    /// Any explicit mode is returned unchanged — `Auto` is the only value this
    /// function decides anything about.
    pub fn resolve(self, host: &str, port: u16, implicit_port: u16) -> MailSecurity {
        match self {
            Self::Auto if is_loopback_host(host) => Self::Plaintext,
            Self::Auto if port == implicit_port => Self::Implicit,
            Self::Auto => Self::Starttls,
            other => other,
        }
    }
}

/// Whether `host` names the local machine, so an unencrypted connection to it
/// never reaches a network.
///
/// Accepts the literal `localhost`, any loopback IPv4 (`127.0.0.0/8`) or IPv6
/// (`::1`) address, and the bracketed IPv6 form. A name that merely *resolves*
/// to loopback is NOT accepted: DNS is attacker-influenceable and this
/// predicate gates whether credentials may cross the wire in the clear, so it
/// is deliberately syntactic and fails closed on anything it cannot prove.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim();
    let h = h.strip_prefix('[').unwrap_or(h);
    let h = h.strip_suffix(']').unwrap_or(h);
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    h.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

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

    /// Optional path to a PEM file holding one or more extra TLS trust
    /// anchors for the SMTP connection.
    ///
    /// Needed because the SMTP path is built on `lettre` +
    /// `tokio1-rustls-tls`, whose default certificate store is the
    /// **compiled-in** `webpki-roots` Mozilla bundle. That path reads no
    /// platform trust store on any OS, so a relay with a private or
    /// self-signed chain — a corporate MTA, or a test relay — is otherwise
    /// unreachable, and neither `SSL_CERT_FILE` nor adding the CA to the
    /// system keychain changes that. (Contrast the IMAP path, which uses
    /// `native-tls` and therefore *does* follow the platform store.)
    ///
    /// This ADDS anchors; it never disables verification. There is
    /// deliberately no option to accept invalid certificates or hostnames —
    /// trusting a named CA and switching verification off are different
    /// decisions, and only the first one is offered here.
    #[serde(default)]
    pub tls_root_cert_path: Option<String>,

    /// Transport security. Defaults to [`MailSecurity::Auto`], which picks
    /// implicit TLS on port 465, plaintext to a loopback host, and STARTTLS
    /// everywhere else.
    #[serde(default)]
    pub security: MailSecurity,
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

    /// Transport security. Defaults to [`MailSecurity::Auto`], which picks
    /// implicit TLS on port 993, plaintext to a loopback host, and STARTTLS
    /// everywhere else.
    #[serde(default)]
    pub security: MailSecurity,
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
    fn security_defaults_to_auto_and_is_absent_from_minimal_toml() {
        let cfg: EmailConfig = toml::from_str(
            r#"
from_address = "bot@acme.com"
[smtp]
host = "smtp.acme.com"
user_credential_handle = "u"
password_credential_handle = "p"
"#,
        )
        .unwrap();
        assert_eq!(cfg.smtp.security, MailSecurity::Auto);
    }

    #[test]
    fn security_round_trips_every_explicit_mode() {
        let cfg: EmailConfig = toml::from_str(
            r#"
from_address = "bot@acme.com"
[smtp]
host = "smtp.acme.com"
port = 465
user_credential_handle = "u"
password_credential_handle = "p"
security = "implicit"
[imap]
host = "imap.acme.com"
port = 143
user_credential_handle = "u"
password_credential_handle = "p"
security = "starttls"
"#,
        )
        .unwrap();
        assert_eq!(cfg.smtp.security, MailSecurity::Implicit);
        assert_eq!(cfg.imap.unwrap().security, MailSecurity::Starttls);
    }

    /// `Auto` must not put TLS on the wire toward the local machine. This is
    /// the resolution the B-3 corpus row depends on: its hermetic mail host
    /// listens in the clear on 127.0.0.1 on an ephemeral port.
    #[test]
    fn auto_resolves_loopback_to_plaintext_on_any_port() {
        for host in ["127.0.0.1", "localhost", "::1", "[::1]", "127.0.0.53"] {
            for port in [143u16, 993, 25, 465, 587, 44293] {
                assert_eq!(
                    MailSecurity::Auto.resolve(host, port, 993),
                    MailSecurity::Plaintext,
                    "host {host} port {port} must resolve to plaintext"
                );
            }
        }
    }

    /// Off the loopback, `Auto` keeps TLS mandatory: the conventional
    /// implicit port stays implicit and EVERY other port gets STARTTLS. No
    /// remote arm may ever resolve to plaintext.
    #[test]
    fn auto_never_resolves_a_remote_host_to_plaintext() {
        assert_eq!(
            MailSecurity::Auto.resolve("imap.acme.com", 993, 993),
            MailSecurity::Implicit
        );
        assert_eq!(
            MailSecurity::Auto.resolve("imap.acme.com", 143, 993),
            MailSecurity::Starttls
        );
        assert_eq!(
            MailSecurity::Auto.resolve("smtp.acme.com", 465, 465),
            MailSecurity::Implicit
        );
        for port in [25u16, 143, 587, 993, 2525, 44293] {
            assert_ne!(
                MailSecurity::Auto.resolve("mail.acme.com", port, 465),
                MailSecurity::Plaintext,
                "port {port} on a remote host must never resolve to plaintext"
            );
        }
    }

    #[test]
    fn explicit_modes_are_returned_unchanged() {
        for mode in [
            MailSecurity::Implicit,
            MailSecurity::Starttls,
            MailSecurity::Plaintext,
        ] {
            assert_eq!(mode.resolve("imap.acme.com", 993, 993), mode);
            assert_eq!(mode.resolve("127.0.0.1", 143, 993), mode);
        }
    }

    #[test]
    fn loopback_predicate_accepts_only_provable_loopback() {
        for h in [
            "127.0.0.1",
            "127.1.2.3",
            "localhost",
            "LocalHost",
            "::1",
            "[::1]",
        ] {
            assert!(is_loopback_host(h), "{h} is loopback");
        }
        // Not provably loopback. `localhost.evil.com` and `127.0.0.1.evil.com`
        // are the classic rebinding shapes; a bare hostname is unknowable
        // without DNS, which this predicate deliberately does not consult.
        for h in [
            "localhost.evil.com",
            "127.0.0.1.evil.com",
            "imap.acme.com",
            "0.0.0.0",
            "10.0.0.1",
            "",
        ] {
            assert!(!is_loopback_host(h), "{h} must not be treated as loopback");
        }
    }

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
