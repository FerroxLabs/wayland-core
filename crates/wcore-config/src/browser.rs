//! W8c.1 E.11 — `BrowserConfig` TOML schema for the multi-backend browser
//! tool family. Matches design §5.16 surface.
//!
//! This is a thin config crate — the actual provider selection logic lives
//! in `wcore_browser::selection::select_provider`. We mirror the operator-
//! facing fields here so config loading stays in `wcore-config` (which
//! already owns the cascade + profile system).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Key identifying the platform an artifact is built for: `"<os>-<arch>"`
/// from `std::env::consts` — e.g. `linux-x86_64`, `macos-aarch64`,
/// `windows-x86_64`.
///
/// Centralised here rather than spelled out at each consumer. Per the
/// architecture rule "Centralize Platform Differences", the platform is a
/// **lookup key into configuration**, not a `cfg!`/`if` ladder that hardcodes
/// one vendor's URL per target.
pub fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// One downloadable artifact, for one platform.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BinaryArtifact {
    /// Where the artifact is fetched from.
    pub url: String,
    /// Operator-supplied SHA-256 (hex) of the bytes at [`Self::url`].
    ///
    /// **There is no built-in digest and none is ever guessed.** An empty
    /// value means *unpinned*, and an unpinned artifact is refused, never
    /// fetched — see `wcore_browser::binary::BrowserBinaryManager::provision_camoufox`.
    pub sha256: String,
    /// Path of the executable *inside* the archive, relative to the archive
    /// root (e.g. `camoufox/camoufox`). Empty means the artifact is itself a
    /// bare executable and no extraction is performed.
    pub archive_exe_path: String,
}

/// Opt-in auto-provisioning of the Camoufox sidecar binary.
///
/// [`Self::enabled`] defaults to **false**: Core does not fetch executable
/// code from the network unless an operator explicitly turns it on *and*
/// pins a digest for the platform being provisioned. Enabling it without a
/// digest is an error, not a permission to fetch-and-trust.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CamoufoxDownloadConfig {
    /// Master switch. OFF by default.
    pub enabled: bool,
    /// Artifacts keyed by [`platform_key`].
    pub artifacts: BTreeMap<String, BinaryArtifact>,
}

impl CamoufoxDownloadConfig {
    /// The artifact configured for the platform this process is running on,
    /// if any. `None` is a refusal condition for the download path, never a
    /// cue to fall back to some other platform's artifact.
    pub fn artifact_for_current_platform(&self) -> Option<&BinaryArtifact> {
        self.artifacts.get(&platform_key())
    }
}

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
    /// Opt-in auto-download of the Camoufox sidecar binary
    /// (`[browser.camoufox_download]`). Disabled by default.
    pub camoufox_download: CamoufoxDownloadConfig,
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
            camoufox_download: CamoufoxDownloadConfig::default(),
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

    /// The auto-download switch must be OFF for a config that never mentions
    /// it — a default-on network fetch of an executable is not acceptable.
    #[test]
    fn camoufox_download_is_off_by_default() {
        let cfg: BrowserConfig = toml::from_str("").unwrap();
        assert!(
            !cfg.camoufox_download.enabled,
            "auto-download must default to disabled"
        );
        assert!(cfg.camoufox_download.artifacts.is_empty());
        assert!(
            cfg.camoufox_download
                .artifact_for_current_platform()
                .is_none()
        );
    }

    /// The operator-facing TOML path is `[browser.camoufox_download]` with a
    /// per-platform artifacts table keyed by [`platform_key`]. Parsing it
    /// through the real serde types is what stops the documented key and the
    /// loaded key from drifting apart (cf. `config_hint.rs`).
    #[test]
    fn camoufox_download_round_trips_per_platform_artifacts() {
        let key = platform_key();
        let toml_src = format!(
            r#"
[camoufox_download]
enabled = true

[camoufox_download.artifacts."{key}"]
url = "https://example.test/camoufox.tar.gz"
sha256 = "aa"
archive_exe_path = "camoufox/camoufox"
"#
        );
        let cfg: BrowserConfig = toml::from_str(&toml_src).unwrap();
        assert!(cfg.camoufox_download.enabled);
        let a = cfg
            .camoufox_download
            .artifact_for_current_platform()
            .expect("artifact must resolve for the running platform");
        assert_eq!(a.url, "https://example.test/camoufox.tar.gz");
        assert_eq!(a.sha256, "aa");
        assert_eq!(a.archive_exe_path, "camoufox/camoufox");
    }

    /// The platform key must be the `<os>-<arch>` pair, not one or the other:
    /// macOS arm64 and macOS x86_64 are different artifacts.
    #[test]
    fn platform_key_pairs_os_and_arch() {
        let k = platform_key();
        assert!(k.starts_with(std::env::consts::OS), "{k}");
        assert!(k.ends_with(std::env::consts::ARCH), "{k}");
        assert!(k.contains('-'), "{k}");
    }
}
