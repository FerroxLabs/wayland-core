//! `PluginContext` — the single boundary object every plugin receives in
//! `initialize()`.
//!
//! All capability registrations flow through this struct. The `'a` lifetime
//! ties the context to the host's borrow window — plugins cannot retain
//! references past `initialize()` returning. Design spec §5.17.

use crate::manifest::PluginManifest;
use crate::registry::agents::ScopedAgentRegistry;
use crate::registry::browser::ScopedBrowserRegistry;
use crate::registry::config::ScopedConfigReader;
use crate::registry::cua::ScopedCuaRegistry;
use crate::registry::hooks::ScopedHookRegistry;
use crate::registry::logger::ScopedPluginLogger;
use crate::registry::mcp::ScopedMcpRegistry;
use crate::registry::memory::ScopedMemoryClient;
use crate::registry::providers::ScopedProviderRegistry;
use crate::registry::rules::ScopedRuleRegistry;
use crate::registry::skills::ScopedSkillRegistry;
use crate::registry::tools::ScopedToolRegistry;
use crate::registry::user_models::ScopedUserModelRegistry;

pub struct PluginContext<'a> {
    pub manifest: &'a PluginManifest,

    /// Permission-gated registries — populated only when the corresponding
    /// `register_*` flag in the manifest is true (and the host adapter has
    /// the underlying surface available).
    pub tools: Option<ScopedToolRegistry<'a>>,
    pub hooks: Option<ScopedHookRegistry<'a>>,
    pub agents: Option<ScopedAgentRegistry<'a>>,
    pub skills: Option<ScopedSkillRegistry<'a>>,
    pub rules: Option<ScopedRuleRegistry<'a>>,
    pub mcp_servers: Option<ScopedMcpRegistry<'a>>,
    pub providers: Option<ScopedProviderRegistry<'a>>,
    /// Wave BR — wayland-browser plugin reifies its `BrowserToolSpec` via
    /// this registry. Host (wcore-agent) builds a real `BrowserTool` via
    /// `wcore_browser::adapter::from_spec` after `initialize()` returns.
    pub browser: Option<ScopedBrowserRegistry<'a>>,
    /// Wave CU — wayland-cua plugin reifies its `CuaToolSpec` via this
    /// registry. Host (wcore-agent) builds a real `CuaTool` via
    /// `wcore_cua::adapter::from_spec` after `initialize()` returns.
    pub cua: Option<ScopedCuaRegistry<'a>>,
    /// v0.6.4 Task 2.1 — wayland-honcho (and future user-model backend
    /// plugins) reify a `UserModelSpec` via this registry. Host
    /// (wcore-agent) constructs a real backend client after
    /// `initialize()` returns (Task 2.2).
    pub user_models: Option<ScopedUserModelRegistry<'a>>,
    /// M5.1 — container-isolated tool execution. Plugins that need a
    /// `requires_sandbox = true` tool ask the host for a `SandboxRegistry`
    /// handle; absent means no sandbox surface is available in this host
    /// (e.g. CI runner without a docker daemon).
    pub sandbox: Option<std::sync::Arc<wcore_sandbox::SandboxRegistry>>,

    /// Always-on observability + read views. Permission for memory access
    /// lives inside the client (per-partition), not at construction time.
    pub config: ScopedConfigReader<'a>,
    pub logger: ScopedPluginLogger<'a>,
    pub memory: ScopedMemoryClient<'a>,
}
