//! Tool re-entry host capability — `Deny*` default + `Gated*` impl.
//!
//! Lets a WASM plugin invoke other host-registered tools by alias. The
//! `ToolRegistry` is a placeholder seam; Task 2.6 will wire the real
//! `wcore_tools` registry through.

use std::sync::Arc;

use wcore_plugin_api::access_gate::PluginAccessGate;

/// Placeholder seam for the host tool registry. Task 2.6 will swap this for
/// the real `wcore_tools::ToolRegistry`.
#[derive(Debug, Default)]
pub struct ToolRegistry;

pub trait WaylandHostTools: Send + Sync {
    fn tool_invoke(&self, alias: &str, params_json: &str) -> Result<String, String>;
}

/// Fail-closed tool host. Every invocation denied.
#[derive(Debug, Default)]
pub struct DenyHostTools;

impl WaylandHostTools for DenyHostTools {
    fn tool_invoke(&self, _alias: &str, _params_json: &str) -> Result<String, String> {
        Err("permission denied: tool-invoke".into())
    }
}

/// Gated tool host.
pub struct GatedHostTools {
    #[allow(dead_code)]
    gate: Arc<PluginAccessGate>,
    #[allow(dead_code)]
    plugin: String,
    #[allow(dead_code)]
    registry: Arc<ToolRegistry>,
    permitted: bool,
}

impl GatedHostTools {
    pub fn new(
        gate: Arc<PluginAccessGate>,
        plugin: String,
        registry: Arc<ToolRegistry>,
        permitted: bool,
    ) -> Self {
        Self {
            gate,
            plugin,
            registry,
            permitted,
        }
    }
}

impl WaylandHostTools for GatedHostTools {
    fn tool_invoke(&self, alias: &str, _params_json: &str) -> Result<String, String> {
        if !self.permitted {
            return Err("permission denied: tool-invoke".into());
        }
        tracing::debug!(plugin = %self.plugin, %alias, "gated tool_invoke: gate ok, registry not wired (Task 2.6)");
        // Real registry dispatch lands in Task 2.6. The GATING decision has
        // happened — surface an explicit not-yet-wired error.
        Err("tool-invoke: not yet wired (Task 2.6 closes)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_tools_returns_err() {
        let d = DenyHostTools;
        let res = d.tool_invoke("foo.bar", "{}");
        assert!(matches!(res, Err(ref m) if m == "permission denied: tool-invoke"));
    }

    #[test]
    fn gated_tools_without_permission_denies() {
        let t = GatedHostTools::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            Arc::new(ToolRegistry),
            false,
        );
        let res = t.tool_invoke("foo.bar", "{}");
        assert!(matches!(res, Err(ref m) if m == "permission denied: tool-invoke"));
    }

    #[test]
    fn gated_tools_with_permission_passes_gate() {
        let t = GatedHostTools::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            Arc::new(ToolRegistry),
            true,
        );
        let res = t.tool_invoke("foo.bar", "{}");
        assert!(matches!(res, Err(ref m) if m.contains("not yet wired")));
    }
}
