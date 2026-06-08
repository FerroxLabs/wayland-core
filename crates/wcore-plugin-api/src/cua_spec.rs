//! Mirror of `wcore_cua::CuaTool` registration shape (REV-2 audit F2).
//! The host adapter (in `wcore-cua` itself, or `wcore-agent` once plugin
//! wiring lands) translates this into a concrete `CuaTool` at boot time.
//!
//! Pattern: see `browser_spec::BrowserToolSpec` for the analogous
//! browser mirror, and `BundledSkillSpec` for the skills mirror.
//!
//! `wayland-cua` (the plugin shell) cannot depend on `wcore-cua`
//! because the `FORBIDDEN_CORE_IMPORTS` build.rs lint forbids it —
//! plugin crates may depend ONLY on `wcore-plugin-api`, `wcore-types`,
//! `wcore-protocol` + external deps. The mirror lets the plugin
//! describe its desired CUA-tool configuration in api-crate-local
//! terms.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CuaToolSpec {
    /// Tool namespace this plugin claims — e.g. `"Cua"`. Mirrors the
    /// `PluginPermissions::tool_namespace` requirement for `register_tools`.
    pub tool_namespace: String,
    pub policy: CuaPolicySpec,
    /// When `true`, the host's screenshot result will be passed through
    /// the sensitive-content blur pass before being returned to the agent.
    #[serde(default)]
    pub redact_screenshots: bool,
}

impl Default for CuaToolSpec {
    fn default() -> Self {
        Self {
            tool_namespace: "Cua".into(),
            policy: CuaPolicySpec::default(),
            redact_screenshots: false,
        }
    }
}

/// Serde-friendly mirror of `wcore_cua::CuaPolicy`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CuaPolicySpec {
    /// Apps that require HITL approval per op (route to Suspend).
    pub require_approval_for_app: Vec<String>,
    /// Apps the agent cannot drive at all.
    pub forbidden_apps: Vec<String>,
    /// Key combinations rejected outright (case-insensitive).
    pub forbidden_key_combos: Vec<String>,
    /// When `true`, the first op against a new app routes to Suspend.
    pub first_time_per_app_approval: bool,
}

/// Op-shape mirror — kept as a thin marker for now. Plugin registers
/// the tool by namespace; runtime ops flow through the host's `CuaTool`
/// which owns the real `CuaOp` enum. This struct lets future per-op
/// gating ride on plugin manifests without leaking `wcore-cua` into
/// the api crate.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CuaOpSpec {
    /// Optional allow-list of op `kind` names. Empty == all v1 ops.
    pub allowed_kinds: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serde() {
        let spec = CuaToolSpec {
            tool_namespace: "Cua".into(),
            policy: CuaPolicySpec {
                require_approval_for_app: vec!["Keychain Access".into()],
                forbidden_apps: vec!["1Password".into()],
                forbidden_key_combos: vec!["cmd+q+system".into()],
                first_time_per_app_approval: true,
            },
            redact_screenshots: true,
        };
        let s = serde_json::to_string(&spec).unwrap();
        let parsed: CuaToolSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.tool_namespace, "Cua");
        assert_eq!(parsed.policy.forbidden_apps, vec!["1Password".to_string()]);
        assert!(parsed.redact_screenshots);
        assert!(parsed.policy.first_time_per_app_approval);
    }

    #[test]
    fn default_namespace_is_cua() {
        assert_eq!(CuaToolSpec::default().tool_namespace, "Cua");
    }
}
