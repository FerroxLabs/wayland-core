//! G.2 — IJFW tool NAMESPACE CLAIMS in the `ijfw::` namespace.
//!
//! Two tool names: `ijfw_run` (route a query through the configured
//! IJFW mode pipeline) and `ijfw_update_apply` (apply an IJFW update
//! diff). Neither is implemented here. The IJFW MCP server advertises
//! both, and `wcore-mcp`'s tool proxy surfaces them through the normal
//! MCP path (see `mcp.rs`); nothing ever executes under the
//! `PluginTool`s registered below.
//!
//! That is why they are [`PluginTool::namespace_claim`] and not
//! `host_delegated`. The distinction is not cosmetic — `host_delegated`
//! means the tool runs somewhere, and the host keeps it in the tool
//! registry. Keeping these in the registry did real damage, because of
//! registration ORDER in `bootstrap.rs`:
//!
//!   1. `let builtin_names = registry.tool_names()` is snapshotted BEFORE
//!      any plugin tool is delivered;
//!   2. `apply_initialize_outcome` then delivers this inert `ijfw_run`;
//!   3. `register_mcp_tools` runs last, and its collision test is against
//!      that pre-plugin snapshot — so it does not see this `ijfw_run`,
//!      finds no collision, and registers the REAL tool under the same
//!      BARE name rather than `mcp__ijfw-memory__ijfw_run`;
//!   4. `ToolRegistry::register` pushes onto a `Vec` and `get()` returns
//!      the FIRST match, which is the inert one from step 2.
//!
//! So the claim did not merely sit beside the real tool — it SHADOWED
//! it, and every `ijfw_run` call returned an error while a working MCP
//! implementation sat unreachable behind it. `to_tool_defs()` also
//! advertised the name twice.
//!
//! The claim itself is still registered, and still worth registering:
//! it is what makes a second copy of this plugin trip the
//! `NamespaceLedger` duplicate-claim check. Only what the HOST does with
//! it at delivery changes — `deliver_tools` drops it before the registry.

use wcore_plugin_api::tool::PluginTool;
use wcore_plugin_api::{PluginContext, PluginResult};
use wcore_protocol::events::ToolCategory;

/// Tool names this plugin registers. The host-side `ScopedToolRegistry`
/// prefixes them with the manifest's `tool_namespace = "ijfw"` so the
/// fully-qualified names land as `ijfw::ijfw_run` and
/// `ijfw::ijfw_update_apply`.
pub const TOOL_NAMES: &[&str] = &["ijfw_run", "ijfw_update_apply"];

/// Build the namespace claim for one IJFW tool name. Behaviour is
/// delivered by the IJFW MCP server; this reserves the name and nothing
/// else.
fn ijfw_tool(name: &str) -> PluginTool {
    let description = match name {
        "ijfw_run" => "Route a query through the configured IJFW mode pipeline.",
        "ijfw_update_apply" => "Apply an IJFW update diff.",
        _ => "IJFW tool.",
    };
    PluginTool::namespace_claim(name, description, ToolCategory::Exec)
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
