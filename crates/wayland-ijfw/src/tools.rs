//! G.2 — IJFW tools registered into the `ijfw::` namespace.
//!
//! Two tool names: `ijfw_run` (route a query through the configured
//! IJFW mode pipeline) and `ijfw_update_apply` (apply an IJFW update
//! diff). Their bodies delegate to the registered IJFW MCP server at
//! runtime — the IJFW MCP server itself advertises these tools and
//! `wcore-mcp`'s tool proxy surfaces them through the normal MCP path
//! (see `mcp.rs`). The `PluginTool` registered here is therefore a
//! host-delegated namespace claim: it carries honest metadata, and its
//! closure is never the live execution path.

use wcore_plugin_api::tool::PluginTool;
use wcore_plugin_api::{PluginContext, PluginResult};
use wcore_protocol::events::ToolCategory;

/// Tool names this plugin registers. The host-side `ScopedToolRegistry`
/// prefixes them with the manifest's `tool_namespace = "ijfw"` so the
/// fully-qualified names land as `ijfw::ijfw_run` and
/// `ijfw::ijfw_update_apply`.
pub const TOOL_NAMES: &[&str] = &["ijfw_run", "ijfw_update_apply"];

/// Build the `PluginTool` for one IJFW tool name. Behavior is delivered
/// by the IJFW MCP server; this carries metadata + a host-delegated
/// closure.
fn ijfw_tool(name: &str) -> PluginTool {
    let description = match name {
        "ijfw_run" => "Route a query through the configured IJFW mode pipeline.",
        "ijfw_update_apply" => "Apply an IJFW update diff.",
        _ => "IJFW tool.",
    };
    PluginTool::host_delegated(name, description, ToolCategory::Exec)
}

/// Register all IJFW tools through `ctx.tools`. Manifest declares
/// `register_tools = true`, so the registry must be present.
pub fn register(ctx: &mut PluginContext<'_>) -> PluginResult<()> {
    // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
    let registry =
        ctx.tools
            .as_mut()
            .ok_or_else(|| wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-ijfw".into(),
                surface: "tools".into(),
            })?;
    for name in TOOL_NAMES {
        registry.register_tool(ijfw_tool(name))?;
    }
    Ok(())
}
