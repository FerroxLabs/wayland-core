//! `ChannelConfig` — TOML schema + on-disk loader.
//!
//! Layout: `~/.wayland/channels/<name>.toml`. Each file is one
//! channel instance; the `[secrets]` table holds plaintext only if
//! the operator explicitly chose unsafe-on-disk storage — otherwise
//! values are keychain references (`keychain:<service>:<account>`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ChannelError;

/// Persisted config for one channel instance.
///
/// Note: `PartialEq` only — `toml::Table` (the value type used in
/// `options` / `secrets`) implements `PartialEq` but not `Eq`, so
/// neither does `ChannelConfig`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChannelConfig {
    /// Stable name — matches file stem. Validated at load time.
    pub name: String,
    /// Platform tag (`"slack"`, `"discord"`, …).
    pub platform: String,
    /// Whether the manager should auto-start this channel on engine
    /// boot. Default `true`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Free-form platform-specific options. Each channel impl parses
    /// its own subset.
    #[serde(default)]
    pub options: toml::Table,
    /// Secret references. Values are `keychain:<service>:<account>`
    /// (preferred) or plain strings (unsafe; logged as warning).
    #[serde(default)]
    pub secrets: toml::Table,
    /// Inbound access + session-shaping policy for this channel. Absent
    /// `[inbound]` table -> the fail-closed default (denies all inbound
    /// until the operator adds allowlist entries). See
    /// [`crate::dispatch::access::InboundPolicy`].
    #[serde(default)]
    pub inbound: crate::dispatch::access::InboundPolicy,
}

fn default_enabled() -> bool {
    true
}

/// Loader for `~/.wayland/channels/*.toml`.
#[derive(Debug, Clone)]
pub struct ChannelConfigLoader {
    root: PathBuf,
}

impl ChannelConfigLoader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Default loader rooted at `$HOME/.wayland/channels`. Falls back
    /// to the platform temp dir if `$HOME` is unset.
    pub fn default_root() -> PathBuf {
        match std::env::var_os("HOME") {
            Some(h) => Path::new(&h).join(".wayland").join("channels"),
            None => std::env::temp_dir().join("wayland-channels"),
        }
    }

    /// Read every `*.toml` under `root`. Files that fail to parse
    /// surface as `Err`; the loader stops at the first failure so
    /// the operator can fix and reload.
    pub fn load_all(&self) -> Result<Vec<ChannelConfig>, ChannelError> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(ChannelError::Config(e.to_string())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| ChannelError::Config(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let body = std::fs::read_to_string(&path)
                .map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))?;
            let cfg: ChannelConfig = toml::from_str(&body)
                .map_err(|e| ChannelError::Config(format!("{}: {e}", path.display())))?;
            let expected_stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if cfg.name != expected_stem {
                return Err(ChannelError::Config(format!(
                    "{}: name field {:?} does not match file stem {:?}",
                    path.display(),
                    cfg.name,
                    expected_stem
                )));
            }
            out.push(cfg);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_all_empty_dir_returns_ok_empty() {
        let tmp = TempDir::new().unwrap();
        let loader = ChannelConfigLoader::new(tmp.path());
        let v = loader.load_all().unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn load_all_missing_dir_returns_ok_empty() {
        let loader = ChannelConfigLoader::new("/nonexistent/wayland/channels");
        let v = loader.load_all().unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn load_all_round_trips_one_config() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("acme.toml"),
            r#"
name = "acme"
platform = "slack"

[options]
workspace = "acme.slack.com"

[secrets]
bot_token = "keychain:wayland-channels:acme-bot"
"#,
        )
        .unwrap();
        let loader = ChannelConfigLoader::new(tmp.path());
        let v = loader.load_all().unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "acme");
        assert_eq!(v[0].platform, "slack");
        assert!(v[0].enabled, "default_enabled should be true");
        assert_eq!(
            v[0].options.get("workspace").and_then(|v| v.as_str()),
            Some("acme.slack.com")
        );
    }

    #[test]
    fn name_must_match_file_stem() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("acme.toml"),
            r#"
name = "different"
platform = "slack"
"#,
        )
        .unwrap();
        let loader = ChannelConfigLoader::new(tmp.path());
        let err = loader.load_all().expect_err("expected mismatch");
        assert!(matches!(err, ChannelError::Config(_)));
    }

    #[test]
    fn inbound_table_round_trips_into_policy() {
        use crate::dispatch::access::{DmPolicy, GroupPolicy};
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("acme.toml"),
            r#"
name = "acme"
platform = "slack"

[inbound]
dm = "allowlist"
group = "open"
require_mention = false
dm_allowlist = ["*"]
"#,
        )
        .unwrap();
        let loader = ChannelConfigLoader::new(tmp.path());
        let v = loader.load_all().unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].inbound.dm, DmPolicy::Allowlist);
        assert_eq!(v[0].inbound.group, GroupPolicy::Open);
        assert!(!v[0].inbound.require_mention);
        assert_eq!(v[0].inbound.dm_allowlist, vec!["*".to_string()]);
    }

    #[test]
    fn missing_inbound_table_defaults_fail_closed() {
        use crate::dispatch::access::{DmPolicy, GroupPolicy, InboundPolicy};
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("acme.toml"),
            r#"
name = "acme"
platform = "slack"
"#,
        )
        .unwrap();
        let loader = ChannelConfigLoader::new(tmp.path());
        let v = loader.load_all().unwrap();
        assert_eq!(v.len(), 1);
        // No [inbound] table -> the fail-closed default.
        assert_eq!(v[0].inbound, InboundPolicy::default());
        assert_eq!(v[0].inbound.dm, DmPolicy::Allowlist);
        assert_eq!(v[0].inbound.group, GroupPolicy::Disabled);
        assert!(v[0].inbound.require_mention);
        assert!(v[0].inbound.dm_allowlist.is_empty());
    }

    #[test]
    fn non_toml_files_skipped() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("README.md"), "ignore me").unwrap();
        std::fs::write(
            tmp.path().join("acme.toml"),
            "name=\"acme\"\nplatform=\"slack\"",
        )
        .unwrap();
        let loader = ChannelConfigLoader::new(tmp.path());
        let v = loader.load_all().unwrap();
        assert_eq!(v.len(), 1);
    }
}
