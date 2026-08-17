//! W8c.1 E.11 — `BrowserConfig` TOML schema for the multi-backend browser
//! tool family. Matches design §5.16 surface.
//!
//! This is a thin config crate — the actual provider selection logic lives
//! in `wcore_browser::selection::select_provider`. We mirror the operator-
//! facing fields here so config loading stays in `wcore-config` (which
//! already owns the cascade + profile system).

use serde::{Deserialize, Serialize};

/// Preferred provider. Mirrors `wcore_browser::ProviderHint`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProvider {
    #[default]
    Auto,
    Camoufox,
    Chromium,
    Browserbase,
}

/// Stealth / provider-selection config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StealthConfig {
    pub preferred_provider: BrowserProvider,
    /// When `false`, never select Browserbase even if env creds are present.
    pub allow_cloud_fallback: bool,
}

/// Operator-facing shape of the gh#911 loopback capability grant.
///
/// Mirrors `wcore_browser::policy::LoopbackCapability`. Every field defaults
/// to the no-authority value; `wcore_browser` re-validates all of them, so a
/// grant that is absent, version-mismatched, unscoped or portless leaves
/// loopback hard-blocked.
///
/// On disk (note the section — `[browser.policy]`, not `[browser]`):
///
/// ```toml
/// [browser.policy.loopback]
/// enabled = true
/// schema_version = 1
/// session_scope = "my-local-dev"
/// ports = [3000]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserLoopbackConfig {
    pub enabled: bool,
    pub schema_version: u32,
    pub session_scope: String,
    pub ports: Vec<u16>,
}

/// Policy mirror — `wcore_browser::BrowserPolicy` accepts these fields too.
///
/// The `default_action` default is `"deny"` (fail-closed) since v0.2.1
/// — operators must explicitly allow-list the origins their agents may
/// touch. Pre-v0.2.1 this defaulted to `"allow"` which was a fail-open
/// SSRF risk (see `STABILITY-v0.2.0.md` MAJOR #6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserPolicyConfig {
    /// `deny` (default) | `allow` | `ask`.
    pub default_action: String,
    pub allowed_origins: Vec<String>,
    pub denied_origins: Vec<String>,
    /// Recoverable local-only loopback grant (gh#911). Off by default.
    pub loopback: BrowserLoopbackConfig,
}

impl Default for BrowserPolicyConfig {
    fn default() -> Self {
        Self {
            // Fail-closed: matches `wcore_browser::PolicyAction::default()`.
            default_action: "deny".into(),
            allowed_origins: Vec::new(),
            denied_origins: Vec::new(),
            loopback: BrowserLoopbackConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    pub stealth: StealthConfig,
    pub policy: BrowserPolicyConfig,
    /// Where downloads land. Empty = use system default.
    pub download_dir: Option<String>,
    /// When true, the same on-disk profile is reused across sessions.
    pub persist_profile: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_is_auto() {
        assert_eq!(BrowserProvider::default(), BrowserProvider::Auto);
    }

    #[test]
    fn round_trip_toml() {
        let cfg = BrowserConfig {
            stealth: StealthConfig {
                preferred_provider: BrowserProvider::Camoufox,
                allow_cloud_fallback: true,
            },
            policy: BrowserPolicyConfig {
                default_action: "ask".into(),
                allowed_origins: vec!["*.example.com".into()],
                denied_origins: vec![],
                loopback: BrowserLoopbackConfig::default(),
            },
            download_dir: Some("/tmp/downloads".into()),
            persist_profile: false,
        };
        let s = toml::to_string(&cfg).unwrap();
        let parsed: BrowserConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.stealth.preferred_provider, BrowserProvider::Camoufox);
        assert_eq!(parsed.policy.default_action, "ask");
        assert!(parsed.stealth.allow_cloud_fallback);
    }

    /// The grant has to survive the real TOML round trip under the section
    /// the loader reads, and it has to be OFF when nobody wrote it.
    #[test]
    fn loopback_grant_round_trips_and_defaults_off() {
        let cfg: BrowserConfig = toml::from_str("").unwrap();
        assert!(!cfg.policy.loopback.enabled);
        assert_eq!(cfg.policy.loopback.schema_version, 0);
        assert!(cfg.policy.loopback.ports.is_empty());

        let cfg: BrowserConfig = toml::from_str(
            "[policy.loopback]\n\
             enabled = true\n\
             schema_version = 1\n\
             session_scope = \"dev\"\n\
             ports = [3000, 5173]\n",
        )
        .unwrap();
        assert!(cfg.policy.loopback.enabled);
        assert_eq!(cfg.policy.loopback.schema_version, 1);
        assert_eq!(cfg.policy.loopback.session_scope, "dev");
        assert_eq!(cfg.policy.loopback.ports, vec![3000, 5173]);
    }

    #[test]
    fn empty_toml_uses_defaults() {
        let cfg: BrowserConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.stealth.preferred_provider, BrowserProvider::Auto);
        assert!(!cfg.stealth.allow_cloud_fallback);
        assert!(!cfg.persist_profile);
    }
}
