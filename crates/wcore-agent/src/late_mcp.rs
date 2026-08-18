//! wayland#562 — late-binding for config MCP servers that connect AFTER boot.
//!
//! `AgentBootstrap::defer_config_mcp(true)` (used by the json-stream host, see
//! wayland#551) skips the config-declared MCP connect inside `build()` so a
//! slow or hung server cannot gate the host's `ready` frame. The caller
//! connects in the background and integrates the manager into the LIVE engine.
//!
//! Late tool REGISTRATION was already supported. Two boot-time consumers of
//! the config MCP manager were not, and this module is the seam that closes
//! both:
//!
//! 1. **MCP-provided skills.** `loader::load_catalog(.., mcp_manager)` pulls
//!    `skill://` resources at boot only. Under deferral the manager is `None`,
//!    so a json-stream session silently lost every skill served by a config
//!    MCP server while the (non-deferred) TUI kept them. [`LateMcpBinder`]
//!    merges them into the SHARED [`SkillCatalog`] — the same `Arc` held by
//!    the engine, `SkillTool` and the skill router — and injects the matching
//!    prompt listing so the model is actually told they exist.
//! 2. **Plugin hook dispatcher / `McpManagerCaller`.** The `plugin -> server`
//!    binding is resolved once at boot from the connected managers. With
//!    deferral the config server is absent, so a hook that should bind to it
//!    never binds, AND the F5/F6 two-server ambiguity gate evaluates against
//!    an incomplete server set. [`LateMcpBinder::bind`] re-runs
//!    [`resolve_server_for_plugin`] over boot managers PLUS every late one and
//!    reinstalls (or, on a newly ambiguous binding, REMOVES) the dispatcher.
//!
//! The async half ([`LateMcpBinder::skill_refs_for`]) is deliberately split
//! from the sync half ([`LateMcpBinder::bind`]): the CLI integrates a deferred
//! manager from a sync helper that may have to retry when the tool registry is
//! momentarily borrowed mid-turn, and that retry loop must not become async.

use std::collections::HashMap;
use std::sync::Arc;

use wcore_mcp::manager::McpManager;
use wcore_skills::refs::{SkillCatalog, SkillRef};

use crate::engine::AgentEngine;
use crate::hooks::{McpHookDispatcher, McpManagerCaller, resolve_server_for_plugin};
use crate::plugins::runner::PluginHook;

/// What one late-bind actually changed. Returned for logging/tests; the CLI
/// does not branch on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LateBindReport {
    /// Names of MCP skills merged into the live catalog (already-present
    /// names are skipped — see [`SkillCatalog::merge_late`]).
    pub skills_added: Vec<String>,
    /// True when a skills listing block was injected into the system prompt.
    pub prompt_updated: bool,
    /// The `plugin -> server` map installed on the engine after rebinding.
    /// Empty means no plugin resolved (or every candidate was ambiguous), in
    /// which case any previously installed dispatcher is REMOVED.
    pub hook_bindings: HashMap<String, String>,
    /// True when the hook dispatcher was re-evaluated at all (i.e. hook
    /// dispatch is enabled and at least one plugin registered a hook).
    pub hooks_rewired: bool,
}

/// Carries the boot-time state a late MCP connect needs in order to bind the
/// same surfaces boot would have bound.
pub struct LateMcpBinder {
    catalog: Arc<SkillCatalog>,
    /// plugin name -> its registered hook (tool) names. Snapshotted at boot
    /// from `AppliedPluginCapabilities::plugin_hooks`; plugins cannot register
    /// hooks after boot, so this never goes stale.
    hooks_by_plugin: HashMap<String, Vec<String>>,
    /// Every manager whose tools participate in hook resolution: the
    /// boot-connected ones, plus each late one as it binds.
    managers: Vec<Arc<McpManager>>,
    /// `config.hooks.dispatch_enabled` — the same gate bootstrap applies.
    dispatch_enabled: bool,
}

impl LateMcpBinder {
    pub fn new(
        catalog: Arc<SkillCatalog>,
        plugin_hooks: &[PluginHook],
        boot_managers: Vec<Arc<McpManager>>,
        dispatch_enabled: bool,
    ) -> Self {
        let mut hooks_by_plugin: HashMap<String, Vec<String>> = HashMap::new();
        for h in plugin_hooks {
            hooks_by_plugin
                .entry(h.plugin.clone())
                .or_default()
                .push(h.name.clone());
        }
        Self {
            catalog,
            hooks_by_plugin,
            managers: boot_managers,
            dispatch_enabled,
        }
    }

    /// Async half: read `skill://` resources off a freshly connected manager.
    ///
    /// Governance (revocation / promotion) is enforced inside
    /// `loader::load_mcp_skill_refs`, so a revoked skill cannot enter the
    /// session through this late door.
    pub async fn skill_refs_for(mgr: &McpManager) -> Vec<SkillRef> {
        wcore_skills::loader::load_mcp_skill_refs(mgr).await
    }

    /// Sync half: merge `refs` into the live catalog, tell the model about the
    /// new skills, and re-resolve the plugin hook dispatcher over the widened
    /// manager set.
    pub fn bind(
        &mut self,
        engine: &mut AgentEngine,
        mgr: Arc<McpManager>,
        refs: Vec<SkillRef>,
    ) -> LateBindReport {
        let mut report = LateBindReport::default();

        let added = self.catalog.merge_late(refs);
        report.skills_added = added.iter().map(|r| r.name.clone()).collect();
        if !added.is_empty() {
            // Hidden skills are not listed at boot either; mirror that filter
            // so the late block cannot advertise something the model must not
            // invoke.
            let listable: Vec<SkillRef> = added
                .into_iter()
                .filter(|r| !r.disable_model_invocation)
                .collect();
            // `None` budget matches the bootstrap call site; the boot listing
            // is built the same way.
            let block = crate::context::format_skills_section(&listable, None);
            if !block.is_empty() {
                // `inject_history` (not `set_system_prompt`) because this is a
                // framework fragment: it must also extend the retained rebind
                // base, or the first in-session `/model` / `/provider` rebind
                // would silently drop the late skills from the prompt again.
                engine.inject_history(block);
                report.prompt_updated = true;
            }
        }

        if !self.managers.iter().any(|m| Arc::ptr_eq(m, &mgr)) {
            self.managers.push(mgr);
        }

        if !self.dispatch_enabled || self.hooks_by_plugin.is_empty() {
            return report;
        }
        report.hooks_rewired = true;

        // Same shape bootstrap builds: server name -> advertised tool names.
        let mut servers: HashMap<String, Vec<String>> = HashMap::new();
        for m in &self.managers {
            for (server_name, tool) in m.all_tools() {
                servers
                    .entry(server_name.to_string())
                    .or_default()
                    .push(tool.name.to_string());
            }
        }
        let servers_view: Vec<(&str, Vec<&str>)> = servers
            .iter()
            .map(|(s, tools)| (s.as_str(), tools.iter().map(String::as_str).collect()))
            .collect();
        let hooks_view: HashMap<&str, Vec<&str>> = self
            .hooks_by_plugin
            .iter()
            .map(|(p, names)| (p.as_str(), names.iter().map(String::as_str).collect()))
            .collect();
        let server_for_plugin = resolve_server_for_plugin(&hooks_view, &servers_view);

        if server_for_plugin.is_empty() {
            // Every candidate is now unbound — either nothing matched, or the
            // late server made a previously unique binding AMBIGUOUS. In the
            // ambiguous case leaving the boot dispatcher installed would keep
            // routing to a binding the F5/F6 gate has just rejected, so it is
            // removed and those hooks fall back to log-only.
            tracing::info!(
                target: "wcore_agent::late_mcp",
                "late MCP bind resolved no plugin->server binding; hook dispatcher removed"
            );
            engine.clear_hook_dispatcher();
            return report;
        }

        let mut bound: Vec<&str> = server_for_plugin.keys().map(String::as_str).collect();
        bound.sort_unstable();
        tracing::info!(
            target: "wcore_agent::late_mcp",
            count = bound.len(),
            plugins = ?bound,
            "plugin hook dispatcher rewired after late MCP connect"
        );
        report.hook_bindings = server_for_plugin.clone();
        let caller = Arc::new(McpManagerCaller::new(self.managers.clone()));
        engine.set_hook_dispatcher(Arc::new(McpHookDispatcher::new(caller, server_for_plugin)));
        report
    }

    /// Test/inspection access to the shared catalog this binder merges into.
    pub fn catalog(&self) -> &Arc<SkillCatalog> {
        &self.catalog
    }
}
