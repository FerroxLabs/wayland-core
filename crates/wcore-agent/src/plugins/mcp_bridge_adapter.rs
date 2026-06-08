//! v0.6.5 Task 2.7 — MCP-bridge plugin → `InitializeOutcome` synthesizer.
//!
//! Dep-cycle escape (methodology #24): `wcore-plugin-subprocess` (which
//! owns `McpBridgePluginRunner`) cannot depend on `wcore-agent`, so
//! synthesis lives HERE.
//!
//! Unlike the WASM + Subprocess adapters, the MCP-bridge runner already
//! synthesizes `Vec<PluginTool>` itself (see Task 3.4's
//! `LoadedMcpBridgePlugin::tools()`) — the synthesized tools' closures
//! call back into `McpBridgePluginRunner::call_mcp_tool`. This adapter
//! is therefore a thin wrapper: it takes the already-prepared list and
//! lifts it into `CapturedPluginTool` entries with plugin provenance.

use wcore_plugin_subprocess::LoadedMcpBridgePlugin;

use crate::plugins::runner::{CapturedPluginTool, InitializeOutcome};

/// Synthesize an `InitializeOutcome` from a loaded MCP-bridge plugin.
///
/// The runner handle inside `loaded` is `Arc<McpBridgePluginRunner>`;
/// dropping the `LoadedMcpBridgePlugin` does NOT kill the subprocess
/// directly (the runner is reference-counted). Caller MUST stash a
/// runner handle for the plugin's lifetime and call `shutdown` on
/// teardown.
pub fn synthesize_initialize_outcome_mcp_bridge(
    loaded: LoadedMcpBridgePlugin,
    plugin_name: &str,
    tool_namespace: &str,
) -> InitializeOutcome {
    let mut outcome = InitializeOutcome::default();
    let (_runner_keepalive, tools) = loaded.into_parts();
    // Caller is responsible for keeping `_runner_keepalive` (or a clone)
    // alive for the plugin lifetime — see module docs. The closures
    // already-baked into each PluginTool capture their own runner
    // reference via the synthesizer in `wcore-plugin-subprocess`.

    for tool in tools {
        let fq_name = format!("{}::{}", tool_namespace, tool.name);
        outcome.tools.push(CapturedPluginTool {
            plugin: plugin_name.to_string(),
            fq_name,
            tool,
        });
    }
    outcome
}

#[cfg(test)]
mod tests {
    // Wave 6A.1 — placeholder removed; the synthesizer now has a real
    // production caller in `bootstrap.rs`.
}
