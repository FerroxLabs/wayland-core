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

mod selector;
pub use selector::{ChannelSelector, constructible_selectors, known_platforms};

pub use wcore_channels::auto_register::{ChannelFactory as Factory, ChannelLoadError as LoadError};

/// The channel runtime this registry loads into, re-exported.
///
/// F24-03. `wayland-core channel` and `gateway run` need the manager, the
/// health projection and the probe report, and they already depend on this
/// crate to construct adapters at all. Re-exporting is how they reach those
/// types WITHOUT `wcore-cli` growing a second direct edge to `wcore-channels`
/// — a new dependency edge rewrites `Cargo.lock`, which is a Phase-24 shared
/// seam that concurrent lanes conflict on deterministically.
pub use wcore_channels;

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
        // F-045 (W7-M): channel adapters matching the desktop app's set.
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

/// WhatsApp has two transports behind one platform string.
///
/// The `backend` key selects. Absent or `meta-business` → the native Cloud API
/// adapter over HTTPS, which is the default and needs no Node. `baileys` or
/// `whatsapp-web` → the OPT-IN Node bridge subprocess. Anything else is
/// REJECTED here rather than defaulted: `bridge.js` itself falls back to
/// `baileys` on an unrecognised `--backend`, so a typo that reached the
/// subprocess would put an operator's personal WhatsApp number on the wire.
fn make_whatsapp(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError> {
    use std::str::FromStr;
    use wcore_channel_whatsapp::WhatsappBackend;

    let backend = match options.get("backend") {
        Some(v) => {
            let raw = v.as_str().ok_or_else(|| {
                ChannelLoadError::Config("whatsapp `backend` must be a string".to_string())
            })?;
            WhatsappBackend::from_str(raw).map_err(|e| ChannelLoadError::Config(e.to_string()))?
        }
        None => WhatsappBackend::default(),
    };

    if backend.is_bridged() {
        let cfg: wcore_channel_whatsapp::WhatsappBridgeConfig = parse_options(options)?;
        return Ok(Box::new(
            wcore_channel_whatsapp::WhatsappBridgeChannel::new(name, cfg),
        ));
    }

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
    auto_register_from_dir(mgr, &channels_dir(), credentials).await
}

/// The canonical channels directory: `$WAYLAND_HOME/channels` (or
/// `~/.wayland/channels` when unset).
///
/// F-019 fix: this resolves through [`wcore_config::config::profile_home`],
/// which honors `WAYLAND_HOME`. The previous loader joined
/// `dirs::home_dir()/.wayland/channels` directly, so a sandboxed/test process
/// under `WAYLAND_HOME` read the host user's real channel configs (the same
/// class of leak as the OAuth-token path). Both the engine loader and the TUI
/// Integrations view resolve the directory through here, so they never diverge.
pub fn channels_dir() -> PathBuf {
    wcore_config::config::profile_home().join("channels")
}

/// Where DM-pairing state lives: `<channels_dir>/pairings`.
///
/// Derived from [`channels_dir`] rather than resolved independently, for the
/// same reason the `[inbound]` policy is (F24-C3-H1): pairing state that is
/// minted into one directory and read from another is a gate that silently
/// never opens. Every operator verb and every runtime host calls THIS.
pub fn pairings_dir() -> PathBuf {
    channels_dir().join("pairings")
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
        // Through the shared parser, not `toml::from_str`, so the removed
        // `[secrets]` table produces its named migration here as well.
        // This is the load path `gateway run` and `channel probe` use, so it
        // is the one that matters most; a first pass wired only
        // `scan_channel_summaries` (i.e. `channel list`) and a live run caught
        // probe still emitting serde's generic `unknown field \`secrets\``.
        let cfg: ChannelConfig = match wcore_channels::parse_channel_config(&name, &body) {
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

/// A read-only, **secret-free** summary of one on-disk channel config, for the
/// TUI Integrations view (`/doctor`). Only key *names* are surfaced — never an
/// option or secret *value* — so the summary needs no redaction and can never
/// leak a credential.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSummary {
    /// Channel instance name (the file stem).
    pub name: String,
    /// Platform tag (`"slack"`, `"telegram"`, …); empty on a parse failure.
    pub platform: String,
    /// Whether the channel is enabled (auto-started at boot).
    pub enabled: bool,
    /// Whether `platform` maps to a known factory. An unknown platform is the
    /// answer to "why isn't my channel loading".
    pub known_platform: bool,
    /// The configured `[options]` key names (no values).
    pub option_keys: Vec<String>,
    /// Every credentials-store handle this config refers to, as
    /// `(dotted option path, handle)` — e.g.
    /// `("credential_handle_bot_token", "slack.acme.bot_token")`.
    ///
    /// Replaces the removed `secret_keys`, which summarised the inert
    /// `[secrets]` table. A handle is **not** a secret — it is the lookup
    /// key, and the adapters already print it in their own "not found at
    /// credential_handle …" errors — so listing it here leaks nothing and
    /// is what tells an operator which credentials they still owe.
    pub credential_handles: Vec<(String, String)>,
    /// `Some(message)` if the file could not be read/parsed — surfaced so a
    /// broken config is visible rather than silently absent.
    pub parse_error: Option<String>,
}

/// Scan a channels directory into secret-free [`ChannelSummary`] rows, sorted
/// by filename. A missing/unreadable directory yields an empty list; an
/// unreadable or unparseable file yields a summary carrying its `parse_error`
/// (so the operator can see *why* a channel isn't loading) rather than being
/// dropped. Read-only: never constructs a channel or touches the network.
pub fn scan_channel_summaries(dir: &Path) -> Vec<ChannelSummary> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .collect();
    paths.sort();

    for path in paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let broken = |msg: String| ChannelSummary {
            name: stem.clone(),
            platform: String::new(),
            enabled: false,
            known_platform: false,
            option_keys: Vec::new(),
            credential_handles: Vec::new(),
            parse_error: Some(msg),
        };
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                out.push(broken(format!("read failed: {e}")));
                continue;
            }
        };
        // Routed through the shared parser so a legacy `[secrets]` table
        // surfaces its named migration message here too — `channel list` is
        // where an operator looks first when a channel will not load, and
        // the generic serde line is what sent the UAT round-tripping.
        match wcore_channels::parse_channel_config(&stem, &body) {
            Ok(cfg) => {
                let mut option_keys: Vec<String> = cfg.options.keys().cloned().collect();
                option_keys.sort();
                let credential_handles = wcore_channels::credential_handles(&cfg.options);
                out.push(ChannelSummary {
                    known_platform: channel_factory_for(&cfg.platform).is_some(),
                    name: cfg.name,
                    platform: cfg.platform,
                    enabled: cfg.enabled,
                    option_keys,
                    credential_handles,
                    parse_error: None,
                });
            }
            Err(e) => out.push(broken(e.to_string())),
        }
    }
    out
}

/// Scan the user's [`channels_dir`] into secret-free summaries.
pub fn scan_user_channels() -> Vec<ChannelSummary> {
    scan_channel_summaries(&channels_dir())
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
    fn scan_summaries_reports_status_and_never_leaks_secret_values() {
        let dir = TempDir::new().unwrap();
        // Known platform, enabled, with options + a credential handle.
        // `SECRETVALUE` is the *handle*, not the secret: the assertion at the
        // end of this test is that no VALUE leaks, and the store is never
        // opened here, so nothing in this scan can carry one.
        std::fs::write(
            dir.path().join("myslack.toml"),
            "name = \"myslack\"\nplatform = \"slack\"\nenabled = true\n\
             [options]\nchannel = \"#general\"\n\
             credential_handle_bot_token = \"slack.myslack.bot_token\"\n",
        )
        .unwrap();
        // Known platform but disabled.
        std::fs::write(
            dir.path().join("mytg.toml"),
            "name = \"mytg\"\nplatform = \"telegram\"\nenabled = false\n",
        )
        .unwrap();
        // Unknown platform — the "why isn't it loading" case.
        std::fs::write(
            dir.path().join("weird.toml"),
            "name = \"weird\"\nplatform = \"carrierpigeon\"\n",
        )
        .unwrap();
        // Unparseable file — must surface as a parse_error, not vanish.
        std::fs::write(dir.path().join("broken.toml"), "name = = not valid").unwrap();
        // A config still carrying the REMOVED `[secrets]` table. Two jobs: it
        // is the known-positive that keeps the leak assertion below alive
        // (`SECRETVALUE` genuinely appears on disk, so a dump that contained
        // it would really be a leak), and it proves the named migration
        // message reaches `channel list` rather than only the loader.
        std::fs::write(
            dir.path().join("legacy.toml"),
            "name = \"legacy\"\nplatform = \"slack\"\n\
             [secrets]\nbot_token = \"keychain:slack:SECRETVALUE\"\n",
        )
        .unwrap();

        let summaries = scan_channel_summaries(dir.path());
        let by = |n: &str| {
            summaries
                .iter()
                .find(|c| c.name == n)
                .unwrap_or_else(|| panic!("missing summary {n}"))
        };

        let slack = by("myslack");
        assert_eq!(slack.platform, "slack");
        assert!(slack.known_platform && slack.enabled);
        assert!(slack.option_keys.contains(&"channel".to_string()));
        assert_eq!(
            slack.credential_handles,
            vec![(
                "credential_handle_bot_token".to_string(),
                "slack.myslack.bot_token".to_string()
            )],
            "the handle a config expects is what tells an operator what to store"
        );

        assert!(!by("mytg").enabled, "disabled channel must read disabled");
        assert!(
            !by("weird").known_platform,
            "unknown platform must read unknown"
        );
        assert!(
            summaries.iter().any(|c| c.parse_error.is_some()),
            "the broken file must surface a parse_error"
        );

        // The removed `[secrets]` table must reach `channel list` as the named
        // migration, not as serde's generic `unknown field` line.
        let legacy = by("legacy")
            .parse_error
            .clone()
            .expect("a [secrets] config must surface a parse_error");
        assert!(
            legacy.contains("[secrets]") && legacy.contains("channel credential set"),
            "the migration must be named and actionable, got: {legacy}"
        );
        assert!(
            !legacy.contains("unknown field"),
            "the named error must win over deny_unknown_fields, got: {legacy}"
        );

        // The secret VALUE must never appear anywhere in the summaries. This
        // is only meaningful because `legacy.toml` above really does contain
        // `SECRETVALUE` on disk — without that known-positive the assertion
        // would pass on an empty scan.
        let dump = format!("{summaries:?}");
        assert!(
            !dump.contains("SECRETVALUE"),
            "a secret value leaked into the summary:\n{dump}"
        );
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

    /// Build the options table a whatsapp channel is constructed from.
    /// `backend` is inserted only when `Some`, so the absent case is testable.
    fn whatsapp_options(backend: Option<&str>) -> toml::Table {
        let mut t = toml::Table::new();
        if let Some(b) = backend {
            t.insert("backend".into(), toml::Value::String(b.into()));
        }
        match backend {
            Some("baileys") | Some("whatsapp-web") => {
                t.insert(
                    "bridge_path".into(),
                    toml::Value::String("/definitely/not/here/bridge.js".into()),
                );
            }
            _ => {
                t.insert("workspace_name".into(), toml::Value::String("acme".into()));
                t.insert("phone_number_id".into(), toml::Value::String("1".into()));
                t.insert(
                    "credential_handle_access_token".into(),
                    toml::Value::String("k1".into()),
                );
                t.insert(
                    "credential_handle_app_secret".into(),
                    toml::Value::String("k2".into()),
                );
            }
        }
        t
    }

    /// The whatsapp backend selector must actually SELECT — three positive
    /// cases and one rejection, so neither direction is assumed.
    #[test]
    fn whatsapp_backend_selector_routes_and_rejects() {
        let factory = channel_factory_for("whatsapp").expect("whatsapp factory");
        let creds = creds();

        // Absent `backend` → the Cloud API adapter. This is the opt-in
        // guarantee: every config written before the bridge existed keeps
        // working with no Node installed.
        let ch = factory(
            "wa-cloud".into(),
            &whatsapp_options(None),
            Arc::clone(&creds),
        )
        .expect("a backend-less whatsapp config must still construct");
        assert_eq!(ch.platform(), "whatsapp");

        // Explicit `meta-business` → the same adapter.
        factory(
            "wa-cloud2".into(),
            &whatsapp_options(Some("meta-business")),
            Arc::clone(&creds),
        )
        .expect("meta-business must construct the Cloud API adapter");

        // `baileys` → the bridge adapter. Construction must SUCCEED even
        // though the bridge is absent: the failure belongs at start()/probe(),
        // named, not at boot.
        factory(
            "wa-personal".into(),
            &whatsapp_options(Some("baileys")),
            Arc::clone(&creds),
        )
        .expect("baileys must construct even with no bridge on disk");

        // An unknown backend is REJECTED, not defaulted.
        // `Box<dyn Channel>` is not `Debug`, so unwrap the error by hand.
        let err = expect_construct_err(
            factory(
                "wa-typo".into(),
                &whatsapp_options(Some("baileyz")),
                Arc::clone(&creds),
            ),
            "an unknown backend must be rejected, never defaulted",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("baileyz"),
            "the rejection must echo the typo: {msg}"
        );
        assert!(
            msg.contains("meta-business"),
            "the rejection must name the valid options: {msg}"
        );
    }

    /// A bridged whatsapp config missing its required key must fail to
    /// construct — the control proving the bridge arm parses its own schema
    /// rather than accepting anything.
    #[test]
    fn whatsapp_bridge_config_requires_a_bridge_path() {
        let factory = channel_factory_for("whatsapp").expect("whatsapp factory");
        let mut opts = toml::Table::new();
        opts.insert("backend".into(), toml::Value::String("baileys".into()));
        let err = expect_construct_err(
            factory("wa".into(), &opts, creds()),
            "bridge_path is required",
        );
        assert!(
            err.to_string().contains("bridge_path"),
            "the error must name the missing key: {err}"
        );
    }

    /// `Box<dyn Channel>` is not `Debug`, so `Result::expect_err` is
    /// unavailable on a factory result. Unwrap the error by hand, and panic
    /// loudly if a construction that must fail instead succeeded.
    fn expect_construct_err(
        result: Result<Box<dyn Channel>, ChannelLoadError>,
        msg: &str,
    ) -> ChannelLoadError {
        match result {
            Ok(ch) => panic!("{msg} — but it constructed a {:?} channel", ch.platform()),
            Err(e) => e,
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
