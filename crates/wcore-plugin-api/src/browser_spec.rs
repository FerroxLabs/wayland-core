//! Mirror of `wcore_browser::BrowserTool` registration shape (REV-2 audit
//! F2). The host adapter (in `wcore-agent`, which DOES depend on
//! `wcore-browser`) translates this into a concrete `BrowserTool` at boot
//! time. Pattern: see `BundledSkillSpec` for the analogous skills mirror.
//!
//! `wayland-browser` (the plugin shell) cannot depend on `wcore-browser`
//! because the `FORBIDDEN_CORE_IMPORTS` build.rs lint forbids it — plugin
//! crates may depend ONLY on `wcore-plugin-api`, `wcore-types`,
//! `wcore-protocol` + external deps. The mirror lets the plugin describe
//! its desired browser-tool configuration in api-crate-local terms.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BrowserToolSpec {
    /// Tool namespace this plugin claims — e.g. `"Browser"`. Mirrors the
    /// `PluginPermissions::tool_namespace` requirement for `register_tools`.
    pub tool_namespace: String,
    pub preferred_provider: BrowserProviderHint,
    pub policy: BrowserPolicySpec,
    /// When true, allow falling back to a cloud provider (Browserbase).
    /// Env creds still gate the actual selection.
    #[serde(default)]
    pub allow_cloud: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProviderHint {
    #[default]
    Auto,
    Camoufox,
    Chromium,
    Browserbase,
}

/// Serde-friendly mirror of `wcore_browser::BrowserPolicy`.
///
/// `default_action` defaults to `"deny"` (fail-closed) since v0.2.1 —
/// matches `wcore_browser::PolicyAction::default()`. Pre-v0.2.1 this
/// defaulted to `"allow"` which was a fail-open SSRF risk (see
/// `STABILITY-v0.2.0.md` MAJOR #6).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BrowserPolicySpec {
    /// `"deny"` (default) | `"allow"` | `"ask"`.
    pub default_action: String,
    pub allowed_origins: Vec<String>,
    pub denied_origins: Vec<String>,
}

impl Default for BrowserPolicySpec {
    fn default() -> Self {
        Self {
            // Fail-closed: matches `wcore_browser::PolicyAction::default()`.
            default_action: "deny".into(),
            allowed_origins: Vec::new(),
            denied_origins: Vec::new(),
        }
    }
}

/// Op-shape mirror — kept as a thin marker for now. The plugin registers
/// the tool by namespace; runtime ops flow through the host's `BrowserTool`
/// which owns the real `BrowserOp` enum. This struct exists so future
/// per-op gating can be expressed in plugin manifests without forcing the
/// api crate to depend on `wcore-browser`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BrowserOpSpec {
    /// Optional allow-list of op `kind` names. Empty == all v1 ops allowed.
    pub allowed_kinds: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serde() {
        let spec = BrowserToolSpec {
            tool_namespace: "Browser".into(),
            preferred_provider: BrowserProviderHint::Camoufox,
            policy: BrowserPolicySpec {
                default_action: "deny".into(),
                allowed_origins: vec!["*.example.com".into()],
                denied_origins: vec!["*.evil.example".into()],
            },
            allow_cloud: false,
        };
        let s = serde_json::to_string(&spec).unwrap();
        let parsed: BrowserToolSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.tool_namespace, "Browser");
        assert_eq!(parsed.preferred_provider, BrowserProviderHint::Camoufox);
        assert_eq!(parsed.policy.allowed_origins.len(), 1);
    }

    #[test]
    fn provider_hint_default_is_auto() {
        assert_eq!(BrowserProviderHint::default(), BrowserProviderHint::Auto);
    }
}
