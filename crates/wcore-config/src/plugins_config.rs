//! `PluginsConfig` — the `plugins.toml` schema and its on-disk loader.
//!
//! The file lives beside `config.toml` in the app config root
//! ([`crate::config::app_config_dir`]), so `WAYLAND_HOME` /
//! `XDG_DATA_HOME` redirect it like every other engine-owned file.
//! [`PluginsConfig::load`] is what binds `enabled = false`,
//! `plugin_signature_verification` and `trusted_plugin_keys` to something —
//! before it existed the engine booted from `PluginsConfig::default()` and
//! every setting in the file was inert.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginsConfig {
    #[serde(default)]
    pub plugin: Vec<PluginEntry>,
    /// Whether plugin binaries must carry a valid ed25519 signature before
    /// the engine will load them. Defaults to `true` (signing enforced).
    /// Operators may opt out by setting this to `false` in `plugins.toml`.
    #[serde(default = "default_plugin_signature_verification")]
    pub plugin_signature_verification: bool,
    /// Sec6: hex-encoded ed25519 verifying keys (32 bytes = 64 hex chars each).
    /// Only used when `plugin_signature_verification = true`.
    #[serde(default)]
    pub trusted_plugin_keys: Vec<String>,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            plugin: Vec::new(),
            plugin_signature_verification: default_plugin_signature_verification(),
            trusted_plugin_keys: Vec::new(),
        }
    }
}

/// Default for `plugin_signature_verification`: signing is enforced
/// unless an operator opts out in `plugins.toml`. Restores the
/// production-hardening posture audited in Phase 0 (v0.7.0 security H2).
fn default_plugin_signature_verification() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginEntry {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub permissions_granted: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

/// Absolute path of the engine-wide `plugins.toml`.
///
/// Resolved from the same root as `config.toml` so `WAYLAND_HOME` sandboxes
/// it too. The fallback mirrors [`crate::config::global_config_path`].
pub fn plugins_config_path() -> PathBuf {
    crate::config::app_config_dir()
        .unwrap_or_else(|| PathBuf::from("wayland-core"))
        .join("plugins.toml")
}

/// Why a `plugins.toml` could not be honoured.
///
/// Deliberately NOT collapsed into "use the defaults": a `plugins.toml` the
/// engine cannot parse is an operator policy it cannot enforce, and silently
/// booting with default policy is how `enabled = false` came to mean nothing.
#[derive(Debug, thiserror::Error)]
pub enum PluginsConfigError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

impl PluginsConfig {
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Load the engine-wide `plugins.toml`, or the built-in defaults when no
    /// such file exists. A malformed or unreadable file is an error, never a
    /// silent fallback.
    pub fn load() -> Result<Self, PluginsConfigError> {
        Self::load_from_path(&plugins_config_path())
    }

    /// [`Self::load`] against an explicit path. Absent file → defaults.
    pub fn load_from_path(path: &Path) -> Result<Self, PluginsConfigError> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(PluginsConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        Self::from_toml_str(&raw).map_err(|source| PluginsConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn entry(&self, name: &str) -> Option<&PluginEntry> {
        self.plugin.iter().find(|e| e.name == name)
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.entry(name).map(|e| e.enabled).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_plugins_toml() {
        let s = r#"
[[plugin]]
name = "wayland-ijfw"
enabled = true
permissions_granted = ["register_mcp_server"]

[[plugin]]
name = "wayland-browser"
enabled = true

[[plugin]]
name = "wayland-ollama"
enabled = false
"#;
        let cfg = PluginsConfig::from_toml_str(s).expect("parse");
        assert_eq!(cfg.plugin.len(), 3);
        assert!(cfg.is_enabled("wayland-ijfw"));
        assert!(!cfg.is_enabled("wayland-ollama"));
        assert!(cfg.is_enabled("nonexistent")); // default-true
        assert_eq!(
            cfg.entry("wayland-ijfw").unwrap().permissions_granted,
            vec!["register_mcp_server"]
        );
    }

    #[test]
    fn empty_file_is_valid() {
        let cfg = PluginsConfig::from_toml_str("").expect("parse empty");
        assert!(cfg.plugin.is_empty());
    }

    #[test]
    fn signature_verification_defaults_to_true() {
        let cfg: PluginsConfig = toml::from_str("").expect("empty toml parses");
        assert!(
            cfg.plugin_signature_verification,
            "must default to enabled (v0.7.0 security audit H2)"
        );
        assert!(
            PluginsConfig::default().plugin_signature_verification,
            "Default impl must also enforce signing"
        );
    }

    #[test]
    fn signature_verification_can_be_explicitly_disabled() {
        let cfg: PluginsConfig = toml::from_str("plugin_signature_verification = false\n")
            .expect("explicit false parses");
        assert!(!cfg.plugin_signature_verification);
    }

    #[test]
    fn load_from_path_reads_the_file_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plugins.toml");
        std::fs::write(
            &path,
            "plugin_signature_verification = false\n\
             trusted_plugin_keys = [\"deadbeef\"]\n\
             \n\
             [[plugin]]\n\
             name = \"wayland-ollama\"\n\
             enabled = false\n",
        )
        .expect("write");

        let cfg = PluginsConfig::load_from_path(&path).expect("load");
        assert!(
            !cfg.is_enabled("wayland-ollama"),
            "enabled = false on disk must reach the loaded config"
        );
        assert!(
            !cfg.plugin_signature_verification,
            "plugin_signature_verification on disk must reach the loaded config"
        );
        assert_eq!(cfg.trusted_plugin_keys, vec!["deadbeef".to_string()]);
    }

    #[test]
    fn load_from_path_absent_file_is_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = PluginsConfig::load_from_path(&dir.path().join("plugins.toml"))
            .expect("absent file is not an error");
        assert!(cfg.plugin.is_empty());
        assert!(cfg.plugin_signature_verification);
    }

    #[test]
    fn load_from_path_malformed_file_is_an_error_not_a_silent_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plugins.toml");
        std::fs::write(&path, "this is not = = toml\n").expect("write");
        let err = PluginsConfig::load_from_path(&path)
            .expect_err("a malformed policy file must not degrade to default policy");
        assert!(
            matches!(err, PluginsConfigError::Parse { .. }),
            "expected a Parse error, got {err:?}"
        );
    }

    #[test]
    fn plugins_config_path_sits_beside_the_global_config() {
        let path = plugins_config_path();
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("plugins.toml")
        );
        assert_eq!(
            path.parent(),
            crate::config::global_config_path().parent(),
            "plugins.toml must resolve into the same root as config.toml"
        );
    }
}
