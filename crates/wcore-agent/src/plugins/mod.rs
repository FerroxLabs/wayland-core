//! W2.5 plugin host glue.
//!
//! `PluginLoader` walks the api crate's compile-time `inventory` slot to find
//! built-in plugins; `PluginRunner` constructs each plugin's `PluginContext`
//! from real host adapters and runs `initialize()` with per-plugin error
//! containment; `DeferredPluginRegistry` stores plugins flagged
//! `deferred = true` for first-use wakeup in W7/W8.

pub mod adapters;
pub mod apply;
pub mod deferred;
pub mod host_supports;
pub mod loader;
pub mod mcp_bridge_adapter; // v0.6.5 Task 2.7 — synthesizer for MCP-bridge plugins
pub mod mcp_delivery;
pub mod runner;
pub mod sig_verifier;
pub mod skill_delivery;
pub mod subprocess_adapter; // v0.6.5 Task 2.7 — synthesizer for subprocess plugins
pub mod var_subst; // Lane D (G3) — ${CLAUDE_PLUGIN_ROOT|DATA}/${CLAUDE_PROJECT_DIR} subst
pub mod wasm_adapter; // v0.6.5 Task 2.7 — synthesizer for WASM plugins

pub use adapters::plugin_tool_adapter::PluginToolAdapter;
pub use apply::{AppliedPluginCapabilities, apply_initialize_outcome};
pub use deferred::DeferredPluginRegistry;
pub use loader::{DiscoveredPlugin, LoadedRuntimeHandle, PluginLoader};
pub use mcp_bridge_adapter::synthesize_initialize_outcome_mcp_bridge;
pub use runner::{CapturedPluginTool, InitializeOutcome, PluginRunner, ReifiedTool};
pub use subprocess_adapter::synthesize_initialize_outcome_subprocess;
pub use wasm_adapter::synthesize_initialize_outcome_wasm;
