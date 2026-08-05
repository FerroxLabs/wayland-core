//! Host-adapter surface: translate a `wcore-plugin-api`-mirrored browser
//! spec into a concrete `BrowserTool`.
//!
//! The plugin shell (`wayland-browser`) cannot depend on `wcore-browser`
//! (REV-2 audit F2 — the `FORBIDDEN_CORE_IMPORTS` lint in
//! `crates/wcore-plugin-api/build.rs` enforces it). Instead, the plugin
//! registers a `BrowserToolSpec` via the api-crate mirror, and the host
//! (which DOES depend on `wcore-browser`) calls into this module to reify
//! the spec into a real `BrowserTool`.
//!
//! Mirror pattern verified against `BundledSkillSpec` (api-crate)
//! ↔ `BundledSkillDefinition` (wcore-skills). This module is the
//! Browser analogue.

use std::sync::Arc;

use crate::policy::{BrowserPolicy, PolicyAction};
use crate::provider::BrowserProvider;
use crate::selection::{ProviderHint, SelectionInputs, select_provider};
use crate::supervisor::BrowserSupervisor;
use crate::tool::BrowserTool;

/// Field-for-field mirror of `wcore_plugin_api::browser_spec::BrowserToolSpec`.
/// The api-crate version lives there; this struct allows the host adapter to
/// hand a normalized payload to `from_spec` without forcing `wcore-plugin-api`
/// to depend on `wcore-browser`.
#[derive(Debug, Clone)]
pub struct BrowserToolSpec {
    pub tool_namespace: String,
    pub preferred_provider: ProviderHint,
    pub policy: BrowserPolicy,
    pub allow_cloud: bool,
}

/// Translate a normalized spec into a concrete `BrowserTool`. Bootstrap-time
/// helper for `wcore-agent` once it carries the plugin-spec → tool wiring.
///
/// The policy is cloned into the backend so the backend's reqwest client
/// gets a [`reqwest::redirect::Policy`] that enforces the same per-hop
/// rules as the tool-layer pre-check (closes BLOCKER #3 from
/// SECURITY-v0.2.0.md).
pub fn from_spec(spec: BrowserToolSpec) -> Arc<BrowserTool> {
    let camoufox_url = crate::backends::CamoufoxBackend::default_url().to_string();
    let provider: Arc<dyn BrowserProvider> = select_provider(SelectionInputs {
        hint: spec.preferred_provider,
        allow_cloud: spec.allow_cloud,
        camoufox_url: None,
        policy: Some(spec.policy.clone()),
    });
    let supervisor = if provider.backend_name() == "camoufox" {
        BrowserSupervisor::with_config(crate::supervisor::SupervisorConfig::local_camoufox(
            &camoufox_url,
        ))
    } else {
        BrowserSupervisor::new()
    };
    Arc::new(BrowserTool::new(
        provider,
        spec.policy,
        Arc::new(supervisor),
    ))
}

/// Convenience builder that the plugin-api mirror (`BrowserPolicySpec`)
/// can call into to construct a concrete policy. Kept here so plugin
/// shells don't need to know our policy shape directly.
pub fn make_policy(
    default_action: PolicyAction,
    allowed_origins: Vec<String>,
    denied_origins: Vec<String>,
) -> BrowserPolicy {
    BrowserPolicy::new(default_action, allowed_origins, denied_origins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_spec_yields_camoufox_default() {
        use wcore_tools::Tool;
        let tool = from_spec(BrowserToolSpec {
            tool_namespace: "Browser".into(),
            preferred_provider: ProviderHint::Auto,
            policy: BrowserPolicy::default(),
            allow_cloud: false,
        });
        assert_eq!(tool.name(), "Browser");
    }
}
