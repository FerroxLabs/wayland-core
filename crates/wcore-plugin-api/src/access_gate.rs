//! Permission check mirroring `MemoryAccessGate` (M2). Every `Scoped*Registry`
//! constructor calls `PluginAccessGate::require_*` before handing out the
//! scoped registry. Design spec §5.17.

use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;

pub struct PluginAccessGate;

impl PluginAccessGate {
    pub fn require_tools(m: &PluginManifest) -> PluginResult<()> {
        if !m.permissions.register_tools {
            return Err(PluginError::PermissionDenied {
                plugin: m.plugin.name.clone(),
                operation: "register_tools".into(),
            });
        }
        // `register_tools` requires `tool_namespace` — already enforced by
        // manifest validation, but re-checked here so callers get the
        // diagnostic at the right call site.
        if m.permissions.tool_namespace.is_none() {
            return Err(PluginError::NamespaceMissing {
                plugin: m.plugin.name.clone(),
                namespace: String::new(),
            });
        }
        Ok(())
    }

    pub fn require_hooks(m: &PluginManifest) -> PluginResult<()> {
        require_flag(m, m.permissions.register_hooks, "register_hooks")
    }
    pub fn require_agents(m: &PluginManifest) -> PluginResult<()> {
        require_flag(m, m.permissions.register_agents, "register_agents")
    }
    pub fn require_skills(m: &PluginManifest) -> PluginResult<()> {
        require_flag(m, m.permissions.register_skills, "register_skills")
    }
    pub fn require_rules(m: &PluginManifest) -> PluginResult<()> {
        require_flag(m, m.permissions.register_rules, "register_rules")
    }
    pub fn require_mcp_server(m: &PluginManifest) -> PluginResult<()> {
        require_flag(m, m.permissions.register_mcp_server, "register_mcp_server")
    }
    pub fn require_providers(m: &PluginManifest) -> PluginResult<()> {
        require_flag(m, m.permissions.register_providers, "register_providers")
    }
    /// v0.6.4 Task 2.1 — gate for `ScopedUserModelRegistry`.
    pub fn require_user_models(m: &PluginManifest) -> PluginResult<()> {
        require_flag(
            m,
            m.permissions.register_user_models,
            "register_user_models",
        )
    }
}

fn require_flag(m: &PluginManifest, flag: bool, op: &'static str) -> PluginResult<()> {
    if !flag {
        return Err(PluginError::PermissionDenied {
            plugin: m.plugin.name.clone(),
            operation: op.into(),
        });
    }
    Ok(())
}
