//! `PluginsConfig` — `~/.wayland-core/plugins.toml` schema.
//!
//! Pure data + parser. The actual file-load wiring lands in W4 alongside the
//! interactive permission-grant UX; W2.5 ships this module so the host
//! loader (T9) can already consume it via `PluginsConfig::default()`.

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

impl PluginsConfig {
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
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
}
