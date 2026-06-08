//! Host-side adapters bridging each `wcore_plugin_api::registry::*` trait to
//! the underlying wcore registries.
//!
//! For W2.5 the adapters are in-memory collectors that record what each
//! plugin registered. The real `wcore_tools::registry::ToolRegistry` /
//! `wcore_config::hooks::HookEngine` / `wcore_skills::bundled::register_bundled_skill`
//! / `wcore_mcp::tool_proxy::register_mcp_tools` wiring lands in W7/W8 when
//! plugin-registered tools and MCP servers actually flow through the agent
//! dispatch loop. The smoke test in
//! `crates/wcore-agent/tests/plugin_api_smoke.rs` asserts each adapter
//! observed the registration; W8 swaps the in-memory collector for the live
//! registry without touching the api-crate surface.

pub mod agent_registrar;
// Wave BR: host-side browser-spec → real `BrowserTool` reifier.
pub mod browser_adapter;
// Wave CU: host-side CUA-spec → real `CuaTool` reifier.
pub mod cua_adapter;
pub mod hook_registrar;
pub mod mcp_registrar;
// v0.6.4 Task 1.1 — host-side `PluginTool` → real `wcore_tools::Tool`
// adapter. The only type that names both `PluginTool` and
// `wcore_tools::Tool`; legal here because `wcore-agent` may depend on
// `wcore-tools`.
pub mod plugin_tool_adapter;
pub mod provider_registrar;
pub mod rule_registrar;
pub mod skill_registrar;
pub mod tool_registrar;
// v0.6.4 Task 2.1: host-side capture for plugin-registered user-model
// backends (Honcho et al.). Reification into a live client lands in Task 2.2.
pub mod user_model_registrar;
