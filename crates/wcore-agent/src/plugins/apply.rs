//! `apply_initialize_outcome`: single reification call site for every
//! plugin-contributed capability. Bootstrap calls this once after
//! `PluginRunner::initialize_all` and threads the returned vectors to
//! their natural consumers.
//!
//! v0.6.4 introduced the function with six surfaces. v0.6.5 Task 1.4
//! folded the previously-separate Browser and CUA reify steps in so all
//! ten surfaces flow through one entry point — SDK authors read one file,
//! not three.
//!
//! Delivered surfaces:
//!
//! - tools — `PluginToolAdapter` wrapped, registered into the caller-supplied
//!   `ToolRegistry` (builtins win name collisions).
//! - agents — fresh `AgentRegistry`.
//! - hooks — `plugin_hooks` carrier (engine setter, post-construction).
//! - skills — `plugin_skills` carrier (session-local catalog insertion before
//!   `load_catalog_with_bundled`).
//! - rules — `plugin_rules` carrier (`build_system_prompt`).
//! - mcp servers — `plugin_mcp_servers` carrier (`connect_plugin_mcp_servers`
//!   second pass).
//! - user models — `plugin_user_models` carrier (Task 2.2; reified by 2.3
//!   once a backend is wired).
//! - providers — registered inside `PluginRunner::initialize_all` itself;
//!   the counter is consumed elsewhere.
//! - browser tools — reified here via `deliver_browser_tools` and registered
//!   into `tool_registry` next to plain plugin tools. Builtins win name
//!   collisions.
//! - cua tools — reified here via `deliver_cua_tools` and registered into
//!   `tool_registry`. Per-plugin reify errors are logged via
//!   `tracing::warn!`.
//!
//! # Carrier shape vs Phase 1 design §5
//!
//! Phase 1's §5 sketched a `mcp_server_specs: &mut HashMap<…>` argument.
//! Task 1.5 instead shipped `connect_plugin_mcp_servers` — a second-pass
//! connector taking a `&[McpServerSpec]` slice (§5.6's *preferred*
//! "no reorder" option). The SHIPPED carrier pattern — every plugin
//! capability either lands in a registry passed by `&mut` or is returned
//! by-value in `AppliedPluginCapabilities` — is the canonical v0.6.5
//! reification shape. This module's doc and signature are authoritative;
//! the design notes have been superseded.
//!
//! # Browser / CUA ownership (v0.6.5 Task 1.4)
//!
//! Both registrars live on `PluginRunner` during plugin init. To keep
//! `apply_initialize_outcome` free of a `&mut PluginRunner` dep, bootstrap
//! moves each registrar out of the runner (`std::mem::take`) before
//! calling this function and passes them in by value. `HostBrowserRegistrar`
//! is `#[derive(Default)]`; `HostCuaRegistrar` carries its
//! `computer_use_advertised` flag through a manual `Default` impl, so a
//! taken-out registrar is replaced with a fresh empty one and the runner
//! stays well-formed.

use wayland_honcho::{HonchoClient, HonchoError};
use wcore_plugin_api::{AgentManifest, BundledSkillSpec, McpServerSpec, PluginError, RuleSpec};

use crate::agents::registry::AgentRegistry;
use crate::plugins::adapters::browser_adapter::HostBrowserRegistrar;
use crate::plugins::adapters::cua_adapter::HostCuaRegistrar;
use crate::plugins::runner::{CapturedUserModel, InitializeOutcome, PluginHook, ReifiedTool};

/// v0.6.5 Task 1.5 — a host-owned, live user-model backend client paired
/// with its originating plugin name for diagnostics.
///
/// `client` is the reified backend (today: `wayland_honcho::HonchoClient`).
/// The variant is open to future backends via the `backend` enum
/// discriminant; v0.6.5 ships only `Honcho`.
pub struct ReifiedUserModel {
    /// Originating plugin name. Matches `CapturedUserModel.plugin`.
    pub plugin: String,
    /// Plugin-supplied name of the user-model registration (e.g. `"honcho"`).
    pub name: String,
    /// The live backend client.
    pub backend: ReifiedUserModelBackend,
}

/// v0.6.5 Task 1.5 — open enum over reified backends. Today only Honcho
/// reifies; adding a backend is a forward-compatible enum extension and
/// requires no surface change to `AppliedPluginCapabilities`.
pub enum ReifiedUserModelBackend {
    /// A live `wayland_honcho::HonchoClient`. Constructed from the spec's
    /// `base_url` and `api_key_env` fields via `HonchoClient::from_spec`.
    Honcho(HonchoClient),
}

/// Phase 1 result: every plugin capability that needs threading *after*
/// `initialize_all` returns. Bootstrap consumes this struct and hands each
/// field to the appropriate owner.
pub struct AppliedPluginCapabilities {
    /// Fresh `AgentRegistry` pre-loaded with every `AgentManifest` the
    /// plugins registered. Bootstrap hands this to `SpawnTool::with_registry`
    /// (and, via `engine.set_agent_registry`, to the engine). Task 1.2.
    pub agent_registry: AgentRegistry,

    /// Plugin-contributed skills. Bootstrap moves each into its local bundled
    /// catalog before `load_catalog_with_bundled` runs. Task 1.6 + 1.7.
    pub plugin_skills: Vec<BundledSkillSpec>,

    /// Plugin-contributed hooks (Task 1.3). Bootstrap hands these to
    /// `engine.register_plugin_hooks` after the engine is built.
    pub plugin_hooks: Vec<PluginHook>,

    /// Plugin-contributed rules (Task 1.4). Bootstrap threads these into
    /// `build_system_prompt` so `RuleScope::Universal` fragments are appended
    /// to the prompt.
    pub plugin_rules: Vec<RuleSpec>,

    /// Plugin-contributed MCP servers (Task 1.5 + 1.7). Bootstrap feeds this
    /// slice to `mcp_delivery::connect_plugin_mcp_servers` for the second
    /// `connect_all` + `register_mcp_tools` pass.
    pub plugin_mcp_servers: Vec<McpServerSpec>,

    /// Plugin-contributed user-model backends (Task 2.2). Each entry is a
    /// `CapturedUserModel` — the plugin-supplied `UserModelSpec` stamped with
    /// the originating plugin name. Kept as a raw-data record for backwards
    /// compatibility and diagnostics: this carrier reflects what plugins
    /// *registered*, not what successfully reified.
    pub plugin_user_models: Vec<CapturedUserModel>,

    /// v0.6.5 Task 1.5 — host-owned reified user-model backends. Produced
    /// by `deliver_user_models` from `plugin_user_models`: each Honcho-tagged
    /// spec becomes a live `HonchoClient::from_spec(...)` instance. Unknown
    /// backend tags do NOT appear here; they short-circuit into
    /// `user_model_errors` as typed `PluginError::UnknownUserModelBackend`.
    ///
    /// Engine consumer status (v0.6.5 Wave 6A.2): bootstrap threads this
    /// slice into the engine via `AgentEngine::set_plugin_user_models`.
    /// At session end, the PUM path mirrors every inferred user-model
    /// delta to each reified backend (e.g. Honcho via `learn_preference`)
    /// in addition to writing through `MemoryApi::update_user_model`.
    pub plugin_reified_user_models: Vec<ReifiedUserModel>,

    /// v0.6.5 Task 1.5 — typed errors from user-model reification.
    /// Populated alongside `plugin_reified_user_models`: unknown backend
    /// tags, missing API keys, and other typed failures appear here so
    /// callers can surface them through the normal error-reporting path
    /// instead of via panics.
    pub user_model_errors: Vec<PluginError>,
}

/// Deliver every plugin-registered capability into its live registry or
/// return it for bootstrap to thread to the correct owner.
///
/// **Tools (Task 1.7):** each `CapturedPluginTool` is reified — wrapped in a
/// `PluginToolAdapter` to obtain a `Box<dyn wcore_tools::Tool>` — and
/// registered into `tool_registry`. A plugin tool whose name collides with a
/// tool already present (a builtin, when called after the builtin block) is
/// logged via `tracing::warn!` and skipped: builtins win, and one bad plugin
/// cannot crash boot.
///
/// **Agents (Task 1.2):** creates a fresh `AgentRegistry`, logs duplicate
/// errors (does not panic), returns it in `agent_registry`.
///
/// **Browser tools (v0.6.5 Task 1.4):** `browser_registrar` is consumed.
/// Each captured `BrowserToolSpec` is reified into an `Arc<BrowserTool>` and
/// registered into `tool_registry` (builtins win collisions). Registration
/// is the contract; no carrier is returned.
///
/// **CUA tools (v0.6.5 Task 1.4):** `cua_registrar` is consumed. Each
/// captured `CuaToolSpec` is reified; per-plugin reify errors are logged via
/// `tracing::warn!` (e.g. `WaylandRestricted` on locked-down compositors,
/// `CapabilityDisabled` when `computer_use` was never advertised) and the
/// failing entries are dropped. Successfully reified tools are registered
/// into `tool_registry`; no carrier is returned.
///
/// **Hooks / skills / rules / mcp / user models:** returned in the
/// corresponding `AppliedPluginCapabilities` field for bootstrap to thread
/// onward at its natural consumer site.
///
/// `outcome` is taken by value: every field is moved into either the
/// `tool_registry` or a returned carrier.
pub fn apply_initialize_outcome(
    outcome: InitializeOutcome,
    tool_registry: &mut wcore_tools::registry::ToolRegistry,
    browser_registrar: HostBrowserRegistrar,
    cua_registrar: HostCuaRegistrar,
) -> AppliedPluginCapabilities {
    // --- Tool path (Task 1.7) ---
    deliver_tools(outcome.tools, tool_registry);

    // --- Agent path (Task 1.2) ---
    let agent_registry = build_agent_registry(outcome.agents);

    // --- Browser tool path (v0.6.5 Task 1.4) ---
    deliver_browser_tools(browser_registrar, tool_registry);

    // --- CUA tool path (v0.6.5 Task 1.4) ---
    deliver_cua_tools(cua_registrar, tool_registry);

    // --- Carrier paths: threaded onward by bootstrap at their natural
    // consumer sites (hooks/skills/rules/mcp/user-models) ---
    let plugin_hooks = outcome.hooks;
    let plugin_skills = outcome.skills;
    let plugin_rules = outcome.rules;
    let plugin_mcp_servers = outcome.mcp_servers;
    let plugin_user_models = outcome.user_models;

    // --- User-model reify path (v0.6.5 Task 1.5) ---
    let (plugin_reified_user_models, user_model_errors) = deliver_user_models(&plugin_user_models);

    // `providers_registered` is set inside `PluginRunner::initialize_all`
    // (provider registration happens during init, not here);
    // `browser_tools_registered` / `cua_tools_registered` are the *captured*
    // counters and are now superseded by the actual reified-vector lengths
    // produced by `deliver_browser_tools` / `deliver_cua_tools` above (Task 1.4);
    // `errors` is consumed by the per-plugin crash budget elsewhere.
    // Explicitly drop the remaining counters so a future field addition
    // surfaces as a compile error here, not a silent omission.
    let _ = (
        outcome.providers_registered,
        outcome.browser_tools_registered,
        outcome.cua_tools_registered,
        outcome.errors,
    );

    AppliedPluginCapabilities {
        agent_registry,
        plugin_skills,
        plugin_hooks,
        plugin_rules,
        plugin_mcp_servers,
        plugin_user_models,
        plugin_reified_user_models,
        user_model_errors,
    }
}

/// v0.6.5 Task 1.5 — reify each `CapturedUserModel` into a live backend
/// client (today: `wayland_honcho::HonchoClient::from_spec`).
///
/// Dispatch:
/// - `spec.backend == "honcho"` → construct via
///   `HonchoClient::from_spec(spec.base_url, spec.api_key_env)`. Honcho's
///   `MissingApiKey` is wrapped into `PluginError::InitializeFailed` so the
///   error path is uniform with the rest of the loader.
/// - any other `backend` tag → typed `PluginError::UnknownUserModelBackend`.
///
/// Returns `(reified, errors)`. Unknown backends and construction failures
/// land in `errors`; successful reifications land in `reified`. No panics.
fn deliver_user_models(
    captured: &[CapturedUserModel],
) -> (Vec<ReifiedUserModel>, Vec<PluginError>) {
    let mut reified = Vec::with_capacity(captured.len());
    let mut errors = Vec::new();

    for entry in captured {
        match entry.spec.backend.as_str() {
            "honcho" => {
                let client_res = HonchoClient::from_spec(
                    entry.spec.base_url.as_deref(),
                    entry.spec.api_key_env.as_deref(),
                );
                match client_res {
                    Ok(client) => reified.push(ReifiedUserModel {
                        plugin: entry.plugin.clone(),
                        name: entry.spec.name.clone(),
                        backend: ReifiedUserModelBackend::Honcho(client),
                    }),
                    // Bug #6: a missing `HONCHO_API_KEY` means the user never
                    // configured this optional backend — degrade SILENTLY like
                    // every other optional integration (postgres/spotify/etc.)
                    // and like bootstrap's own auto-select, which already falls
                    // back to the local backend. No WARN, no surfaced error;
                    // just a debug breadcrumb. The live path is skipped, not
                    // failed.
                    Err(HonchoError::MissingApiKey) => {
                        tracing::debug!(
                            plugin = %entry.plugin,
                            name = %entry.spec.name,
                            "honcho user-model: HONCHO_API_KEY absent — skipping live backend \
                             (local fallback applies)"
                        );
                    }
                    Err(e) => {
                        // A genuine failure with a key that IS configured
                        // (bad URL, auth rejected, etc.). Wrap Honcho's typed
                        // error into a plugin-loader error so callers see a
                        // uniform error path.
                        tracing::warn!(
                            plugin = %entry.plugin,
                            name = %entry.spec.name,
                            error = %e,
                            "honcho user-model reify failed; surfacing typed error"
                        );
                        errors.push(PluginError::InitializeFailed {
                            plugin: entry.plugin.clone(),
                            source: anyhow::Error::new(e),
                        });
                    }
                }
            }
            unknown => {
                tracing::warn!(
                    plugin = %entry.plugin,
                    backend = %unknown,
                    "unknown user-model backend; surfacing typed error"
                );
                errors.push(PluginError::UnknownUserModelBackend {
                    plugin: entry.plugin.clone(),
                    backend: unknown.to_string(),
                });
            }
        }
    }

    (reified, errors)
}

/// Reify each `CapturedPluginTool` and register it into `tool_registry`.
///
/// Collision rule (§5.1, Task 1.7): if a tool with the same name is already
/// registered, log a warning and skip the plugin tool. Called *after* the
/// builtin block, so builtins always win.
fn deliver_tools(
    tools: Vec<crate::plugins::runner::CapturedPluginTool>,
    tool_registry: &mut wcore_tools::registry::ToolRegistry,
) {
    for captured in tools {
        let reified = ReifiedTool::from_captured(captured);
        let name = reified.tool.name().to_string();
        if tool_registry.get(&name).is_some() {
            tracing::warn!(
                plugin = %reified.plugin,
                tool = %name,
                fq_name = %reified.fq_name,
                "plugin tool name collides with an already-registered tool \
                 (builtin wins) — skipping plugin tool"
            );
            continue;
        }
        tool_registry.register(reified.tool);
    }
}

/// Build a fresh `AgentRegistry` from a `Vec<AgentManifest>`.
///
/// Duplicate names are logged and skipped — the first registration wins,
/// matching the "one bad plugin cannot crash boot" principle from §5 of the
/// Phase 1 design notes.
fn build_agent_registry(agents: Vec<AgentManifest>) -> AgentRegistry {
    use wcore_plugin_api::registry::agents::AgentRegistrar as _;

    let mut registry = AgentRegistry::new();
    for agent in agents {
        let name = agent.name.clone();
        if let Err(e) = registry.host_register_agent(agent) {
            tracing::warn!(
                agent = %name,
                error = %e,
                "plugin agent registration skipped (duplicate)"
            );
        }
    }
    registry
}

/// Reify every captured `BrowserToolSpec` into a real `Arc<BrowserTool>` and
/// register it into `tool_registry`. Name collisions with already-registered
/// tools (builtins, when this runs after the builtin block) are logged and
/// skipped — builtins win.
///
/// Registration into `tool_registry` is the contract; this function returns
/// no carrier. Callers introspect the registry directly.
fn deliver_browser_tools(
    browser_registrar: HostBrowserRegistrar,
    tool_registry: &mut wcore_tools::registry::ToolRegistry,
) {
    use wcore_tools::Tool as _;

    let tools = browser_registrar.reify_all();
    for browser_tool in &tools {
        let name = browser_tool.name().to_string();
        if tool_registry.get(&name).is_some() {
            tracing::warn!(
                tool = %name,
                "browser plugin tool name collides with a builtin — skipping"
            );
            continue;
        }
        tool_registry.register(Box::new(browser_tool.clone()));
    }
}

/// Reify every captured `CuaToolSpec` into a real `Arc<CuaTool>` and register
/// it into `tool_registry`. Per-plugin reify errors are logged via
/// `tracing::warn!` (e.g. `WaylandRestricted` on locked-down compositors,
/// `CapabilityDisabled` when the host never advertised `computer_use`) and
/// the failing entries are dropped. Name collisions with already-registered
/// tools (builtins) are logged and skipped — builtins win.
///
/// Registration into `tool_registry` is the contract; this function returns
/// no carrier. Callers introspect the registry directly.
fn deliver_cua_tools(
    cua_registrar: HostCuaRegistrar,
    tool_registry: &mut wcore_tools::registry::ToolRegistry,
) {
    use wcore_tools::Tool as _;

    let (cua_tools, cua_errs) = cua_registrar.reify_all();
    for (plugin, err) in &cua_errs {
        tracing::warn!(plugin = %plugin, error = %err, "cua tool reification failed");
    }
    for reg_cua in &cua_tools {
        let name = reg_cua.tool.name().to_string();
        if tool_registry.get(&name).is_some() {
            tracing::warn!(
                tool = %name,
                plugin = %reg_cua.plugin,
                "cua plugin tool name collides with a builtin — skipping"
            );
            continue;
        }
        tool_registry.register(Box::new(reg_cua.tool.clone()));
    }
}
