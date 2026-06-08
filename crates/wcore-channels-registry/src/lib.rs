//! `wcore-channels-registry` — per-platform factory dispatch +
//! on-disk auto-registration for `wcore-channels`.
//!
//! v0.8.1 U5. The 7 channel adapters that landed in v0.8.0
//! (Slack / Telegram / Email / Discord / SMS / WhatsApp / Signal) all
//! compile and have their own test suites, but nothing wired them into
//! `ChannelManager` at engine boot — every deploy had to hand-register
//! each adapter. This crate closes that loop:
//!
//! * [`channel_factory_for`] maps a `"slack"` / `"telegram"` / … string
//!   from the on-disk `ChannelConfig` to a constructor function pointer.
//! * [`auto_register_from_user_config`] scans `~/.wayland/channels/*.toml`
//!   and registers every channel whose `platform` field maps to a known
//!   factory. Parse failures / missing factories log + skip so one bad
//!   config can't take the agent down.
//!
//! This crate is the natural meeting point because individual channel
//! crates depend on `wcore-channels` — putting the dispatch table in
//! `wcore-channels` itself would invert the dep edge and pull every
//! channel transport (reqwest, lettre, signal subprocess) into the
//! channel-runtime crate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wcore_channels::auto_register::{ChannelFactory, ChannelLoadError};
use wcore_channels::{Channel, ChannelConfig, ChannelManager};
use wcore_config::credentials::CredentialsStore;

pub use wcore_channels::auto_register::{ChannelFactory as Factory, ChannelLoadError as LoadError};

/// Look up the constructor for a given platform string. Returns `None`
/// for any platform the registry doesn't know about — callers should
/// log + skip rather than crash so a single rogue config can't take
/// the agent down at boot.
pub fn channel_factory_for(platform: &str) -> Option<ChannelFactory> {
    match platform {
        "slack" => Some(make_slack),
        "telegram" => Some(make_telegram),
        "email" => Some(make_email),
        "discord" => Some(make_discord),
        "sms" => Some(make_sms),
        "whatsapp" => Some(make_whatsapp),
        "signal" => Some(make_signal),
        // F-045 (W7-M): new channel adapters ported from desktop OpenClaw fork.
        "matrix" => Some(make_matrix),
        "msteams" => Some(make_msteams),
        // iMessage is macOS-only; return None on other platforms so the
        // registry logs a clear skip rather than crashing.
        #[cfg(target_os = "macos")]
        "imessage" => Some(make_imessage),
        _ => None,
    }
}

/// Deserialize a per-channel options table into the platform's
/// concrete `Config` struct. Re-serializing through TOML keeps the
/// behaviour portable across toml-rs versions where `Table::try_into`
/// has churned (the round-trip via `Value` always works).
fn parse_options<T: serde::de::DeserializeOwned>(
    options: &toml::Table,
) -> Result<T, ChannelLoadError> {
    let value = toml::Value::Table(options.clone());
    value
        .try_into()
        .map_err(|e: toml::de::Error| ChannelLoadError::Config(e.to_string()))
}

fn make_slack(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_slack::SlackConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_slack::SlackChannel::new(
        name,
        cfg,
        credentials,
    )))
}

fn make_telegram(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_telegram::TelegramConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_telegram::TelegramChannel::new(
        name,
        cfg,
        credentials,
    )))
}

fn make_email(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_email::EmailConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_email::EmailChannel::new(
        name,
        cfg,
        credentials,
    )))
}

fn make_discord(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_discord::DiscordConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_discord::DiscordChannel::new(
        name,
        cfg,
        credentials,
    )))
}

fn make_sms(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_sms::SmsConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_sms::SmsChannel::new(
        name,
        cfg,
        credentials,
    )))
}

fn make_whatsapp(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_whatsapp::WhatsappConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_whatsapp::WhatsappChannel::new(
        name,
        cfg,
        credentials,
    )))
}

fn make_signal(
    name: String,
    options: &toml::Table,
    _credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    // Signal's bot creds live in signal-cli's own store, not the
    // engine-wide credentials backend — the `_credentials` argument is
    // intentionally ignored.
    let cfg: wcore_channel_signal::SignalConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_signal::SignalChannel::new(
        name, cfg,
    )))
}

// F-045 (W7-M): Matrix + MS Teams + iMessage factories.

fn make_matrix(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_matrix::MatrixConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_matrix::MatrixChannel::new(
        name,
        cfg,
        credentials,
    )))
}

fn make_msteams(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_msteams::MsTeamsConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_msteams::MsTeamsChannel::new(
        name,
        cfg,
        credentials,
    )))
}

#[cfg(target_os = "macos")]
fn make_imessage(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    let cfg: wcore_channel_imessage::IMessageConfig = parse_options(options)?;
    Ok(Box::new(wcore_channel_imessage::IMessageChannel::new(
        name,
        cfg,
        credentials,
    )))
}

/// Production entry point — scan `~/.wayland/channels/*.toml` and
/// register every channel whose `platform` field maps to a known
/// factory. Returns the count successfully registered.
///
/// Failures (missing dir, unreadable file, parse error, unknown
/// platform, factory error) are logged at `warn` and skipped so one
/// bad config can't block boot. The `Ok(0)` path covers a fresh
/// install where the user hasn't created any channel configs yet.
pub async fn auto_register_from_user_config(
    mgr: &mut ChannelManager,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<usize, ChannelLoadError> {
    let dir = dirs::home_dir()
        .ok_or_else(|| ChannelLoadError::Config("no home dir".to_string()))?
        .join(".wayland")
        .join("channels");
    auto_register_from_dir(mgr, &dir, credentials).await
}

/// Test-visible variant of [`auto_register_from_user_config`] that
/// takes an explicit directory instead of resolving `$HOME`. Lets
/// unit tests point at a tempdir without juggling the `HOME` env
/// var (which is racy under nextest).
pub async fn auto_register_from_dir(
    mgr: &mut ChannelManager,
    dir: &Path,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<usize, ChannelLoadError> {
    if !dir.exists() {
        tracing::debug!(
            target: "wcore_channels_registry",
            dir = %dir.display(),
            "channels dir does not exist; nothing to auto-register"
        );
        return Ok(0);
    }

    let entries = std::fs::read_dir(dir)
        .map_err(|e| ChannelLoadError::Config(format!("{}: {e}", dir.display())))?;

    // Sort by filename so registration order is deterministic regardless
    // of filesystem iteration order (matters for the
    // `ChannelManager::list_names` invariant).
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .collect();
    paths.sort();

    let mut registered = 0usize;
    for path in paths {
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => {
                tracing::warn!(
                    target: "wcore_channels_registry",
                    file = %path.display(),
                    "channel config has no valid file stem; skipping"
                );
                continue;
            }
        };
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channels_registry",
                    file = %path.display(),
                    error = %e,
                    "channel config read failed; skipping"
                );
                continue;
            }
        };
        let cfg: ChannelConfig = match toml::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channels_registry",
                    file = %path.display(),
                    error = %e,
                    "channel config parse failed; skipping"
                );
                continue;
            }
        };
        if cfg.name != name {
            tracing::warn!(
                target: "wcore_channels_registry",
                file = %path.display(),
                expected = %name,
                got = %cfg.name,
                "channel config name does not match file stem; skipping"
            );
            continue;
        }
        if !cfg.enabled {
            tracing::info!(
                target: "wcore_channels_registry",
                channel = %name,
                "channel disabled; skipping"
            );
            continue;
        }
        let factory = match channel_factory_for(&cfg.platform) {
            Some(f) => f,
            None => {
                tracing::warn!(
                    target: "wcore_channels_registry",
                    file = %path.display(),
                    platform = %cfg.platform,
                    "no factory registered for platform; skipping"
                );
                continue;
            }
        };
        match factory(cfg.name.clone(), &cfg.options, Arc::clone(&credentials)) {
            Ok(ch) => {
                mgr.register(ch).await;
                registered += 1;
                tracing::info!(
                    target: "wcore_channels_registry",
                    channel = %cfg.name,
                    platform = %cfg.platform,
                    "channel auto-registered"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channels_registry",
                    channel = %cfg.name,
                    platform = %cfg.platform,
                    error = %e,
                    "channel construction failed; skipping"
                );
            }
        }
    }
    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tempfile::TempDir;
    use wcore_config::credentials::{CredentialsError, CredentialsStore as CredsTrait};

    /// In-memory `CredentialsStore` impl so tests don't touch the
    /// real keyring or write a plaintext file to disk.
    struct MemStore {
        inner: StdMutex<std::collections::HashMap<String, String>>,
    }

    impl MemStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: StdMutex::new(std::collections::HashMap::new()),
            })
        }
    }

    impl CredsTrait for MemStore {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn creds() -> Arc<dyn CredentialsStore> {
        MemStore::new()
    }

    #[test]
    fn factory_lookup_covers_all_seven_platforms() {
        for platform in [
            "slack", "telegram", "email", "discord", "sms", "whatsapp", "signal",
        ] {
            assert!(
                channel_factory_for(platform).is_some(),
                "missing factory for {platform}"
            );
        }
    }

    /// F-045 (W7-M): verify the 3 new platforms are registered.
    /// iMessage is only checked on macOS since it's gated with #[cfg(target_os = "macos")].
    #[test]
    fn factory_lookup_covers_w7m_platforms() {
        assert!(
            channel_factory_for("matrix").is_some(),
            "missing factory for matrix"
        );
        assert!(
            channel_factory_for("msteams").is_some(),
            "missing factory for msteams"
        );
        // iMessage only available on macOS.
        #[cfg(target_os = "macos")]
        assert!(
            channel_factory_for("imessage").is_some(),
            "missing factory for imessage on macOS"
        );
    }

    #[test]
    fn factory_lookup_returns_none_for_unknown() {
        assert!(channel_factory_for("nonexistent").is_none());
        assert!(channel_factory_for("").is_none());
        assert!(channel_factory_for("SLACK").is_none(), "case-sensitive");
    }

    #[tokio::test]
    async fn auto_register_missing_dir_returns_zero() {
        let mut mgr = ChannelManager::new();
        let count = auto_register_from_dir(
            &mut mgr,
            Path::new("/nonexistent/wayland/channels/x"),
            creds(),
        )
        .await
        .unwrap();
        assert_eq!(count, 0);
        assert!(mgr.list_names().is_empty());
    }

    #[tokio::test]
    async fn auto_register_empty_dir_returns_zero() {
        let tmp = TempDir::new().unwrap();
        let mut mgr = ChannelManager::new();
        let count = auto_register_from_dir(&mut mgr, tmp.path(), creds())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn auto_register_three_configs_registers_three() {
        let tmp = TempDir::new().unwrap();

        // Slack config — uses SlackConfig's deny_unknown_fields shape.
        std::fs::write(
            tmp.path().join("acme-slack.toml"),
            r#"
name = "acme-slack"
platform = "slack"

[options]
workspace_name = "acme"
credential_handle_bot_token = "slack.acme.bot_token"
credential_handle_signing_secret = "slack.acme.signing_secret"
"#,
        )
        .unwrap();

        // Telegram config — minimal fields, defaults fill the rest.
        std::fs::write(
            tmp.path().join("acme-tg.toml"),
            r#"
name = "acme-tg"
platform = "telegram"

[options]
credential_handle = "telegram.acme.bot_token"
"#,
        )
        .unwrap();

        // Email config — outbound only (no IMAP section).
        std::fs::write(
            tmp.path().join("acme-mail.toml"),
            r#"
name = "acme-mail"
platform = "email"

[options]
from_address = "bot@acme.com"
[options.smtp]
host = "smtp.acme.com"
user_credential_handle = "email.acme.smtp_user"
password_credential_handle = "email.acme.smtp_pass"
"#,
        )
        .unwrap();

        let mut mgr = ChannelManager::new();
        let count = auto_register_from_dir(&mut mgr, tmp.path(), creds())
            .await
            .unwrap();
        assert_eq!(count, 3, "expected 3 registered channels");

        let mut names = mgr.list_names();
        names.sort();
        assert_eq!(names, vec!["acme-mail", "acme-slack", "acme-tg"]);
    }

    #[tokio::test]
    async fn unknown_platform_is_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("alien.toml"),
            r#"
name = "alien"
platform = "platform-from-the-future"

[options]
foo = "bar"
"#,
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("acme-slack.toml"),
            r#"
name = "acme-slack"
platform = "slack"

[options]
workspace_name = "acme"
credential_handle_bot_token = "k1"
credential_handle_signing_secret = "k2"
"#,
        )
        .unwrap();

        let mut mgr = ChannelManager::new();
        let count = auto_register_from_dir(&mut mgr, tmp.path(), creds())
            .await
            .unwrap();
        assert_eq!(count, 1, "alien platform skipped, slack still registered");
        assert_eq!(mgr.list_names(), vec!["acme-slack".to_string()]);
    }

    #[tokio::test]
    async fn malformed_toml_is_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("broken.toml"), "this is not [valid toml").unwrap();
        std::fs::write(
            tmp.path().join("acme-slack.toml"),
            r#"
name = "acme-slack"
platform = "slack"

[options]
workspace_name = "acme"
credential_handle_bot_token = "k1"
credential_handle_signing_secret = "k2"
"#,
        )
        .unwrap();

        let mut mgr = ChannelManager::new();
        let count = auto_register_from_dir(&mut mgr, tmp.path(), creds())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn disabled_channels_are_skipped() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("dormant.toml"),
            r#"
name = "dormant"
platform = "slack"
enabled = false

[options]
workspace_name = "acme"
credential_handle_bot_token = "k1"
credential_handle_signing_secret = "k2"
"#,
        )
        .unwrap();

        let mut mgr = ChannelManager::new();
        let count = auto_register_from_dir(&mut mgr, tmp.path(), creds())
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert!(mgr.list_names().is_empty());
    }

    #[tokio::test]
    async fn non_toml_files_skipped() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("README.md"), "ignore me").unwrap();
        std::fs::write(
            tmp.path().join("acme-slack.toml"),
            r#"
name = "acme-slack"
platform = "slack"

[options]
workspace_name = "acme"
credential_handle_bot_token = "k1"
credential_handle_signing_secret = "k2"
"#,
        )
        .unwrap();

        let mut mgr = ChannelManager::new();
        let count = auto_register_from_dir(&mut mgr, tmp.path(), creds())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
