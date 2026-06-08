//! G.3 — 5 IJFW lifecycle hooks.
//!
//! Each registration is a `(phase, name)` pair recorded in the host-side
//! [`HostHookRegistrar`]. The actual hook implementation lives behind
//! the IJFW MCP server; W4 routes the hook engine into the plugin
//! crate when it fires.

use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_plugin_api::{PluginContext, PluginResult};

/// The 5 IJFW lifecycle hooks per design spec §5.17 + the plan's G.3
/// enumeration. Each `(phase, name)` is a contract the host adapter
/// dispatches to at the matching point in the agent lifecycle.
pub const HOOKS: &[(HookPhase, &str)] = &[
    (HookPhase::SessionStart, "ijfw_memory_prelude"),
    (HookPhase::PostToolUse, "ijfw_observation_capture"),
    (HookPhase::PrePrompt, "ijfw_pre_prompt_recall"),
    (HookPhase::PreCompact, "ijfw_pre_compact_optimize"),
    (HookPhase::SessionEnd, "ijfw_session_summarize"),
];

/// Register the 5 IJFW hooks through `ctx.hooks`. Manifest declares
/// `register_hooks = true`, so the registry must be present.
pub fn register(ctx: &mut PluginContext<'_>) -> PluginResult<()> {
    // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
    let registry =
        ctx.hooks
            .as_mut()
            .ok_or_else(|| wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-ijfw".into(),
                surface: "hooks".into(),
            })?;
    for (phase, name) in HOOKS {
        registry.register_hook(*phase, (*name).to_string())?;
    }
    Ok(())
}
