//! `ChannelConfig` — TOML schema + on-disk loader.
//!
//! Layout: `~/.wayland/channels/<name>.toml`. Each file is one channel
//! instance. **A channel config file never contains a secret.** Each
//! adapter declares one or more `*credential_handle*` keys in its
//! `[options]` table whose values are *lookup keys* into the
//! [`wcore_config::credentials::CredentialsStore`]; the adapter resolves
//! them at `start()` time, so the secret only ever exists in the store
//! and in memory. Write one with `wayland-core channel credential set`.
//!
//! # The removed `[secrets]` table
//!
//! An earlier schema accepted a `[secrets]` table documented as holding
//! `keychain:<service>:<account>` references. **Nothing ever resolved
//! it.** Its only consumer read the key *names* for a diagnostic
//! summary and never the values, and no adapter field was ever bound to
//! a `[secrets]` key — so the feature was unfinished rather than merely
//! unimplemented, and a user who followed the comment got a channel that
//! silently never authenticated.
//!
//! The table is now rejected by [`parse_channel_config`] with a named
//! migration error rather than accepted-and-ignored, and rather than
//! left to `deny_unknown_fields`' generic `unknown field` line. A silent
//! misconfiguration becomes a loud, actionable one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ChannelError;

/// Persisted config for one channel instance.
///
/// Note: `PartialEq` only — `toml::Table` (the value type used in
/// `options`) implements `PartialEq` but not `Eq`, so neither does
/// `ChannelConfig`.
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
    /// its own subset. Credentials appear here only as *handles* —
    /// see [`credential_handles`].
    #[serde(default)]
    pub options: toml::Table,
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

/// The removed table name, rejected by [`parse_channel_config`].
const LEGACY_SECRETS_TABLE: &str = "secrets";

/// Substring identifying an option key whose value is a credentials-store
/// lookup key.
///
/// It is a **substring** test and not a prefix test on purpose: the email
/// adapter spells its handles `user_credential_handle` and
/// `password_credential_handle` — *suffixed* — while the other nine
/// adapters use `credential_handle` or `credential_handle_<what>`. A
/// `starts_with` test silently omits email entirely.
pub const CREDENTIAL_HANDLE_MARKER: &str = "credential_handle";

/// Parse one channel config file body.
///
/// `source` names the file for error messages. Prefer this over calling
/// `toml::from_str::<ChannelConfig>` directly: it rejects the removed
/// `[secrets]` table with a migration message that names the working
/// alternative, which `deny_unknown_fields` cannot do.
pub fn parse_channel_config(source: &str, body: &str) -> Result<ChannelConfig, ChannelError> {
    // Checked before the typed parse so this message wins over serde's
    // generic `unknown field \`secrets\`` line.
    if let Ok(raw) = body.parse::<toml::Table>()
        && raw.contains_key(LEGACY_SECRETS_TABLE)
    {
        return Err(ChannelError::Config(format!(
            "{source}: the `[secrets]` table is no longer accepted, and never had any \
             effect — no code ever read its values, so this channel has not been \
             authenticating. Move each secret into the credentials store with \
             `wayland-core channel credential set <handle>`, then reference it from \
             `[options]` by this platform's `*{CREDENTIAL_HANDLE_MARKER}*` key. \
             Run `wayland-core channel credential list` to see the handles your \
             configs expect. See docs/channels.md."
        )));
    }
    toml::from_str::<ChannelConfig>(body)
        .map_err(|e| ChannelError::Config(format!("{source}: {e}")))
}

/// Collect every credentials-store handle an `[options]` table refers to,
/// as `(dotted option path, handle value)`, sorted by path.
///
/// Recurses into sub-tables because the email adapter nests its handles
/// under `[options.smtp]` and `[options.imap]`; a flat scan finds none of
/// its four. Non-string values are skipped rather than erroring — the
/// adapter's own typed parse is what reports a malformed option.
pub fn credential_handles(options: &toml::Table) -> Vec<(String, String)> {
    fn walk(prefix: &str, table: &toml::Table, out: &mut Vec<(String, String)>) {
        for (key, value) in table {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match value {
                toml::Value::Table(sub) => walk(&path, sub, out),
                toml::Value::String(s)
                    if key.contains(CREDENTIAL_HANDLE_MARKER) && !s.is_empty() =>
                {
                    out.push((path, s.clone()));
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk("", options, &mut out);
    out.sort();
    out
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
            let cfg = parse_channel_config(&path.display().to_string(), &body)?;
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
credential_handle_bot_token = "slack.acme.bot_token"
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

    // --- the removed `[secrets]` table -------------------------------------

    /// The negative direction: a legacy `[secrets]` table must be rejected,
    /// and the message must be actionable rather than serde's generic
    /// `unknown field` line. Both halves are asserted — a bare
    /// `is_err()` would also pass on the generic message, which is the
    /// outcome this change exists to avoid.
    #[test]
    fn legacy_secrets_table_is_rejected_with_a_named_migration() {
        let err = parse_channel_config(
            "acme.toml",
            r#"
name = "acme"
platform = "slack"

[secrets]
bot_token = "keychain:wayland-channels:acme-bot"
"#,
        )
        .expect_err("a [secrets] table must not load");
        let msg = err.to_string();
        assert!(msg.contains("[secrets]"), "must name the table: {msg}");
        assert!(
            msg.contains("channel credential set"),
            "must name the verb that replaces it: {msg}"
        );
        assert!(
            msg.contains(CREDENTIAL_HANDLE_MARKER),
            "must name the replacement option key: {msg}"
        );
        assert!(
            !msg.contains("unknown field"),
            "the named error must win over deny_unknown_fields: {msg}"
        );
    }

    /// The positive direction for the same gate: the identical config with
    /// the table migrated to a handle loads clean. Without this, the check
    /// above would also pass on a parser that rejected everything.
    #[test]
    fn the_migrated_form_of_that_same_config_loads() {
        let cfg = parse_channel_config(
            "acme.toml",
            r#"
name = "acme"
platform = "slack"

[options]
credential_handle_bot_token = "slack.acme.bot_token"
"#,
        )
        .expect("the migrated config must load");
        assert_eq!(cfg.name, "acme");
        assert_eq!(
            credential_handles(&cfg.options),
            vec![(
                "credential_handle_bot_token".to_string(),
                "slack.acme.bot_token".to_string()
            )]
        );
    }

    /// An empty `[secrets]` table is still a rejection: it is a leftover
    /// from the removed schema and silently dropping it would leave the
    /// operator believing the file had been understood.
    #[test]
    fn an_empty_legacy_secrets_table_is_also_rejected() {
        let err = parse_channel_config(
            "acme.toml",
            "name = \"acme\"\nplatform = \"slack\"\n\n[secrets]\n",
        )
        .expect_err("even an empty [secrets] table must not load");
        assert!(err.to_string().contains("[secrets]"));
    }

    // --- handle discovery ---------------------------------------------------

    /// The email adapter is the reason [`credential_handles`] recurses and
    /// matches a substring: its handles are SUFFIXED (`user_credential_handle`)
    /// and nested under `[options.smtp]` / `[options.imap]`. A prefix-only
    /// flat scan returns zero here, so this test is what keeps the walker
    /// honest.
    #[test]
    fn credential_handles_finds_nested_and_suffixed_email_handles() {
        let cfg = parse_channel_config(
            "mail.toml",
            r#"
name = "mail"
platform = "email"

[options.smtp]
host = "smtp.acme.com"
user_credential_handle = "email.acme.smtp_user"
password_credential_handle = "email.acme.smtp_pass"

[options.imap]
host = "imap.acme.com"
user_credential_handle = "email.acme.imap_user"
password_credential_handle = "email.acme.imap_pass"
"#,
        )
        .unwrap();
        let found = credential_handles(&cfg.options);
        let paths: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "imap.password_credential_handle",
                "imap.user_credential_handle",
                "smtp.password_credential_handle",
                "smtp.user_credential_handle",
            ],
            "all four nested, suffixed handles must be found"
        );
        // A prefix-only scan would have found none of them — assert that,
        // so a future "simplification" back to starts_with reddens here.
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.starts_with(CREDENTIAL_HANDLE_MARKER))
                .count(),
            0,
            "these handles are suffixed; a starts_with scan is wrong"
        );
    }

    /// Known-negative for the walker: an options table with no handles
    /// yields none, and a non-string value under a handle-shaped key is
    /// skipped rather than panicking.
    #[test]
    fn credential_handles_ignores_unrelated_and_non_string_keys() {
        let cfg = parse_channel_config(
            "x.toml",
            r#"
name = "x"
platform = "discord"

[options]
allowed_channel_ids = ["1", "2"]
intents = 37376
credential_handle = ""
"#,
        )
        .unwrap();
        assert!(
            credential_handles(&cfg.options).is_empty(),
            "an empty handle is not a handle, and non-strings are skipped"
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
