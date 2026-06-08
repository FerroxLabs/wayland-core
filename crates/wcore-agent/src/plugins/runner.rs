//! `PluginRunner` — lifecycle owner. Builds each plugin's `PluginContext`
//! from real host adapters, runs `Plugin::initialize` with per-plugin error
//! containment, and aggregates the results into `InitializeOutcome`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};

use parking_lot::Mutex;
use wcore_plugin_api::registry::agents::ScopedAgentRegistry;
use wcore_plugin_api::registry::browser::ScopedBrowserRegistry;
use wcore_plugin_api::registry::config::ScopedConfigReader;
use wcore_plugin_api::registry::cua::ScopedCuaRegistry;
use wcore_plugin_api::registry::hooks::ScopedHookRegistry;
use wcore_plugin_api::registry::logger::ScopedPluginLogger;
use wcore_plugin_api::registry::mcp::ScopedMcpRegistry;
use wcore_plugin_api::registry::memory::ScopedMemoryClient;
use wcore_plugin_api::registry::providers::ScopedProviderRegistry;
use wcore_plugin_api::registry::rules::ScopedRuleRegistry;
use wcore_plugin_api::registry::skills::ScopedSkillRegistry;
use wcore_plugin_api::registry::tools::ScopedToolRegistry;
use wcore_plugin_api::registry::user_models::ScopedUserModelRegistry;
use wcore_plugin_api::tool::PluginTool;
use wcore_plugin_api::{
    AgentManifest, BundledSkillSpec, McpServerSpec, PluginContext, PluginError, RuleSpec,
    UserModelSpec,
};

use super::adapters::agent_registrar::HostAgentRegistrar;
use super::adapters::browser_adapter::HostBrowserRegistrar;
use super::adapters::cua_adapter::HostCuaRegistrar;
use super::adapters::hook_registrar::HostHookRegistrar;
use super::adapters::mcp_registrar::HostMcpRegistrar;
use super::adapters::provider_registrar::HostProviderRegistrar;
use super::adapters::rule_registrar::HostRuleRegistrar;
use super::adapters::skill_registrar::HostSkillRegistrar;
use super::adapters::tool_registrar::HostToolRegistrar;
use super::adapters::user_model_registrar::HostUserModelRegistrar;
use super::host_supports::{NullConfigReader, NullMemoryHost};
use super::loader::DiscoveredPlugin;

/// A plugin hook captured during `initialize_all`, enriched with the
/// originating plugin name. Carries no shell command — Phase 1 plugin hooks
/// are name-only; "firing" emits a tracing log line.
///
/// `HookEngine::register_plugin_hook` stores these; the engine consumer
/// (Task 1.3) maps `phase` to the nearest existing phase entrypoint.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PluginHook {
    /// Originating plugin name.
    pub plugin: String,
    /// Which of the 8 `HookPhase` variants this hook subscribes to.
    pub phase: wcore_plugin_api::registry::hooks::HookPhase,
    /// Hook name as registered by the plugin.
    pub name: String,
}

/// A captured plugin tool plus its provenance. Holds the plugin-api-native
/// `PluginTool` (data + closure), NOT a `wcore_tools::Tool`. Reification
/// into a real `Tool` happens later, via `PluginToolAdapter` (Task 1.7).
pub struct CapturedPluginTool {
    /// Originating plugin name — diagnostics + collision messages.
    pub plugin: String,
    /// Fully-qualified `"<namespace>::<name>"` — `ScopedToolRegistry`-computed.
    pub fq_name: String,
    /// The captured plugin tool, still data.
    pub tool: PluginTool,
}

/// v0.6.4 Task 2.1 — a captured user-model spec stamped with the
/// originating plugin name. Carries plain data; reification into a live
/// backend client (e.g. `wayland_honcho::HonchoClient`) lands in Task 2.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedUserModel {
    /// Originating plugin name — diagnostics + collision messages.
    pub plugin: String,
    /// The captured user-model spec, still data.
    pub spec: UserModelSpec,
}

/// The reified, host-owned tool. Produced by `runner.rs` / `apply.rs` by
/// wrapping a `CapturedPluginTool` in a `PluginToolAdapter`. Lives in
/// `wcore-agent`, which may name `wcore_tools::Tool`. Task 1.1 defines the
/// type; Task 1.7 produces it inside `apply_initialize_outcome`.
pub struct ReifiedTool {
    pub plugin: String,
    pub fq_name: String,
    pub tool: Box<dyn wcore_tools::Tool>,
}

impl ReifiedTool {
    /// Reify a `CapturedPluginTool` by wrapping its `PluginTool` in a
    /// `PluginToolAdapter` to obtain a `Box<dyn wcore_tools::Tool>`.
    pub fn from_captured(captured: CapturedPluginTool) -> Self {
        Self {
            plugin: captured.plugin,
            fq_name: captured.fq_name,
            tool: Box::new(
                super::adapters::plugin_tool_adapter::PluginToolAdapter::new(captured.tool),
            ),
        }
    }
}

#[derive(Default)]
pub struct InitializeOutcome {
    pub tools: Vec<CapturedPluginTool>,
    pub hooks: Vec<PluginHook>,
    pub agents: Vec<AgentManifest>,
    pub skills: Vec<BundledSkillSpec>,
    pub rules: Vec<RuleSpec>,
    pub mcp_servers: Vec<McpServerSpec>,
    pub providers_registered: usize,
    /// v0.6.4 Task 2.1 — captured `UserModelSpec`s with plugin provenance.
    /// Reification into a live backend client happens in `apply.rs` (Task 2.2).
    pub user_models: Vec<CapturedUserModel>,
    /// Wave BR — count of `BrowserToolSpec`s captured from `wayland-browser`
    /// plugin loads. The reified `BrowserTool` set is built via
    /// `PluginRunner::browser.reify_all()` after `initialize_all` returns.
    pub browser_tools_registered: usize,
    /// Wave CU — count of `CuaToolSpec`s captured from `wayland-cua`
    /// plugin loads. The reified `CuaTool` set is built via
    /// `PluginRunner::cua.reify_all()` after `initialize_all` returns.
    pub cua_tools_registered: usize,
    pub errors: Vec<(String, PluginError)>,
}

impl InitializeOutcome {
    pub fn has_any_registered(&self) -> bool {
        !self.tools.is_empty()
            || !self.hooks.is_empty()
            || !self.agents.is_empty()
            || !self.skills.is_empty()
            || !self.rules.is_empty()
            || !self.mcp_servers.is_empty()
            || self.providers_registered > 0
            || self.browser_tools_registered > 0
            || self.cua_tools_registered > 0
            || !self.user_models.is_empty()
    }
}

/// Per-plugin error from `initialize()`. Captured into `InitializeOutcome.errors`
/// instead of aborting the whole boot — design spec §5.17 (one bad plugin
/// cannot crash session boot).
pub struct PluginRunner {
    pub tools: HostToolRegistrar,
    pub hooks: HostHookRegistrar,
    pub agents: HostAgentRegistrar,
    pub skills: HostSkillRegistrar,
    pub rules: HostRuleRegistrar,
    pub mcp_servers: HostMcpRegistrar,
    pub providers: HostProviderRegistrar,
    /// Wave BR — captures `BrowserToolSpec`s from wayland-browser plugins.
    pub browser: HostBrowserRegistrar,
    /// Wave CU — captures `CuaToolSpec`s from wayland-cua plugins.
    pub cua: HostCuaRegistrar,
    /// v0.6.4 Task 2.1 — captures `UserModelSpec`s from plugins that
    /// supply a user-model backend (e.g. wayland-honcho).
    pub user_models: HostUserModelRegistrar,
    pub config: NullConfigReader,
    pub memory: NullMemoryHost,
    /// v0.6.5 Task 1.2 — per-plugin consecutive-failure counter. Each entry
    /// is incremented by [`record_failure`] on `Plugin::initialize` or
    /// `register_*` errors, and reset to zero on the first subsequent
    /// success. When a counter reaches [`CRASH_THRESHOLD`] the plugin is
    /// considered auto-disabled for the remainder of the session ([`is_disabled`]).
    /// Reset across session boundaries via [`reset_budget`].
    ///
    /// [`record_failure`]: PluginRunner::record_failure
    /// [`is_disabled`]: PluginRunner::is_disabled
    /// [`reset_budget`]: PluginRunner::reset_budget
    pub crash_budget: Mutex<HashMap<String, AtomicU8>>,
}

/// v0.6.5 Task 1.2 — consecutive-failure threshold for auto-disable.
/// After this many back-to-back `Err`s from `Plugin::initialize` or any
/// `register_*` invocation, [`PluginRunner::is_disabled`] returns `true`
/// for the offending plugin until [`PluginRunner::reset_budget`] is
/// called (typically at the `on_session_start` engine hook).
pub const CRASH_THRESHOLD: u8 = 3;

impl Default for PluginRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRunner {
    pub fn new() -> Self {
        Self {
            tools: HostToolRegistrar::default(),
            hooks: HostHookRegistrar::default(),
            agents: HostAgentRegistrar::default(),
            skills: HostSkillRegistrar::default(),
            rules: HostRuleRegistrar::default(),
            mcp_servers: HostMcpRegistrar::default(),
            providers: HostProviderRegistrar::default(),
            browser: HostBrowserRegistrar::default(),
            cua: HostCuaRegistrar::default(),
            user_models: HostUserModelRegistrar::default(),
            config: NullConfigReader,
            memory: NullMemoryHost::default(),
            crash_budget: Mutex::new(HashMap::new()),
        }
    }

    /// Increment the consecutive-failure counter for `plugin_name`. When the
    /// counter crosses [`CRASH_THRESHOLD`] for the first time, emits a
    /// `tracing::warn!` with the auto-disable message. Subsequent failures
    /// keep climbing (saturating at `u8::MAX`) but only the threshold-crossing
    /// log fires once per disable.
    ///
    /// Semantics are **consecutive**: any call to [`record_success`] resets
    /// the counter to zero. Mixed Ok/Err sequences therefore never reach the
    /// threshold.
    pub fn record_failure(&self, plugin_name: &str) {
        let mut map = self.crash_budget.lock();
        let counter = map
            .entry(plugin_name.to_string())
            .or_insert_with(|| AtomicU8::new(0));
        let prev = counter.fetch_add(1, Ordering::AcqRel);
        let new = prev.saturating_add(1);
        if prev < CRASH_THRESHOLD && new >= CRASH_THRESHOLD {
            tracing::warn!(
                plugin = %plugin_name,
                "auto-disabled after 3 consecutive failures"
            );
        }
    }

    /// Reset the consecutive-failure counter for `plugin_name` to zero.
    /// Called whenever a `register_*` invocation or `Plugin::initialize`
    /// succeeds, implementing the **consecutive-only** semantics: a single
    /// success in the middle of failures wipes the counter.
    pub fn record_success(&self, plugin_name: &str) {
        let map = self.crash_budget.lock();
        if let Some(counter) = map.get(plugin_name) {
            counter.store(0, Ordering::Release);
        }
    }

    /// True iff `plugin_name`'s consecutive-failure counter has reached
    /// [`CRASH_THRESHOLD`]. Dispatch sites should short-circuit when this
    /// returns `true` to honor the 3-strike auto-disable.
    pub fn is_disabled(&self, plugin_name: &str) -> bool {
        let map = self.crash_budget.lock();
        map.get(plugin_name)
            .map(|c| c.load(Ordering::Acquire) >= CRASH_THRESHOLD)
            .unwrap_or(false)
    }

    /// Clear every per-plugin counter. Engine consumers call this at the
    /// `on_session_start` boundary so a new session starts each plugin with
    /// a fresh 3-strike budget.
    pub fn reset_budget(&self) {
        self.crash_budget.lock().clear();
    }

    /// Toggle the `computer_use` capability flag on the CUA registrar.
    /// Reification only succeeds when `true`; otherwise every captured
    /// `CuaToolSpec` produces `CuaError::CapabilityDisabled` at reify-time.
    pub fn with_computer_use_advertised(mut self, advertised: bool) -> Self {
        self.cua.set_computer_use_advertised(advertised);
        self
    }

    /// Build a `PluginContext` for one plugin, call `initialize()`, and capture
    /// any error in the outcome. The outcome's vectors are populated by the
    /// host adapters as the plugin registers; the runner just relays them.
    pub async fn initialize_all(
        &mut self,
        plugins: &[DiscoveredPlugin],
    ) -> Result<InitializeOutcome, PluginError> {
        let mut errors: Vec<(String, PluginError)> = Vec::new();

        for d in plugins {
            // v0.6.5 Task 1.2: skip plugins already auto-disabled. This keeps
            // a wedged plugin from re-running `initialize` (and re-incrementing
            // its budget) on every discovery pass within the same session.
            if self.is_disabled(&d.name) {
                continue;
            }
            // Track whether this plugin's `register_*` chain or
            // `Plugin::initialize` produced any error. A clean boot
            // resets the consecutive-failure counter; any error
            // increments it (3 in a row → auto-disable).
            let errors_before = errors.len();
            // Build per-plugin scoped registries. Each Scoped*::new
            // returns Err on permission denial; that's expected for
            // plugins that don't ask for the surface, so we silently
            // leave the field None.
            //
            // Wave RB STABILITY MINOR #10: any error OTHER than the
            // expected `PluginError::PermissionDenied` "permission not
            // requested" sentinel is a real registration failure. We
            // capture it in `errors` (which the host renders via the
            // `PluginRegistrationFailed` protocol event) so missing
            // surfaces have a visible cause instead of being silently
            // swallowed.
            let manifest = &d.manifest;

            // Per-plugin capture borrows `&mut self.tools` for the
            // iteration so each captured `PluginTool` is stamped with the
            // originating plugin name (mirrors the cua arm below).
            let mut tools_capture = self.tools.capture_for_plugin(d.name.clone());
            let tools = unwrap_scoped(
                ScopedToolRegistry::new(manifest, &mut tools_capture),
                &d.name,
                "tools",
                &mut errors,
            );
            // Per-plugin capture borrows `&mut self.hooks` so each captured
            // hook is stamped with the originating plugin name — matching the
            // tool and cua arm patterns (Task 1.3).
            let mut hooks_capture = self.hooks.capture_for_plugin(d.name.clone());
            let hooks = unwrap_scoped(
                ScopedHookRegistry::new(manifest, &mut hooks_capture),
                &d.name,
                "hooks",
                &mut errors,
            );
            let agents = unwrap_scoped(
                ScopedAgentRegistry::new(manifest, &mut self.agents),
                &d.name,
                "agents",
                &mut errors,
            );
            let skills = unwrap_scoped(
                ScopedSkillRegistry::new(manifest, &mut self.skills),
                &d.name,
                "skills",
                &mut errors,
            );
            let rules = unwrap_scoped(
                ScopedRuleRegistry::new(manifest, &mut self.rules),
                &d.name,
                "rules",
                &mut errors,
            );
            let mcp_servers = unwrap_scoped(
                ScopedMcpRegistry::new(manifest, &mut self.mcp_servers),
                &d.name,
                "mcp_servers",
                &mut errors,
            );
            let providers = unwrap_scoped(
                ScopedProviderRegistry::new(manifest, &mut self.providers),
                &d.name,
                "providers",
                &mut errors,
            );
            let browser = unwrap_scoped(
                ScopedBrowserRegistry::new(manifest, &mut self.browser),
                &d.name,
                "browser",
                &mut errors,
            );
            // Wave CU: per-plugin capture borrows `&mut self.cua` for the
            // iteration so `reify_all` can later override `policy.plugin_id`
            // with the originating plugin name (SECURITY MAJOR from wave SC).
            let mut cua_capture = self.cua.capture_for_plugin(d.name.clone());
            let cua = unwrap_scoped(
                ScopedCuaRegistry::new(manifest, &mut cua_capture),
                &d.name,
                "cua",
                &mut errors,
            );
            // v0.6.4 Task 2.1: per-plugin capture borrows `&mut self.user_models`
            // so each captured spec is stamped with the originating plugin name.
            let mut user_models_capture = self.user_models.capture_for_plugin(d.name.clone());
            let user_models = unwrap_scoped(
                ScopedUserModelRegistry::new(manifest, &mut user_models_capture),
                &d.name,
                "user_models",
                &mut errors,
            );

            let config = ScopedConfigReader::new(&self.config);
            let logger = ScopedPluginLogger::new(&manifest.plugin.name);
            let memory = ScopedMemoryClient::new(manifest, &mut self.memory);

            let mut ctx = PluginContext {
                manifest,
                tools,
                hooks,
                agents,
                skills,
                rules,
                mcp_servers,
                providers,
                browser,
                cua,
                user_models,
                sandbox: None,
                config,
                logger,
                memory,
            };

            if let Err(e) = d.plugin.initialize(&mut ctx).await {
                tracing::warn!(plugin = %d.name, error = %e, "plugin initialize failed");
                errors.push((d.name.clone(), e));
            }

            // v0.6.5 Task 1.2: roll up this plugin's boot result into the
            // crash budget. One `record_failure` for each error that the
            // `register_*` chain or `Plugin::initialize` produced this pass;
            // a clean boot (no new errors) calls `record_success` so the
            // counter resets — consecutive-only semantics.
            let new_errors = errors.len() - errors_before;
            if new_errors == 0 {
                self.record_success(&d.name);
            } else {
                for _ in 0..new_errors {
                    self.record_failure(&d.name);
                }
            }
        }

        // Move the captured `(plugin, fq, PluginTool)` triples out of the
        // tool registrar into `CapturedPluginTool`s — `mem::take` avoids
        // cloning the `Arc<dyn Fn..>` execution closures.
        let tools = std::mem::take(&mut self.tools.registered)
            .into_iter()
            .map(|(plugin, fq_name, tool)| CapturedPluginTool {
                plugin,
                fq_name,
                tool,
            })
            .collect();

        // Map captured (plugin, phase, name) triples into `PluginHook`s.
        let hooks = self
            .hooks
            .registered
            .iter()
            .map(|(plugin, phase, name)| PluginHook {
                plugin: plugin.clone(),
                phase: *phase,
                name: name.clone(),
            })
            .collect();

        // v0.6.4 Task 2.1: move the captured `(plugin, spec)` tuples out of
        // the host registrar into `CapturedUserModel`s for the outcome.
        let user_models = std::mem::take(&mut self.user_models.registered)
            .into_iter()
            .map(|(plugin, spec)| CapturedUserModel { plugin, spec })
            .collect();

        Ok(InitializeOutcome {
            tools,
            hooks,
            agents: self.agents.registered.clone(),
            skills: self.skills.registered.clone(),
            rules: self.rules.registered.clone(),
            mcp_servers: self.mcp_servers.registered.clone(),
            providers_registered: self.providers.registered.len(),
            browser_tools_registered: self.browser.specs.len(),
            cua_tools_registered: self.cua.registered_count(),
            user_models,
            errors,
        })
    }

    /// Reverse-order shutdown — design spec §5.17 line 1398 discovery flow (5).
    pub async fn shutdown_all(
        &mut self,
        plugins: &[DiscoveredPlugin],
    ) -> Vec<(String, PluginError)> {
        let mut errs = Vec::new();
        for d in plugins.iter().rev() {
            if let Err(e) = d.plugin.shutdown().await {
                errs.push((d.name.clone(), e));
            }
        }
        errs
    }
}

/// Wave RB STABILITY MINOR #10: convert a scoped-registry constructor
/// result into `Option<Registry>`, capturing any non-permission error
/// into the runner's `errors` accumulator so the host can render a
/// `PluginRegistrationFailed` diagnostic. The previous behaviour
/// silently dropped every error — including real namespace collisions
/// — which made plugin-load failures invisible.
///
/// `PluginError::PermissionDenied` is the expected "the plugin didn't
/// request this surface" sentinel — that case correctly returns `None`
/// without an error entry. Every other variant (NamespaceCollision,
/// NamespaceMissing, ManifestSchema, etc.) is a real failure and gets
/// logged.
fn unwrap_scoped<R>(
    res: Result<R, PluginError>,
    plugin_name: &str,
    surface: &str,
    errors: &mut Vec<(String, PluginError)>,
) -> Option<R> {
    match res {
        Ok(r) => Some(r),
        Err(PluginError::PermissionDenied { .. }) => {
            // Expected: plugin didn't request this surface.
            None
        }
        Err(e) => {
            tracing::warn!(
                plugin = %plugin_name,
                surface = %surface,
                error = %e,
                "plugin scoped-registry registration failed (non-permission)"
            );
            // Surface as a per-plugin error so the host can render
            // a typed `PluginRegistrationFailed` event. We synthesise
            // a key like "<plugin>:<surface>" so the host can tell
            // which surface failed.
            errors.push((format!("{plugin_name}:{surface}"), e));
            None
        }
    }
}

#[cfg(test)]
mod crash_budget_tests {
    //! v0.6.5 Task 1.2 — unit tests for the per-plugin 3-strike auto-disable.
    //!
    //! These exercise `PluginRunner::{record_failure, record_success,
    //! is_disabled, reset_budget}` directly. The wiring in `initialize_all`
    //! (one increment per `register_*` or `Plugin::initialize` error, success
    //! resets the counter) is covered indirectly by the existing
    //! `plugin_api_smoke` integration tests, which exercise a full clean boot
    //! after these changes.
    use super::{CRASH_THRESHOLD, PluginRunner};

    #[test]
    fn disables_after_three_failures() {
        let r = PluginRunner::new();
        assert!(!r.is_disabled("p"));
        for i in 1..=CRASH_THRESHOLD {
            r.record_failure("p");
            assert_eq!(
                r.is_disabled("p"),
                i >= CRASH_THRESHOLD,
                "disabled state at failure #{i}"
            );
        }
        // Fourth and subsequent failures stay disabled (and don't panic on
        // u8 overflow — counter saturates).
        r.record_failure("p");
        assert!(r.is_disabled("p"));
    }

    #[test]
    fn mixed_success_does_not_disable() {
        // Consecutive-failure semantics: any success between failures resets
        // the counter, so an alternating pattern never crosses the threshold.
        let r = PluginRunner::new();
        for _ in 0..10 {
            r.record_failure("p");
            r.record_success("p");
        }
        assert!(!r.is_disabled("p"));
    }

    #[test]
    fn session_start_resets_budget() {
        // Drive a plugin into the disabled state, then simulate the engine's
        // `on_session_start` hook by calling `reset_budget`. The plugin must
        // come back enabled with a fresh 3-strike window.
        let r = PluginRunner::new();
        for _ in 0..CRASH_THRESHOLD {
            r.record_failure("p");
        }
        assert!(r.is_disabled("p"));

        r.reset_budget();
        assert!(!r.is_disabled("p"));

        // And the counter really is zero — two more failures alone must not
        // re-trigger disable.
        r.record_failure("p");
        r.record_failure("p");
        assert!(!r.is_disabled("p"));
    }

    #[test]
    fn per_plugin_isolation() {
        // Sanity check the "per-plugin not global" requirement (T7): one
        // plugin's failures must not poison another plugin.
        let r = PluginRunner::new();
        for _ in 0..CRASH_THRESHOLD {
            r.record_failure("noisy");
        }
        assert!(r.is_disabled("noisy"));
        assert!(!r.is_disabled("quiet"));
    }
}
