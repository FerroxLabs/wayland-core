//! wayland#562 — late binding of the config MCP servers that boot deferred.
//!
//! wayland#551 moved the config-declared MCP connect out of
//! [`crate::bootstrap::AgentBootstrap::build`] so a slow or hung server can no
//! longer gate the json-stream `ready` frame. Tool registration already
//! late-binds (the CLI re-runs `register_mcp_tools` against the live
//! registry), but two boot-time consumers of the config manager did not:
//!
//! 1. **MCP-provided skills.** [`wcore_skills::loader::load_catalog`] reads
//!    `skill://` resources from the connected servers at boot only. Under
//!    deferral the manager is `None`, so a json-stream session silently ran
//!    without every skill served over MCP — while the TUI (which never
//!    defers) loaded them.
//! 2. **The plugin hook dispatcher.** `resolve_server_for_plugin` ran against
//!    the boot manager set. With deferral that set omits every config server,
//!    so a hook that would bind to one stayed log-only, and the F5/F6
//!    two-server ambiguity gate decided without the config server present.
//!
//! [`LateMcpBinder`] closes both by replaying the exact boot computation once
//! the background connect settles. It is not an incremental merge: the skill
//! catalog is re-derived from the same inputs boot used, with the connected
//! manager supplied, so bundled/MCP/user/project precedence and the
//! prioritizer order are whatever boot would have produced. The hook binding
//! is likewise re-resolved over *all* managers, so the ambiguity gate sees
//! the config server it was previously blind to.
//!
//! The binder runs before the first provider turn (the CLI settles the
//! deferred connect at the `Message` boundary), so the rebuilt system prompt
//! is the one the first request carries — no prompt-cache churn mid-session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use wcore_mcp::manager::McpManager;

use crate::engine::AgentEngine;

/// What a late bind actually changed. Empty fields mean "nothing to do" —
/// the common case when the deferred servers serve neither skills nor
/// hook-named tools.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LateBindOutcome {
    /// Skill names present after the rebind that were absent before.
    pub skills_added: Vec<String>,
    /// Plugins whose lifecycle hooks now resolve to an MCP server.
    pub hook_plugins_bound: Vec<String>,
}

impl LateBindOutcome {
    pub fn is_empty(&self) -> bool {
        self.skills_added.is_empty() && self.hook_plugins_bound.is_empty()
    }
}

/// Captured boot inputs needed to re-derive the skill catalog and the plugin
/// hook binding once a deferred config MCP manager connects.
///
/// Constructed by `AgentBootstrap::build` only when the caller asked for
/// deferral; a non-deferring session already saw the manager at boot and has
/// nothing to rebind.
pub struct LateMcpBinder {
    cwd: PathBuf,
    extra_skill_dirs: Vec<PathBuf>,
    bundled_catalog: Arc<wcore_skills::bundled::BundledSkillCatalog>,
    /// `Some` only when `[memory] enabled` is on — matching the boot-time
    /// gate on the `SkillPrioritizer` reorder.
    memory_api: Option<Arc<dyn wcore_memory::MemoryApi>>,
    /// plugin name -> its registered lifecycle hook (tool) names.
    hooks_by_plugin: HashMap<String, Vec<String>>,
    /// Managers already known at boot (plugin-supplied MCP servers, and any
    /// config server that was NOT deferred). The late resolution runs over
    /// these plus the newly connected manager.
    boot_managers: Vec<Arc<McpManager>>,
    hook_dispatch_enabled: bool,
}

impl LateMcpBinder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cwd: PathBuf,
        extra_skill_dirs: Vec<PathBuf>,
        bundled_catalog: Arc<wcore_skills::bundled::BundledSkillCatalog>,
        memory_api: Option<Arc<dyn wcore_memory::MemoryApi>>,
        hooks_by_plugin: HashMap<String, Vec<String>>,
        boot_managers: Vec<Arc<McpManager>>,
        hook_dispatch_enabled: bool,
    ) -> Self {
        Self {
            cwd,
            extra_skill_dirs,
            bundled_catalog,
            memory_api,
            hooks_by_plugin,
            boot_managers,
            hook_dispatch_enabled,
        }
    }

    /// Rebind `engine` against a config MCP `manager` that connected after
    /// boot. Idempotent and best-effort: when the manager serves no skills
    /// and no hook-named tools, nothing is touched and the returned outcome
    /// is empty.
    pub async fn bind(
        &self,
        engine: &mut AgentEngine,
        manager: Arc<McpManager>,
    ) -> LateBindOutcome {
        let mut outcome = LateBindOutcome::default();
        self.bind_skills(engine, &manager, &mut outcome).await;
        self.bind_hooks(engine, &manager, &mut outcome);
        outcome
    }

    async fn bind_skills(
        &self,
        engine: &mut AgentEngine,
        manager: &McpManager,
        outcome: &mut LateBindOutcome,
    ) {
        let Some(catalog) = engine.skill_catalog().cloned() else {
            return;
        };
        let refs = crate::bootstrap::load_session_skill_refs(
            &self.cwd,
            &self.extra_skill_dirs,
            Some(manager),
            &self.bundled_catalog,
            self.memory_api.as_ref(),
        )
        .await;

        let before: Vec<String> = catalog.iter_names().collect();
        let after: Vec<String> = refs.iter().map(|r| r.name.clone()).collect();
        if before == after {
            // Nothing the MCP servers contributed changed the catalog. Leave
            // the prompt byte-identical so the cached prefix survives.
            return;
        }
        outcome.skills_added = after
            .iter()
            .filter(|name| !before.contains(name))
            .cloned()
            .collect();

        let section = crate::context::skills_reminder_section(&refs, None);
        catalog.replace_refs(refs);
        engine.swap_skill_listing_section(section);
        tracing::info!(
            target: "wcore_agent::late_mcp",
            added = outcome.skills_added.len(),
            "wayland#562: skill catalog rebound after deferred MCP connect"
        );
    }

    fn bind_hooks(
        &self,
        engine: &mut AgentEngine,
        manager: &Arc<McpManager>,
        outcome: &mut LateBindOutcome,
    ) {
        if !self.hook_dispatch_enabled || self.hooks_by_plugin.is_empty() {
            return;
        }
        let mut managers = self.boot_managers.clone();
        managers.push(Arc::clone(manager));

        // Same shape bootstrap builds: every connected server with the tool
        // names it advertises, fed to the shared F5/F6 ambiguity gate.
        let mut servers: HashMap<String, Vec<String>> = HashMap::new();
        for mgr in &managers {
            for (server_name, tool) in mgr.all_tools() {
                servers.entry(server_name).or_default().push(tool.name);
            }
        }
        let servers_view: Vec<(&str, Vec<&str>)> = servers
            .iter()
            .map(|(s, tools)| (s.as_str(), tools.iter().map(String::as_str).collect()))
            .collect();
        let hooks_view: HashMap<&str, Vec<&str>> = self
            .hooks_by_plugin
            .iter()
            .map(|(plugin, hooks)| (plugin.as_str(), hooks.iter().map(String::as_str).collect()))
            .collect();

        let server_for_plugin = crate::hooks::resolve_server_for_plugin(&hooks_view, &servers_view);
        if server_for_plugin.is_empty() {
            // Identical to a non-deferred boot that resolved nothing: no
            // dispatcher is installed, so plugin hooks stay log-only and the
            // durable tool-hook authority is unchanged.
            return;
        }
        let mut bound: Vec<String> = server_for_plugin.keys().cloned().collect();
        bound.sort();
        let caller = Arc::new(crate::hooks::McpManagerCaller::new(managers));
        engine.set_hook_dispatcher(Arc::new(crate::hooks::McpHookDispatcher::new(
            caller,
            server_for_plugin,
        )));
        tracing::info!(
            target: "wcore_agent::late_mcp",
            plugins = ?bound,
            "wayland#562: plugin hook dispatcher wired after deferred MCP connect"
        );
        outcome.hook_plugins_bound = bound;
    }
}
