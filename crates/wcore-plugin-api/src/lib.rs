//! `wcore-plugin-api` — the isolation boundary for the wcore plugin architecture.
//!
//! Plugins import from this crate and only this crate. This crate has zero
//! dependencies on `wcore-agent`, `wcore-tools`, `wcore-mcp`, `wcore-skills`,
//! `wcore-memory`, `wcore-config`, or `wcore-providers`. That isolation is the
//! whole point and is enforced by `build.rs`.
//!
//! See `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md` §5.17 for
//! the design contract; `docs/superpowers/plans/2026-05-14-wcore-W2.5-plugin-api.md`
//! for the implementation plan; and `AGENTS.md` for crate-graph rules.

pub mod access_gate;
pub mod context;
pub mod error;
pub mod manifest;
pub mod plugin;
/// Host-side registrar and scoped-registry traits. Plugin authors normally
/// import the `Scoped*Registry` types via [`PluginContext`]; the bare
/// `*Registrar` traits and host-only utilities (`NamespaceLedger`) are
/// re-exported here for the engine's own use. Hidden from rustdoc to keep
/// the plugin-facing surface focused on the top-level re-exports below.
#[doc(hidden)]
pub mod registry;

// Value-type mirrors of host-side types so plugins can reference them without
// the api crate having to pull `wcore-agent` / `wcore-skills` / `wcore-mcp` /
// `wcore-memory` / `wcore-browser` deps. Host adapters translate.
pub mod agent_manifest;
// W8c.1 E.13 — `BrowserToolSpec` mirror of `wcore_browser::BrowserTool`
// registration shape. Plugin shells (wayland-browser) describe their
// desired browser surface against this api-crate-local type; the host
// adapter in wcore-agent translates into a concrete `BrowserTool`.
pub mod browser_spec;
pub mod bundled_skill_spec;
// W8c.2 F.8 — `CuaToolSpec` mirror of `wcore_cua::CuaTool` registration
// shape. Plugin shells (wayland-cua) describe their desired CUA surface
// against this api-crate-local type; the host adapter in wcore-cua
// translates into a concrete `CuaTool`.
pub mod cua_spec;
pub mod mcp_server_spec;
pub mod memory_spec;
pub mod rule_spec;
// v0.6.4 Task 1.1 — `PluginTool`: the plugin-api-native tool contract.
// A plugin delivers a tool as a `PluginTool` (metadata + execution
// closure typed in allowed terms); the host adapter in `wcore-agent`
// reifies it into a real `wcore_tools::Tool`. Nothing here names
// `wcore-tools` (a FORBIDDEN_CORE_IMPORTS member).
pub mod tool;
// v0.6.4 Task 2.1 — `UserModelSpec`: plain-data contract for
// plugin-registered user-model backends (e.g. Honcho). The host
// adapter in `wcore-agent` reads these fields and constructs the real
// client. Nothing here names `wayland-honcho`.
pub mod user_model_spec;

// Top-level re-exports for the public surface plugins will write against.
pub use access_gate::PluginAccessGate;
pub use context::PluginContext;
pub use error::{PluginError, PluginResult};
pub use manifest::{
    PluginCapabilities, PluginIdentity, PluginInfo, PluginManifest, PluginPermissions,
};
pub use plugin::{Plugin, PluginFactory};

/// Plugin API semver version. A plugin's `plugin_api_version` field must
/// equal this string to load. The `from_toml_str` parser checks the field
/// against this constant; mismatches return a typed error.
pub const PLUGIN_API_VERSION: &str = "1.0";

pub use agent_manifest::AgentManifest;
pub use browser_spec::{BrowserOpSpec, BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec};
pub use bundled_skill_spec::BundledSkillSpec;
pub use cua_spec::{CuaOpSpec, CuaPolicySpec, CuaToolSpec};
pub use mcp_server_spec::{McpServerSpec, McpTransport};
pub use memory_spec::{MemoryItem, MemoryQuery, Partition};
pub use rule_spec::{RuleScope, RuleSpec};
pub use tool::{PluginTool, PluginToolCaps, PluginToolEmit, PluginToolInvocation};
pub use user_model_spec::UserModelSpec;

// Re-exported so plugin authors don't need to add separate `async-trait` and
// `inventory` deps — they get the exact versions the api crate is pinned to.
pub use async_trait::async_trait;
pub use inventory::{self, submit};

/// Submodule for the `inventory` discovery surface. Re-exported so the host
/// crate (`wcore-agent`) does not need to depend on `inventory` directly.
pub mod plugin_inventory {
    use crate::plugin::PluginFactory;

    inventory::collect!(&'static dyn PluginFactory);

    pub fn iter() -> impl Iterator<Item = &'static dyn PluginFactory> {
        inventory::iter::<&'static dyn PluginFactory>().copied()
    }
}
