use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use wcore_config::circuit_breaker::{BreakerState, CircuitBreaker, CircuitBreakerConfig};
use wcore_types::tool::{ToolDef, ToolResult};

use crate::Tool;
use crate::dispatcher::ToolDispatcher;

/// Per-tool circuit-breaker defaults.
///
/// 3 failures in a 30-second window trips the breaker; it stays Open
/// for 60 seconds before allowing a single trial (HalfOpen).
fn default_breaker_cfg() -> CircuitBreakerConfig {
    CircuitBreakerConfig::default()
}

/// A requested circuit-breaker restoration named tools that are not
/// registered in this registry.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("cannot restore circuit breakers for unregistered tools: {names:?}")]
pub struct BreakerRestoreError {
    /// Sorted, deduplicated unregistered tool names.
    pub names: Vec<String>,
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    /// One circuit breaker per registered tool name. `Arc<RwLock<…>>`
    /// so the registry can be shared across async call sites without
    /// requiring `&mut self` at dispatch time.
    breakers: Arc<RwLock<HashMap<String, CircuitBreaker>>>,
    /// Optional filesystem the orchestration dispatcher routes every
    /// tool's `ToolContext` through. `None` (the default) means the
    /// dispatcher uses an unconfined `RealFs` — the local-CLI behaviour.
    /// A channel-originated engine in `Workspace` posture sets this to a
    /// `SandboxedFs` rooted at its workspace so `Read`/`Grep`/`Glob`
    /// (which honour `ctx.vfs`) cannot escape the jail. Carried on the
    /// registry — which is already threaded into every orchestration
    /// `execute_*` call — to avoid plumbing a new parameter through the
    /// whole dispatch stack.
    tool_vfs: Option<Arc<dyn crate::vfs::VirtualFs>>,

    /// Session workspace policy, installed at bootstrap (`Trusted`) or by the
    /// `Workspace` posture (`Contained`). Threaded onto every dispatched
    /// `ToolContext` so BashTool can root its OS sandbox at the workspace.
    workspace_policy: Option<Arc<crate::workspace_policy::WorkspacePolicy>>,

    /// Immutable per-session OS sandbox runtime threaded into every
    /// `ToolContext`. The default fails closed so a host that forgets to
    /// install a session runtime cannot inherit process-global bypass state;
    /// production bootstrap replaces it with the resolved session runtime.
    sandbox_runtime: Arc<wcore_sandbox::SandboxRegistry>,

    /// `[default] read_only` for this session. When `true` the orchestration
    /// dispatcher refuses every tool that does not declare
    /// [`crate::Tool::read_only_safe`] for its concrete input — BEFORE
    /// PreToolUse hooks run, so a refused call fires no operator shell.
    ///
    /// Carried on the registry for the same reason `tool_vfs` and
    /// `workspace_policy` are: it is already threaded into every
    /// orchestration `execute_*` call, so a new dispatch path cannot forget
    /// to plumb a parameter and silently lose the gate.
    read_only: bool,

    /// Set when the most recent attempt to reach a human FAILED and no later
    /// attempt has succeeded. While set, the orchestration dispatcher refuses
    /// every tool that cannot claim [`crate::Tool::read_only_safe`], except
    /// the human-contact surface itself — the one call that can clear it.
    ///
    /// Corpus row B-3. A failed `send_message` used to be an ordinary tool
    /// error: the model saw a string, decided for itself what it meant, and
    /// carried on. With the approval channel made undeliverable, one of two
    /// graded runs went ahead and rewrote the dependency pin anyway. The
    /// property has to hold whatever the model concludes, so it is enforced
    /// here instead of asked for in a prompt.
    ///
    /// `AtomicBool` behind the shared registry for the same reason the
    /// breakers are: the dispatch path holds `&ToolRegistry`, never `&mut`.
    human_unreachable: Arc<std::sync::atomic::AtomicBool>,

    /// The session's hydrated-tool set, published by the engine and read by
    /// the registered `ToolSearch`. Owned HERE, not by the search tool, so
    /// [`Self::refresh_tool_search_catalog`] can rebuild the tool without
    /// forgetting what the engine has already admitted — a rebuild happens on
    /// bootstrap, on every config-MCP registration, on `/mcp add`, and from
    /// the TUI engine bridge.
    hydrated_tools: crate::tool_search::HydratedTools,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            breakers: Arc::new(RwLock::new(HashMap::new())),
            tool_vfs: None,
            workspace_policy: None,
            sandbox_runtime: Arc::new(wcore_sandbox::SandboxRegistry::new(Arc::new(
                wcore_sandbox::FailClosedBackend::new(),
            ))),
            read_only: false,
            human_unreachable: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            hydrated_tools: crate::tool_search::HydratedTools::default(),
        }
    }

    /// Handle onto the session's hydrated-tool set.
    ///
    /// The engine owns the decision (`AgentEngine::hydrated_tool_names`) and
    /// publishes it here with [`Self::publish_hydrated_tools`]; the registered
    /// `ToolSearch` reads it to decide whether a match is a first load or one
    /// the engine has already admitted. Exposed so a host that keeps the
    /// registry behind an `Arc` can still publish without `&mut`.
    pub fn hydrated_tools(&self) -> crate::tool_search::HydratedTools {
        self.hydrated_tools.clone()
    }

    /// Replace the session's hydrated-tool set. Takes `&self`: the engine
    /// holds the registry behind an `Arc` and publishes on every hydration.
    pub fn publish_hydrated_tools<I, S>(&self, names: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        *self.hydrated_tools.write() = names.into_iter().map(Into::into).collect();
    }

    /// Install the session's `[default] read_only` posture. Set once at
    /// bootstrap; there is no un-set — a read-only session cannot be talked
    /// back into mutating.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Whether this session is read-only. Consulted by the orchestration
    /// dispatcher before anything else happens to a tool call.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Set the filesystem every dispatched tool's `ToolContext` is built
    /// with. See the [`tool_vfs`](Self::tool_vfs) field. Used by the
    /// channel `Workspace` posture to install a `SandboxedFs` jail.
    pub fn set_tool_vfs(&mut self, vfs: Arc<dyn crate::vfs::VirtualFs>) {
        self.tool_vfs = Some(vfs);
    }

    /// The filesystem the dispatcher should build tool contexts with, if
    /// one was installed. `None` means use the default `RealFs`.
    pub fn tool_vfs(&self) -> Option<Arc<dyn crate::vfs::VirtualFs>> {
        self.tool_vfs.clone()
    }

    pub fn set_workspace_policy(&mut self, policy: Arc<crate::workspace_policy::WorkspacePolicy>) {
        self.workspace_policy = Some(policy);
    }

    pub fn workspace_policy(&self) -> Option<Arc<crate::workspace_policy::WorkspacePolicy>> {
        self.workspace_policy.clone()
    }

    /// Where this session's oversized tool results are spilled
    /// (FerroxLabs/wayland#1097).
    ///
    /// THE decision, taken here rather than at the engine's shed site, because
    /// the registry is what holds the two facts it depends on — the session's
    /// workspace policy and the fact that the same session's file tools read
    /// through [`tool_vfs`](Self::tool_vfs). A spill directory chosen without
    /// them is a file the engine writes and then tells the model to `Read`
    /// from outside its own jail.
    pub fn spill_storage(&self) -> crate::tool_result_storage::StorageDir {
        crate::tool_result_storage::StorageDir::for_optional_session(self.workspace_policy())
    }

    pub fn set_sandbox_runtime(&mut self, runtime: Arc<wcore_sandbox::SandboxRegistry>) {
        self.sandbox_runtime = runtime;
    }

    pub fn sandbox_runtime(&self) -> Arc<wcore_sandbox::SandboxRegistry> {
        Arc::clone(&self.sandbox_runtime)
    }

    /// Drop every registered tool for which `keep` returns `false`.
    ///
    /// Applied once, AFTER the full tool set is registered, to enforce a
    /// reduced toolset on a restricted engine (e.g. a channel-originated
    /// engine that must not expose host filesystem/shell tools to a remote
    /// sender). Filtering at the registry — rather than only omitting tools
    /// from the LLM schema — means a dropped tool is also un-dispatchable:
    /// `get()` returns `None`, so even a hallucinated call cannot reach it.
    /// The matching circuit-breaker entries are pruned too.
    pub fn retain<F>(&mut self, keep: F)
    where
        F: Fn(&dyn Tool) -> bool,
    {
        let mut kept_names: Vec<String> = Vec::with_capacity(self.tools.len());
        self.tools.retain(|t| {
            let keep_it = keep(t.as_ref());
            if keep_it {
                kept_names.push(t.name().to_string());
            }
            keep_it
        });
        self.breakers
            .write()
            .retain(|name, _| kept_names.contains(name));
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        // External-service tools (web, vision, transcription, gitlab,
        // notion, discord, …) ship a `Null*Backend` default and override
        // `is_available()` to return false until the host wires a real
        // backend. Silently skipping unavailable tools here keeps the
        // model from ever seeing a tool it cannot successfully call —
        // which used to manifest as "running forever" in the TUI because
        // the tool sat in AwaitingApproval while the agent burned turns
        // retrying a call that always errored.
        if !tool.is_available() {
            tracing::info!(
                tool = %tool.name(),
                "skipping registration of tool whose backend is not configured"
            );
            return;
        }
        self.breakers
            .write()
            .entry(tool.name().to_string())
            .or_insert_with(|| CircuitBreaker::new(default_breaker_cfg()));
        self.tools.push(tool);
    }

    /// Replace any previously-registered tool with the same `name()` and
    /// install the new one. Preserves the existing circuit-breaker state
    /// (the breaker is per-name and persists across re-registration).
    ///
    /// Use this for the boot-time `Null*Transport` → real-transport
    /// upgrade pattern (audit 2026-05-24 fix): the host registers a
    /// schema-visible default at the registry-construction site, then
    /// later upgrades the implementation once host-side resources
    /// (channel managers, async runtimes) are available.
    pub fn replace_by_name(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.retain(|t| t.name() != name);
        self.breakers
            .write()
            .entry(name)
            .or_insert_with(|| CircuitBreaker::new(default_breaker_cfg()));
        self.tools.push(tool);
    }

    /// Rebuild the registered `ToolSearch` tool from the live registry.
    ///
    /// `ToolSearch` deliberately owns a snapshot so searches do not hold a
    /// registry lock. Any tool added after bootstrap (for example a deferred
    /// config MCP or `/mcp add`) therefore has to replace that snapshot before
    /// it can be discovered. Reapply the configured cold split so a newly
    /// added non-deferred proxy is still searchable when global cold deferral
    /// is enabled.
    ///
    /// The rebuilt tool is handed the registry's [`Self::hydrated_tools`]
    /// handle, NOT a fresh one. A refresh runs on bootstrap, on every
    /// config-MCP registration, on `/mcp add`, and from the TUI engine
    /// bridge; without the shared handle each of those would forget what the
    /// engine had already admitted and hand the model back the stale
    /// pre-hydration answer, which is what it reads as "still not loaded".
    pub fn refresh_tool_search_catalog(
        &mut self,
        defer_cold: &wcore_config::tools::DeferColdConfig,
    ) {
        let mut snapshot = self.to_tool_defs();
        snapshot.retain(|def| def.name != "ToolSearch");
        if defer_cold.enabled {
            apply_cold_deferral(&mut snapshot, &defer_cold.hot_allowlist);
        }
        self.replace_by_name(Box::new(
            crate::tool_search::ToolSearchTool::with_hydration(
                snapshot,
                self.hydrated_tools.clone(),
            ),
        ));
    }

    /// Find a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// AUDIT B-4 — is the named tool's circuit breaker currently open?
    ///
    /// The breaker (3 failures / 30s trips it; 60s open) was previously
    /// reachable ONLY through `ToolDispatcher::dispatch[_with_ctx]`,
    /// which the agent's main tool loop bypasses (it calls `get()` +
    /// `execute_with_ctx()` directly). This inherent method lets the
    /// orchestration dispatch path consult the breaker without routing
    /// through the full `ToolDispatcher` trait. Returns `false` for an
    /// unregistered tool (nothing to short-circuit).
    pub fn breaker_is_open(&self, name: &str) -> bool {
        self.breakers
            .read()
            .get(name)
            .map(|b| b.is_open())
            .unwrap_or(false)
    }

    /// AUDIT B-4 — record a dispatch outcome against the named tool's
    /// circuit breaker. `is_error == true` records a failure (a timeout
    /// or panic counts here too); `false` records a success, which
    /// resets the failure window. No-op for an unregistered tool.
    pub fn record_breaker_outcome(&self, name: &str, is_error: bool) {
        if let Some(breaker) = self.breakers.read().get(name) {
            if is_error {
                breaker.record_failure();
            } else {
                breaker.record_success();
            }
        }
    }

    /// Record a dispatch outcome, skipping errors the tool itself attributes
    /// to the caller's request rather than to its own machinery.
    ///
    /// See [`Tool::error_is_tool_fault`]. Such an outcome is NEUTRAL — the
    /// breaker's failure window is left exactly as it was, so a genuinely
    /// flaky tool is still caught while a shell reporting `exit 1` (or
    /// refusing a command it cannot deliver) no longer removes itself from
    /// the agent for a cooldown.
    pub fn record_dispatch_outcome(&self, name: &str, result: &ToolResult) {
        if result.is_error
            && let Some(tool) = self.get(name)
            && !tool.error_is_tool_fault(&result.content)
        {
            return;
        }
        self.record_breaker_outcome(name, result.is_error);
    }

    /// Record the outcome of one dispatched call against the human-contact
    /// latch. A no-op for every tool that does not declare
    /// [`crate::Tool::reaches_a_human`].
    ///
    /// A failure ARMS the latch; a success clears it. Counting is deliberately
    /// NOT used: measured on this row, a run that eventually reached the
    /// on-call took four attempts (three malformed-envelope errors, then a
    /// delivery), while the run that gave up and acted unsupervised made only
    /// two. No threshold separates "still trying" from "gave up", so the state
    /// tracked is the one that matters — whether the LAST word from the
    /// outbound route was a failure.
    pub fn record_human_reach_outcome(&self, name: &str, is_error: bool) {
        if !self.get(name).is_some_and(|tool| tool.reaches_a_human()) {
            return;
        }
        self.human_unreachable
            .store(is_error, std::sync::atomic::Ordering::Release);
    }

    /// Is the session's route to a human currently down?
    pub fn human_unreachable(&self) -> bool {
        self.human_unreachable
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Re-arm the human-contact latch from durable recovery state.
    ///
    /// Deliberately arm-only, with no clearing counterpart. Restoring a
    /// recovery checkpoint must be able to re-freeze a turn resumed after a
    /// crash, and must never be able to LIFT a freeze this process has
    /// already armed. Clearing stays with the two events that earn it: a
    /// delivery, and a fresh user turn.
    pub fn arm_human_unreachable(&self) {
        self.human_unreachable
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Clear the human-contact latch.
    ///
    /// Called where the engine clears the per-tool breakers: at the start of a
    /// new USER turn. A fresh user message is itself proof that a human is
    /// present and reachable, which is the only evidence that should lift the
    /// freeze other than a delivered message.
    pub fn clear_human_unreachable(&self) {
        self.human_unreachable
            .store(false, std::sync::atomic::Ordering::Release);
    }

    /// #403 — clear every tool circuit breaker back to Closed. Called at the
    /// start of each new user turn so transient per-turn failures (a flaky
    /// `web`/`WebFetch` burst that opened the breaker) don't leave tools wedged
    /// across independent user messages, which made the session look dead.
    /// Persistent failures simply re-open the breaker within the new turn.
    pub fn reset_all_breakers(&self) {
        for breaker in self.breakers.read().values() {
            breaker.record_success();
        }
    }

    /// Return the registered tool names whose live breaker state must be
    /// restored conservatively after a process restart.
    ///
    /// The result is sorted by tool name so callers can persist and compare it
    /// deterministically regardless of `HashMap` iteration order.
    pub fn breakers_requiring_conservative_restore(&self) -> Vec<String> {
        let breakers = self.breakers.read();
        let mut names: Vec<String> = breakers
            .iter()
            .filter(|(_, breaker)| breaker.requires_conservative_restore())
            .map(|(name, _)| name.clone())
            .collect();
        names.sort();
        names
    }

    /// Restore exactly the supplied circuit breakers conservatively.
    ///
    /// Every name is validated before any breaker is changed, so an invalid
    /// request is atomic. Supplied names are sorted and deduplicated for
    /// deterministic application. Breakers omitted from `tool_names` are left
    /// untouched; a fresh registry therefore keeps them Closed, while an
    /// already-live registry preserves their existing state.
    pub fn restore_breakers_conservatively(
        &self,
        tool_names: &[String],
    ) -> Result<(), BreakerRestoreError> {
        let breakers = self.breakers.read();
        let requested: std::collections::BTreeSet<&str> =
            tool_names.iter().map(String::as_str).collect();
        let unknown: Vec<String> = requested
            .iter()
            .filter(|name| !breakers.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect();
        if !unknown.is_empty() {
            return Err(BreakerRestoreError { names: unknown });
        }

        for name in requested {
            breakers
                .get(name)
                .expect("all breaker names were validated above")
                .restore_conservative();
        }
        Ok(())
    }

    /// Get all registered tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Count registered MCP tools by server without materializing provider
    /// schemas. Intended for read-only runtime diagnostics.
    pub fn mcp_tool_counts(&self) -> HashMap<String, u32> {
        let mut counts = HashMap::new();
        for tool in &self.tools {
            if let Some(server) = tool.mcp_server() {
                let count = counts.entry(server.to_string()).or_insert(0_u32);
                *count = count.saturating_add(1);
            }
        }
        counts
    }

    /// Remove every callable tool owned by one MCP server.
    ///
    /// Returns sorted removed display names for a deterministic host receipt.
    /// The caller must refresh the `ToolSearch` snapshot after this mutation.
    pub fn remove_mcp_server(&mut self, server: &str) -> Vec<String> {
        let mut removed = Vec::new();
        self.tools.retain(|tool| {
            if tool.mcp_server() == Some(server) {
                removed.push(tool.name().to_string());
                false
            } else {
                true
            }
        });
        let mut breakers = self.breakers.write();
        for name in &removed {
            breakers.remove(name);
        }
        drop(breakers);
        removed.sort();
        removed
    }

    /// Generate API tool definitions for all registered tools
    pub fn to_tool_defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
                server: t.mcp_server().map(str::to_string),
            })
            .collect()
    }

    /// Generate API tool definitions for tools matching a predicate.
    ///
    /// Used by plan mode to restrict the tool set sent to the LLM.
    pub fn to_tool_defs_filtered<F>(&self, filter: F) -> Vec<ToolDef>
    where
        F: Fn(&dyn Tool) -> bool,
    {
        self.tools
            .iter()
            .filter(|t| filter(t.as_ref()))
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
                server: t.mcp_server().map(str::to_string),
            })
            .collect()
    }
}

/// Layer D1 (token-opt): mark every tool NOT on the hot allowlist as
/// deferred, so providers serialize it as a name + truncated-description
/// stub instead of its full schema. The model hydrates a stub on demand via
/// `ToolSearch` (which is never deferred — it is the hydration path — and
/// is skipped here regardless of the allowlist).
///
/// CRITICAL caching constraint: this is a pure function of the def names
/// and the static allowlist — never of per-turn state — so applying it
/// every turn yields an identical hot/stub split and the serialized
/// `tools[]` array stays byte-identical across a conversation (guarded by
/// `tools_array_byte_stable_across_roundtrips` in `wcore-providers`).
///
/// Only the `deferred` flag is flipped; `input_schema`/`description` are
/// retained on the def so `ToolSearchTool` (which snapshots these defs) can
/// return the full schema on hydration. Tools already deferred (e.g. MCP
/// proxies) stay deferred.
pub fn apply_cold_deferral(defs: &mut [ToolDef], hot_allowlist: &[String]) {
    for def in defs.iter_mut() {
        if def.name == "ToolSearch" {
            continue;
        }
        if !hot_allowlist.iter().any(|hot| hot == &def.name) {
            def.deferred = true;
        }
    }
}

/// Layer D1 follow-up (hydrated-tool admission): un-defer every def whose name
/// the model has hydrated via `ToolSearch` this session, so the full schema
/// ships and the tool is genuinely callable (providers validate tool calls
/// against the CURRENT `tools[]` array).
///
/// Cache stability (FerroxLabs/wayland#1171): an admitted tool is APPENDED at
/// the tail, in `hydrated`'s FIRST-HYDRATION order — it is NOT reinstated at
/// the registry position it held while deferred. The `tools[]` array sits in
/// the cached prompt prefix, so a mid-array reinstatement rewrites every byte
/// after it and re-bills the whole prompt uncached; appending keeps the prefix
/// byte-identical and makes each hydration cost exactly its own new entries.
/// A hydrated name that was never deferred is already in the stable base and
/// is left where it is.
///
/// Cache stability (FerroxLabs/wayland#1209): move every DEFERRED def to the
/// TAIL of the array, preserving relative order inside both halves.
///
/// This is the ONE ordering discipline both deferral modes share, and it is
/// what makes [`admit_hydrated_tools`] safe. In catalog mode
/// ([`fold_deferred_into_catalog`]) the deferred defs are deleted from the
/// array outright, so pulling one out of the middle shifts nothing — the
/// stable prefix is "every hot tool". With the catalog fold OFF
/// (`builtin_tools.defer_cold.catalog = false`, per-tool stub entries) the
/// stubs SURVIVE onto the wire, interleaved with the hot tools at their
/// registry slots; admitting one then removed it from mid-array and appended
/// it, rewriting every byte after its old slot. Measured before this pass:
/// turn1 `[Bash, Delegate, Edit, Forge, Glob, Grep, Read, Spawn, ToolSearch,
/// Workflow, Write]` -> turn2 `[Bash, Edit, Forge, Glob, Grep, Read,
/// ToolSearch, Write, Delegate, Spawn, Workflow]`, first differing wire index
/// `Some(1)` — the whole prompt prefix re-billed uncached, which is the
/// wayland#1150 / wayland#1171 bug on a documented config path.
///
/// Sinking first gives stub mode the same shape catalog mode gets for free:
/// a byte-identical hot prefix for the life of the conversation, and a
/// mutable region confined to the tail.
///
/// Order-preserving and a pure function of the `deferred` flags, so it cannot
/// itself introduce per-turn churn: the same registry + the same deferral
/// decision yields the same array every turn.
///
/// Naturally deferred defs (`Tool::is_deferred`, e.g. MCP proxies) are sunk
/// too — they are wire stubs by the same mechanism and hydrating one shifted
/// the prefix in exactly the same way, including when `defer_cold` is off
/// entirely.
pub fn sink_deferred_to_tail(defs: &mut Vec<ToolDef>) {
    if !defs.iter().any(|def| def.deferred) {
        return;
    }
    let (hot, cold): (Vec<ToolDef>, Vec<ToolDef>) = defs.drain(..).partition(|def| !def.deferred);
    *defs = hot;
    defs.extend(cold);
}

/// `hydrated` is in FIRST-HYDRATION order.
pub fn admit_hydrated_tools(defs: &mut Vec<ToolDef>, hydrated: &[String]) {
    if hydrated.is_empty() {
        return;
    }
    let wanted: std::collections::HashSet<&str> = hydrated.iter().map(String::as_str).collect();
    let mut admitted: Vec<ToolDef> = Vec::new();
    let mut index = 0;
    while index < defs.len() {
        if defs[index].deferred && wanted.contains(defs[index].name.as_str()) {
            let mut def = defs.remove(index);
            def.deferred = false;
            admitted.push(def);
        } else {
            index += 1;
        }
    }
    // First-hydration order: an already-admitted tool holds its tail slot and
    // a newly hydrated one appends after it, so a second hydration is another
    // append rather than an insert between the first one's entries.
    admitted.sort_by_key(|def| {
        hydrated
            .iter()
            .position(|name| name == &def.name)
            .unwrap_or(usize::MAX)
    });
    defs.extend(admitted);
}

/// Layer D3 (token-opt): fold every deferred def OUT of the
/// tools[] array entirely, replacing the per-tool name-only stubs with ONE
/// compact catalog line appended to ToolSearch's description. Measured on
/// the reference workload the 43 stub entries cost ~2.5k tokens/request —
/// more than the hot full schemas — so removing them is the bigger half of
/// the deferral win.
///
/// Determinism / caching: deferred names are emitted sorted and deduped
/// (`BTreeSet`), so the catalog line is a pure function of the deferral
/// state; combined with the monotonic hydrated-tool union the line is
/// byte-stable across turns and changes exactly when the tools[] array
/// already changes (a hydration admission).
///
/// `catalog_max_chars` bounds the names portion of the line; overflow is
/// replaced by a `+N more not listed` suffix, keeping the line a
/// bounded directory so an MCP swarm cannot balloon the prompt while every
/// omitted tool stays discoverable through ToolSearch queries.
///
/// Fallback: when no non-deferred `ToolSearch` def is present there is no
/// surface to carry the catalog — the defs are returned unchanged (per-tool
/// stubs), never silently undiscoverable.
pub fn fold_deferred_into_catalog(
    mut defs: Vec<ToolDef>,
    catalog_max_chars: usize,
) -> Vec<ToolDef> {
    if !defs.iter().any(|d| !d.deferred && d.name == "ToolSearch") {
        return defs;
    }
    let names: std::collections::BTreeSet<String> = defs
        .iter()
        .filter(|d| d.deferred)
        .map(|d| d.name.clone())
        .collect();
    defs.retain(|d| !d.deferred);
    // Cache stability (FerroxLabs/wayland#1171): the catalog line is the ONE
    // part of a hydrating session's tools[] that legitimately changes — the
    // hydrated names leave the deferred list. Carried mid-array it would
    // rewrite every byte from its slot onward on the turn after any
    // hydration, which is the same whole-prefix invalidation a mid-array
    // admission causes. The carrier therefore moves to the TAIL: everything
    // ahead of it stays byte-identical for the life of the conversation.
    // `defs` is already free of deferred entries, so this finds the live
    // ToolSearch the guard above proved is present.
    if let Some(index) = defs.iter().position(|d| d.name == "ToolSearch") {
        let mut carrier = defs.remove(index);
        if !names.is_empty() {
            let catalog = render_deferred_catalog(&names, catalog_max_chars);
            carrier.description = format!("{} {}", carrier.description.trim_end(), catalog);
        }
        defs.push(carrier);
    }
    defs
}

/// Render the sorted, bounded deferred-tool inventory line for
/// [`fold_deferred_into_catalog`]. `max_chars` is a HARD bound on the
/// name-list portion — even the FIRST name is dropped when it alone exceeds
/// the budget (a pathological MCP name must not blow past the documented
/// cap). The fixed prefix and the constant-size `+N more` overflow suffix
/// sit outside the budget; omitted names remain discoverable via ToolSearch
/// queries.
///
/// The suffix STATES the omitted tools' reachability instead of advising a
/// search for them. Measured (Wayland Desktop / GPT-5.6 Sol, 2026-08-08):
/// with `search to discover` on the line, a model hunting two tools that did
/// not exist ran ten consecutive searches, every one `status=Success`. The
/// prompt's own inventory told it iterating was the way to find out.
fn render_deferred_catalog(names: &std::collections::BTreeSet<String>, max_chars: usize) -> String {
    const PREFIX: &str =
        "Deferred tools (name-only; load the full schema via this tool before calling): ";
    let total = names.len();
    let mut list = String::new();
    let mut included = 0usize;
    for name in names {
        let sep = if included == 0 { "" } else { ", " };
        if list.len() + sep.len() + name.len() > max_chars {
            break;
        }
        list.push_str(sep);
        list.push_str(name);
        included += 1;
    }
    let omitted = total - included;
    if omitted > 0 {
        if included > 0 {
            list.push_str(", ");
        }
        list.push_str(&format!(
            "+{omitted} more not listed — this tool searches every deferred \
             tool, listed or not"
        ));
    }
    format!("{PREFIX}{list}.")
}

#[async_trait]
impl ToolDispatcher for ToolRegistry {
    async fn dispatch(&self, tool: &str, input: serde_json::Value) -> ToolResult {
        // Check circuit breaker before executing. Row B-3: the human-contact
        // surface is exempt — backing it off removes the session's only route
        // to a person, which is the one failure this product must not absorb
        // quietly.
        if let Some(breaker) = self.breakers.read().get(tool)
            && breaker.is_open()
            && !self.get(tool).is_some_and(|t| t.reaches_a_human())
        {
            return ToolResult {
                content: format!(
                    "tool '{tool}' circuit open: too many recent failures, try again later"
                ),
                is_error: true,
            };
        }

        let result = match self.get(tool) {
            Some(t) => t.execute(input).await,
            None => {
                return ToolResult {
                    content: format!("tool '{tool}' not in registry"),
                    is_error: true,
                };
            }
        };

        // Record outcome. An errored result that the tool itself says is the
        // caller's request failing (see `Tool::error_is_tool_fault`) is
        // NEUTRAL: it neither records a failure nor clears the window.
        let counts = result.is_error
            && self
                .get(tool)
                .is_none_or(|t| t.error_is_tool_fault(&result.content));
        if let Some(breaker) = self.breakers.read().get(tool) {
            if result.is_error {
                if counts {
                    breaker.record_failure();
                }
            } else {
                breaker.record_success();
            }
        }
        // Row B-3: keep the human-contact latch in step on this path too, so
        // a send routed through `ToolDispatcher` (ScriptTool sub-steps, plugin
        // dispatchers) is not a hole in the freeze.
        self.record_human_reach_outcome(tool, result.is_error);

        result
    }

    /// W8b.2.A — propagate the caller's `ToolContext` to the resolved
    /// tool's `execute_with_ctx`. Lets `ScriptTool` thread its parent
    /// context (vfs, cancel, file_write_notifier) into every sub-step.
    async fn dispatch_with_ctx(
        &self,
        tool: &str,
        input: serde_json::Value,
        ctx: &crate::context::ToolContext,
    ) -> ToolResult {
        // Check circuit breaker before executing. Row B-3: the human-contact
        // surface is exempt — backing it off removes the session's only route
        // to a person, which is the one failure this product must not absorb
        // quietly.
        if let Some(breaker) = self.breakers.read().get(tool)
            && breaker.is_open()
            && !self.get(tool).is_some_and(|t| t.reaches_a_human())
        {
            return ToolResult {
                content: format!(
                    "tool '{tool}' circuit open: too many recent failures, try again later"
                ),
                is_error: true,
            };
        }

        let result = match self.get(tool) {
            Some(t) => t.execute_with_ctx(input, ctx).await,
            None => {
                return ToolResult {
                    content: format!("tool '{tool}' not in registry"),
                    is_error: true,
                };
            }
        };

        // Record outcome. An errored result that the tool itself says is the
        // caller's request failing (see `Tool::error_is_tool_fault`) is
        // NEUTRAL: it neither records a failure nor clears the window.
        let counts = result.is_error
            && self
                .get(tool)
                .is_none_or(|t| t.error_is_tool_fault(&result.content));
        if let Some(breaker) = self.breakers.read().get(tool) {
            if result.is_error {
                if counts {
                    breaker.record_failure();
                }
            } else {
                breaker.record_success();
            }
        }
        // Row B-3: keep the human-contact latch in step on this path too, so
        // a send routed through `ToolDispatcher` (ScriptTool sub-steps, plugin
        // dispatchers) is not a hole in the freeze.
        self.record_human_reach_outcome(tool, result.is_error);

        result
    }

    /// Returns the current `BreakerState` for a tool, or `None` if
    /// the tool is not registered. Used by tests and observability hooks.
    fn breaker_state(&self, tool: &str) -> Option<BreakerState> {
        self.breakers.read().get(tool).map(|b| b.state())
    }
}

#[cfg(test)]
mod human_reach_latch_tests {
    use super::*;
    use crate::Tool;
    use async_trait::async_trait;
    use serde_json::Value;
    use wcore_protocol::events::ToolCategory;
    use wcore_types::tool::{JsonSchema, ToolResult};

    /// Minimal tool whose only interesting property is whether it claims to
    /// reach a human and whether it errors.
    struct Probe {
        name: &'static str,
        human: bool,
        fails: bool,
    }

    #[async_trait]
    impl Tool for Probe {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "probe"
        }
        fn input_schema(&self) -> JsonSchema {
            serde_json::json!({"type": "object"})
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::Exec
        }
        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            false
        }
        fn reaches_a_human(&self) -> bool {
            self.human
        }
        async fn execute(&self, _input: Value) -> ToolResult {
            ToolResult {
                content: "probe".to_string(),
                is_error: self.fails,
            }
        }
    }

    fn registry(tools: Vec<Probe>) -> ToolRegistry {
        let mut r = ToolRegistry::new();
        for t in tools {
            r.register(Box::new(t));
        }
        r
    }

    #[test]
    fn a_failed_human_contact_arms_the_latch_and_a_delivery_clears_it() {
        let r = registry(vec![Probe {
            name: "send_message",
            human: true,
            fails: false,
        }]);
        assert!(!r.human_unreachable(), "a fresh session starts reachable");
        r.record_human_reach_outcome("send_message", true);
        assert!(r.human_unreachable());
        r.record_human_reach_outcome("send_message", false);
        assert!(!r.human_unreachable(), "a delivery must clear the latch");
    }

    #[test]
    fn an_ordinary_tool_failing_says_nothing_about_reachability() {
        let r = registry(vec![
            Probe {
                name: "send_message",
                human: true,
                fails: false,
            },
            Probe {
                name: "Bash",
                human: false,
                fails: true,
            },
        ]);
        r.record_human_reach_outcome("Bash", true);
        assert!(
            !r.human_unreachable(),
            "a failing shell command must not be read as losing the human"
        );
        // …and it must not CLEAR a real loss either.
        r.record_human_reach_outcome("send_message", true);
        r.record_human_reach_outcome("Bash", false);
        assert!(
            r.human_unreachable(),
            "an unrelated success must not lift the freeze"
        );
    }

    #[test]
    fn an_unregistered_name_is_inert() {
        let r = registry(vec![]);
        r.record_human_reach_outcome("send_message", true);
        assert!(
            !r.human_unreachable(),
            "a name this registry does not know cannot arm the latch"
        );
    }

    #[test]
    fn the_latch_clears_on_a_new_user_turn() {
        let r = registry(vec![Probe {
            name: "send_message",
            human: true,
            fails: false,
        }]);
        r.record_human_reach_outcome("send_message", true);
        assert!(r.human_unreachable());
        r.clear_human_unreachable();
        assert!(
            !r.human_unreachable(),
            "a fresh user message is itself a reachable human"
        );
    }

    #[tokio::test]
    async fn the_dispatcher_path_keeps_the_latch_in_step() {
        // ScriptTool and plugin dispatchers go through `ToolDispatcher`, not
        // the agent loop's `execute_single_with_streaming`. If that path did
        // not record, a send routed through it would be a hole in the freeze.
        let r = registry(vec![Probe {
            name: "send_message",
            human: true,
            fails: true,
        }]);
        let out = r.dispatch("send_message", serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(r.human_unreachable());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;
    use async_trait::async_trait;
    use wcore_protocol::events::ToolCategory;
    use wcore_types::tool::ToolResult;

    #[test]
    fn workspace_policy_defaults_none_and_sets() {
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let mut reg = ToolRegistry::new();
        assert!(reg.workspace_policy().is_none());
        let dir = tempfile::tempdir().unwrap();
        let policy = Arc::new(WorkspacePolicy::trusted_local(dir.path()));
        reg.set_workspace_policy(Arc::clone(&policy));
        assert_eq!(reg.workspace_policy().unwrap().root(), policy.root());
    }

    #[test]
    fn sandbox_runtime_is_preserved_by_arc_identity() {
        let mut reg = ToolRegistry::new();
        let runtime = Arc::new(wcore_sandbox::SandboxRegistry::new(Arc::new(
            wcore_sandbox::FailClosedBackend::new(),
        )));
        reg.set_sandbox_runtime(Arc::clone(&runtime));

        assert!(Arc::ptr_eq(&runtime, &reg.sandbox_runtime()));
    }

    #[test]
    fn sandbox_runtime_defaults_fail_closed() {
        assert_eq!(
            ToolRegistry::new().sandbox_runtime().backend_name(),
            "fail_closed"
        );
    }

    /// A minimal Tool implementation used only in tests
    struct MockTool {
        tool_name: String,
        tool_description: String,
        tool_category: ToolCategory,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            &self.tool_description
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            self.tool_category
        }
    }

    /// Helper to create a MockTool with the given name and description
    fn make_tool(name: &str, description: &str) -> Box<MockTool> {
        Box::new(MockTool {
            tool_name: name.to_string(),
            tool_description: description.to_string(),
            tool_category: ToolCategory::Info,
        })
    }

    fn make_tool_with_category(
        name: &str,
        description: &str,
        category: ToolCategory,
    ) -> Box<MockTool> {
        Box::new(MockTool {
            tool_name: name.to_string(),
            tool_description: description.to_string(),
            tool_category: category,
        })
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("my_tool", "does something"));

        let found = registry.get("my_tool");
        assert!(
            found.is_some(),
            "registered tool should be retrievable by name"
        );
        assert_eq!(found.unwrap().name(), "my_tool");
    }

    #[test]
    fn test_get_nonexistent_returns_none() {
        let registry = ToolRegistry::new();

        let result = registry.get("ghost");
        assert!(
            result.is_none(),
            "looking up an unregistered name should return None"
        );
    }

    #[test]
    fn test_tool_names() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("alpha", "first tool"));
        registry.register(make_tool("beta", "second tool"));
        registry.register(make_tool("gamma", "third tool"));

        let mut names = registry.tool_names();
        names.sort(); // sort for a stable assertion order
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_to_tool_defs() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("tool_a", "description A"));
        registry.register(make_tool("tool_b", "description B"));

        let defs = registry.to_tool_defs();
        assert_eq!(
            defs.len(),
            2,
            "to_tool_defs should return one entry per registered tool"
        );

        // Collect (name, description) pairs for assertion independent of order
        let mut pairs: Vec<(&str, &str)> = defs
            .iter()
            .map(|d| (d.name.as_str(), d.description.as_str()))
            .collect();
        pairs.sort();

        assert_eq!(pairs[0], ("tool_a", "description A"));
        assert_eq!(pairs[1], ("tool_b", "description B"));

        // Verify the input_schema field is populated correctly
        let expected_schema = serde_json::json!({"type": "object"});
        for def in &defs {
            assert_eq!(def.input_schema, expected_schema);
        }
    }

    // --- retain / tool_vfs tests ---

    #[test]
    fn retain_drops_unmatched_tools_and_prunes_breakers() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool_with_category(
            "Read",
            "fs read",
            ToolCategory::Info,
        ));
        registry.register(make_tool_with_category("Bash", "shell", ToolCategory::Exec));
        registry.register(make_tool_with_category("web", "net", ToolCategory::Info));

        // Keep only "web".
        registry.retain(|t| t.name() == "web");

        let mut names = registry.tool_names();
        names.sort();
        assert_eq!(names, vec!["web"], "only the kept tool survives");
        // Dropped tools are un-dispatchable, not merely hidden from the schema.
        assert!(registry.get("Read").is_none());
        assert!(registry.get("Bash").is_none());
        // Breaker entries for dropped tools are pruned; the survivor keeps one.
        assert!(!registry.breaker_is_open("web"), "survivor breaker intact");
        assert!(registry.breakers.read().contains_key("web"));
        assert!(!registry.breakers.read().contains_key("Read"));
        assert!(!registry.breakers.read().contains_key("Bash"));
    }

    #[test]
    fn tool_vfs_defaults_none_and_round_trips() {
        let mut registry = ToolRegistry::new();
        assert!(
            registry.tool_vfs().is_none(),
            "default is unconfined RealFs"
        );
        registry.set_tool_vfs(Arc::new(crate::vfs::RealFs));
        assert!(registry.tool_vfs().is_some(), "installed vfs is observable");
    }

    #[test]
    fn conservative_restore_candidates_are_sorted_and_include_closed_history() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("zeta", "last"));
        registry.register(make_tool("alpha", "first"));
        registry.register(make_tool("middle", "middle"));

        registry.record_breaker_outcome("zeta", true);
        registry.record_breaker_outcome("alpha", true);

        assert_eq!(
            registry.breakers_requiring_conservative_restore(),
            vec!["alpha".to_string(), "zeta".to_string()]
        );
        assert_eq!(
            registry.breakers.read()["alpha"].state(),
            BreakerState::Closed,
            "a below-threshold failure is still restart-relevant"
        );
    }

    #[test]
    fn conservative_restore_mutates_only_supplied_breakers() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("alpha", "already has history"));
        registry.register(make_tool("beta", "restore this"));
        registry.register(make_tool("gamma", "leave fresh"));
        registry.record_breaker_outcome("alpha", true);

        registry
            .restore_breakers_conservatively(&["beta".to_string(), "beta".to_string()])
            .unwrap();

        let breakers = registry.breakers.read();
        assert_eq!(breakers["alpha"].state(), BreakerState::Closed);
        assert!(breakers["alpha"].requires_conservative_restore());
        assert_eq!(breakers["beta"].state(), BreakerState::Open);
        assert_eq!(breakers["gamma"].state(), BreakerState::Closed);
        assert!(!breakers["gamma"].requires_conservative_restore());
    }

    #[test]
    fn invalid_conservative_restore_is_atomic_and_reports_sorted_names() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("alpha", "registered"));

        let error = registry
            .restore_breakers_conservatively(&[
                "zeta".to_string(),
                "alpha".to_string(),
                "missing".to_string(),
                "zeta".to_string(),
            ])
            .unwrap_err();

        assert_eq!(
            error,
            BreakerRestoreError {
                names: vec!["missing".to_string(), "zeta".to_string()]
            }
        );
        assert_eq!(
            registry.breakers.read()["alpha"].state(),
            BreakerState::Closed,
            "validation failure must not partially restore registered names"
        );
    }

    // --- to_tool_defs_filtered tests ---

    #[test]
    fn filtered_by_category_returns_matching_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool_with_category(
            "Read",
            "read files",
            ToolCategory::Info,
        ));
        registry.register(make_tool_with_category(
            "Write",
            "write files",
            ToolCategory::Edit,
        ));
        registry.register(make_tool_with_category(
            "Bash",
            "run commands",
            ToolCategory::Exec,
        ));
        registry.register(make_tool_with_category(
            "ExitPlanMode",
            "exit plan mode",
            ToolCategory::Info,
        ));

        let defs = registry.to_tool_defs_filtered(|t| t.category() == ToolCategory::Info);

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"ExitPlanMode"));
        assert!(!names.contains(&"Write"));
        assert!(!names.contains(&"Bash"));
    }

    #[test]
    fn filtered_by_name_excludes_specific_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("alpha", "first"));
        registry.register(make_tool("beta", "second"));
        registry.register(make_tool("gamma", "third"));

        let defs = registry.to_tool_defs_filtered(|t| t.name() != "beta");

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"gamma"));
        assert!(!names.contains(&"beta"));
    }

    #[test]
    fn filtered_accept_all_matches_to_tool_defs() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("a", "tool a"));
        registry.register(make_tool("b", "tool b"));

        let all = registry.to_tool_defs();
        let filtered = registry.to_tool_defs_filtered(|_| true);

        assert_eq!(all.len(), filtered.len());
        for (a, f) in all.iter().zip(filtered.iter()) {
            assert_eq!(a.name, f.name);
        }
    }

    #[test]
    fn filtered_reject_all_returns_empty() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("a", "tool a"));

        let defs = registry.to_tool_defs_filtered(|_| false);
        assert!(defs.is_empty());
    }

    #[test]
    fn filtered_empty_registry_returns_empty() {
        let registry = ToolRegistry::new();
        let defs = registry.to_tool_defs_filtered(|_| true);
        assert!(defs.is_empty());
    }

    // --- deferred flag tests ---

    /// A minimal Tool that overrides is_deferred() to return true
    struct DeferredMockTool {
        tool_name: String,
    }

    #[async_trait]
    impl Tool for DeferredMockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            "a deferred tool"
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}})
        }

        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }

        fn is_deferred(&self) -> bool {
            true
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }
    }

    /// FerroxLabs/wayland#1171: an admitted tool must APPEND at the tail in
    /// first-hydration order, not reappear at the registry slot it held while
    /// deferred. A mid-array reinstatement rewrites every cached byte after
    /// it; an append leaves the prefix byte-identical.
    #[test]
    fn admit_hydrated_tools_appends_in_first_hydration_order() {
        let def = |name: &str, deferred: bool| ToolDef {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred,
            server: None,
        };
        // Registry order interleaves the deferred tools with the hot ones.
        let mut defs = vec![
            def("Bash", false),
            def("Delegate", true),
            def("Read", false),
            def("Spawn", true),
            def("Write", false),
        ];
        // Spawn hydrated FIRST, Delegate second.
        admit_hydrated_tools(&mut defs, &["Spawn".to_string(), "Delegate".to_string()]);

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Bash", "Read", "Write", "Spawn", "Delegate"]);
        assert!(
            defs.iter().all(|d| !d.deferred),
            "an admitted tool must ship its full schema"
        );
    }

    /// A hydrated name that was never deferred is already in the stable base:
    /// moving it to the tail would itself be the prefix rewrite #1171 fixes.
    #[test]
    fn admit_hydrated_tools_leaves_an_already_hot_tool_in_place() {
        let def = |name: &str, deferred: bool| ToolDef {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred,
            server: None,
        };
        let mut defs = vec![def("Bash", false), def("Read", false), def("Spawn", true)];
        admit_hydrated_tools(&mut defs, &["Bash".to_string()]);

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Bash", "Read", "Spawn"]);
        assert!(defs[2].deferred, "an unhydrated stub stays deferred");
    }

    #[test]
    fn to_tool_defs_includes_deferred_flag() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("core_tool", "a core tool"));
        let defs = registry.to_tool_defs();
        assert!(!defs[0].deferred, "default tools should not be deferred");
    }

    #[test]
    fn to_tool_defs_deferred_tool_flagged() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredMockTool {
            tool_name: "lazy_tool".to_string(),
        }));
        let defs = registry.to_tool_defs();
        assert!(defs[0].deferred, "deferred tool should have deferred=true");
    }

    // --- apply_cold_deferral tests (Layer D1) ---

    #[test]
    fn cold_deferral_is_pure_function_of_allowlist() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("Read", "read files"));
        registry.register(make_tool("web", "search the web"));
        registry.register(make_tool("ToolSearch", "hydrate deferred tools"));

        let hot = vec!["Read".to_string()];

        // Applying to two independently-generated def lists yields the
        // identical split — no per-turn state involved.
        let mut turn1 = registry.to_tool_defs();
        let mut turn2 = registry.to_tool_defs();
        apply_cold_deferral(&mut turn1, &hot);
        apply_cold_deferral(&mut turn2, &hot);

        for defs in [&turn1, &turn2] {
            let by_name = |n: &str| defs.iter().find(|d| d.name == n).unwrap();
            assert!(!by_name("Read").deferred, "hot tool stays full");
            assert!(by_name("web").deferred, "cold tool defers");
            assert!(
                !by_name("ToolSearch").deferred,
                "ToolSearch is the hydration path — never deferred"
            );
            // The full schema survives on the def (only the flag flips) so
            // ToolSearch hydration can return it.
            assert_eq!(
                by_name("web").input_schema,
                serde_json::json!({"type": "object"})
            );
        }
        assert_eq!(turn1.len(), turn2.len());
        for (a, b) in turn1.iter().zip(turn2.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.deferred, b.deferred);
        }
    }

    // --- fold_deferred_into_catalog tests (Layer D3) ---

    fn catalog_def(name: &str, deferred: bool) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: format!("{name} description"),
            input_schema: serde_json::json!({"type": "object"}),
            deferred,
            server: None,
        }
    }

    #[test]
    fn catalog_line_is_sorted_deterministic_and_replaces_stub_entries() {
        let defs = vec![
            catalog_def("ToolSearch", false),
            catalog_def("Read", false),
            catalog_def("zulu_tool", true),
            catalog_def("alpha_tool", true),
            catalog_def("mike_tool", true),
        ];

        let folded = fold_deferred_into_catalog(defs.clone(), 4096);

        // No deferred entries survive in the array.
        assert!(
            folded.iter().all(|d| !d.deferred),
            "no stub entries may remain"
        );
        assert_eq!(folded.len(), 2, "only non-deferred defs remain");

        // The catalog line is on ToolSearch, sorted, name-only.
        let ts = folded.iter().find(|d| d.name == "ToolSearch").unwrap();
        assert!(
            ts.description.contains("alpha_tool, mike_tool, zulu_tool"),
            "sorted name-only inventory: {}",
            ts.description
        );
        assert!(
            !ts.description.contains("alpha_tool description")
                && !ts.description.contains("zulu_tool description"),
            "no per-tool description text leaks into the catalog (names only): {}",
            ts.description
        );

        // Byte-stable: same fold from a reordered input.
        let mut reordered = defs;
        reordered.reverse();
        let folded2 = fold_deferred_into_catalog(reordered, 4096);
        let ts2 = folded2.iter().find(|d| d.name == "ToolSearch").unwrap();
        assert_eq!(
            ts.description, ts2.description,
            "catalog line must be byte-identical regardless of input order"
        );
    }

    #[test]
    fn catalog_truncates_with_more_marker_at_cap() {
        let mut defs = vec![catalog_def("ToolSearch", false)];
        for i in 0..50 {
            defs.push(catalog_def(&format!("mcp__srv__tool_{i:03}"), true));
        }
        // Budget fits only a handful of ~18-char names.
        let folded = fold_deferred_into_catalog(defs, 60);
        let ts = folded.iter().find(|d| d.name == "ToolSearch").unwrap();
        // Truncation accounting only — the marker's WORDING is pinned by
        // `the_overflow_marker_*` below, so this test does not have to move
        // when that sentence is retuned.
        assert!(
            ts.description.contains(" more"),
            "overflow must be summarized: {}",
            ts.description
        );
        // The first (sorted) name is present; a late name is not.
        assert!(ts.description.contains("mcp__srv__tool_000"));
        assert!(!ts.description.contains("mcp__srv__tool_049"));
        // +N accounting: included + omitted = 50.
        let omitted: usize = ts
            .description
            .split("+")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .and_then(|s| s.parse().ok())
            .expect("+N marker present");
        let included = ts.description.matches("mcp__srv__tool_").count();
        assert_eq!(included + omitted, 50);
    }

    /// The overflow marker as the model reads it — everything from `+N`
    /// onward. Shared by the two tests below so they cannot drift apart.
    fn overflow_marker(description: &str) -> String {
        let at = description
            .find('+')
            .unwrap_or_else(|| panic!("no `+N` overflow marker in: {description}"));
        description[at..].to_string()
    }

    /// A catalogue with far more deferred tools than the budget can name.
    fn overflowing_catalog(max_chars: usize) -> String {
        let mut defs = vec![catalog_def("ToolSearch", false)];
        for i in 0..50 {
            defs.push(catalog_def(&format!("mcp__srv__tool_{i:03}"), true));
        }
        fold_deferred_into_catalog(defs, max_chars)
            .iter()
            .find(|d| d.name == "ToolSearch")
            .expect("ToolSearch carries the catalog")
            .description
            .clone()
    }

    /// The catalogue must not TELL the model to go searching.
    ///
    /// Measured, Wayland Desktop / GPT-5.6 Sol, 2026-08-08: ten consecutive
    /// ToolSearch calls, every one `status=Success`, no matcher miss. The
    /// model was hunting a web-search tool and a `research-advisor` skill
    /// that DO NOT EXIST — one dead end, then four rephrasings. The overflow
    /// marker read `+N more — search to discover`, i.e. the prompt's own
    /// inventory line advised discovery-by-iteration, and until 0b94370f the
    /// miss said nothing to stop it.
    ///
    /// The marker's job is to be an accurate DIRECTORY footnote: N tools are
    /// not named here and are still reachable. It is not a suggested action.
    ///
    /// MUTANT: restore `search to discover` and this fails.
    #[test]
    fn the_overflow_marker_does_not_advise_discovery_by_searching() {
        let marker = overflow_marker(&overflowing_catalog(60));
        for invitation in [
            "search to discover",
            "to discover",
            "search again",
            "keep searching",
        ] {
            assert!(
                !marker.to_lowercase().contains(invitation),
                "the overflow marker must not read as an instruction to go \
                 searching ({invitation:?}); got: {marker}"
            );
        }
    }

    /// NEGATIVE CONTROL for the test above. "Do not invite a search loop" is
    /// trivially satisfiable by saying nothing, or by implying the omitted
    /// tools are gone — both of which are WORSE than the invitation, because
    /// a model that believes a tool is unavailable stops looking for a tool
    /// that is right there.
    ///
    /// MUTANT: collapse the marker to a bare `+{omitted} more` and this fails
    /// while `the_overflow_marker_does_not_advise_discovery_by_searching`
    /// stays green.
    #[test]
    fn the_overflow_marker_still_says_the_omitted_tools_are_reachable() {
        let marker = overflow_marker(&overflowing_catalog(60));
        let lower = marker.to_lowercase();

        assert!(
            ["searches", "reachable", "loadable", "findable"]
                .iter()
                .any(|w| lower.contains(w)),
            "the marker must state that the unlisted tools are still \
             reachable through this tool; got: {marker}"
        );
        for absent in ["unavailable", "not available", "cannot be", "hidden"] {
            assert!(
                !lower.contains(absent),
                "the marker must not imply the unlisted tools are gone \
                 ({absent:?}); got: {marker}"
            );
        }
    }

    /// Codex verify finding: `catalog_max_chars` must be a HARD bound. The
    /// first renderer version exempted the first name from the length check,
    /// so a single pathological MCP name could blow past the documented cap.
    #[test]
    fn catalog_cap_is_hard_even_for_the_first_name() {
        let long_name = format!("mcp__srv__{}", "x".repeat(120));
        let defs = vec![
            catalog_def("ToolSearch", false),
            catalog_def(&long_name, true),
            catalog_def(&format!("{long_name}_2"), true),
        ];
        // Budget smaller than the (sorted-first) long name: ZERO names ship;
        // everything collapses into the +N marker.
        let folded = fold_deferred_into_catalog(defs, 40);
        let ts = folded.iter().find(|d| d.name == "ToolSearch").unwrap();
        assert!(
            !ts.description.contains(&long_name),
            "an over-budget first name must NOT ship: {}",
            ts.description
        );
        assert!(
            ts.description.contains("+2 more"),
            "all names collapse into the omitted marker: {}",
            ts.description
        );
        assert!(
            !ts.description.contains(", +"),
            "no dangling separator when zero names are included: {}",
            ts.description
        );
        // The deferred entries are still folded out of the array.
        assert_eq!(folded.len(), 1);
    }

    #[test]
    fn catalog_without_tool_search_falls_back_to_stubs() {
        // No ToolSearch def → nothing can carry the catalog; deferred defs
        // must be returned unchanged (stub entries), never dropped into
        // undiscoverability.
        let defs = vec![catalog_def("Read", false), catalog_def("cold_tool", true)];
        let folded = fold_deferred_into_catalog(defs.clone(), 4096);
        assert_eq!(folded.len(), 2);
        assert!(folded.iter().any(|d| d.name == "cold_tool" && d.deferred));
    }

    #[test]
    fn catalog_no_deferred_is_a_noop() {
        let defs = vec![catalog_def("ToolSearch", false), catalog_def("Read", false)];
        let folded = fold_deferred_into_catalog(defs.clone(), 4096);
        assert_eq!(folded.len(), 2);
        let ts = folded.iter().find(|d| d.name == "ToolSearch").unwrap();
        assert_eq!(
            ts.description, "ToolSearch description",
            "no catalog suffix when nothing is deferred"
        );
    }

    /// Correctness gate for Layer D1: a cold-deferred tool must remain
    /// (1) discoverable via ToolSearch — which returns its FULL schema —
    /// and (2) callable through the registry (deferral only changes what
    /// the LLM sees, never dispatch).
    #[tokio::test]
    async fn cold_deferred_tool_hydrates_via_tool_search_and_dispatches() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("Read", "read files"));
        registry.register(make_tool("web", "search the web"));

        let mut defs = registry.to_tool_defs();
        apply_cold_deferral(&mut defs, &["Read".to_string()]);

        // Discoverable + hydratable: ToolSearch built on the deferred defs
        // returns the cold tool's name AND full parameters schema.
        let search = crate::tool_search::ToolSearchTool::new(defs);
        let found = search.execute(serde_json::json!({"query": "web"})).await;
        assert!(!found.is_error);
        assert!(found.content.contains("\"web\""), "cold tool discoverable");
        assert!(
            found.content.contains("parameters"),
            "hydration returns the full schema"
        );

        // Still callable: dispatch routes by name, unaffected by deferral.
        let result = registry.dispatch("web", serde_json::json!({})).await;
        assert!(!result.is_error, "deferred tool still dispatches");
        assert_eq!(result.content, "ok");
    }

    /// TEST A from the Wayland Desktop handoff, 2026-08-04. It asked whether
    /// `ToolSearchTool`'s construction-time snapshot is why MCP tools could not
    /// be found. This settles it: the snapshot IS frozen, and that is NOT the
    /// outage, because production rebuilds it.
    ///
    /// The handoff flagged an apparent contradiction — `bootstrap.rs` says
    /// "Late tool REGISTRATION is fully supported" while ToolSearch owns a
    /// private `Vec<ToolDef>` copy. Both are true. Late registration is
    /// supported BECAUSE `refresh_tool_search_catalog` exists to rebuild that
    /// copy, and every late-registration path calls it: bootstrap, the MCP tool
    /// proxy, `/mcp add`, and the TUI engine bridge.
    ///
    /// Corroborated live on this build: a config-declared stdio MCP server's
    /// tools ARE returned by ToolSearch. So discovery was never the failure —
    /// the real defects were the whole-query substring match (see
    /// `a_multi_word_query_matches_words_scattered_through_a_description`) and
    /// the absent callability signal.
    ///
    /// Pinned so the answer cannot rot: skip the refresh and a late tool goes
    /// silently undiscoverable, which is a real way to break every MCP server.
    #[tokio::test]
    async fn a_late_registered_tool_is_invisible_until_the_catalog_is_refreshed() {
        let defer_cold = wcore_config::tools::DeferColdConfig {
            enabled: true,
            hot_allowlist: vec!["Read".to_string()],
            catalog: false,
            catalog_max_chars: 4096,
        };

        let mut registry = ToolRegistry::new();
        registry.register(make_tool("Read", "read files"));
        registry.refresh_tool_search_catalog(&defer_cold);

        // Arrives AFTER the snapshot was taken — the shape of every config MCP
        // proxy and every `/mcp add`.
        registry.register(make_tool(
            "late_mcp_tool",
            "a tool registered after ToolSearch was built",
        ));

        let before = registry
            .get("ToolSearch")
            .expect("ToolSearch registered")
            .execute(serde_json::json!({"query": "late_mcp_tool"}))
            .await;
        // Assert on the not-found SENTINEL, not on absence of the name: the
        // miss message echoes the query back ("No deferred tools matching
        // \"late_mcp_tool\" found."), so a `contains(name)` check is true on
        // both branches and can never fail.
        assert!(
            before.content.starts_with("No deferred tools matching"),
            "the snapshot is taken at construction, so a later tool is invisible \
             until refreshed — got: {}",
            before.content
        );

        registry.refresh_tool_search_catalog(&defer_cold);

        let after = registry
            .get("ToolSearch")
            .expect("ToolSearch still registered")
            .execute(serde_json::json!({"query": "late_mcp_tool"}))
            .await;
        assert!(
            after.content.contains("late_mcp_tool"),
            "refresh_tool_search_catalog must make a late tool discoverable — \
             every production late-registration path relies on exactly this; \
             got: {}",
            after.content
        );
    }

    /// v0.9.1.1 F8 — the catalog the LLM sees must use the exact
    /// string each backend reports from `Tool::name()`. A mismatch
    /// here means the model is taught the tool is called X, the
    /// dispatcher routes only on Y, and every call comes back as
    /// "tool 'X' not in registry" → which the live drive surfaced
    /// as `cancelled text_to_speech · API 400 …` errors.
    ///
    /// The current `to_tool_defs()` builds the catalog directly from
    /// `t.name()`, so this property holds by construction. The test
    /// pins it so a future refactor that, say, lower-cases the
    /// catalog or rewrites snake_case to PascalCase before sending
    /// to the LLM is caught immediately.
    #[test]
    fn tool_catalog_names_match_backend_names_v0911() {
        let mut registry = ToolRegistry::new();
        // The real-world set the architecture audit cited as the
        // dispatcher-mismatch surface — file ops in PascalCase,
        // multimodal/integration tools in snake_case, plus the two
        // names (`web`, `homeassistant`) that already matched.
        let names = [
            "Bash",
            "Read",
            "Write",
            "Edit",
            "Grep",
            "Glob",
            "web",
            "WebFetch",
            "vision_analyze",
            "transcribe_audio",
            "image_generate",
            "text_to_speech",
            "github_api",
            "discord_server",
            "homeassistant",
        ];
        for name in names {
            registry.register(make_tool(name, "fixture"));
        }
        let defs = registry.to_tool_defs();
        // Build a name-keyed map from both sides so we compare equal
        // sets regardless of registration order.
        let catalog_names: std::collections::HashSet<String> =
            defs.iter().map(|d| d.name.clone()).collect();
        let backend_names: std::collections::HashSet<String> =
            registry.tool_names().into_iter().collect();
        assert_eq!(
            catalog_names, backend_names,
            "tool catalog names sent to the LLM must equal the set returned by Tool::name() \
             (catalog={catalog_names:?}, backend={backend_names:?})"
        );
        // And no name was rewritten in transit.
        for d in &defs {
            assert!(
                backend_names.contains(&d.name),
                "catalog name `{}` not present in backend names {:?}",
                d.name,
                backend_names
            );
        }
    }

    struct MockMcpTool {
        name: String,
        server: String,
    }

    #[async_trait]
    impl Tool for MockMcpTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "mcp fixture"
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }

        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            false
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult {
                content: "ok".into(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Mcp
        }

        fn mcp_server(&self) -> Option<&str> {
            Some(&self.server)
        }
    }

    #[test]
    fn removing_mcp_server_is_scoped_and_idempotent() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("Read", "built in"));
        registry.register(Box::new(MockMcpTool {
            name: "alpha_search".into(),
            server: "alpha".into(),
        }));
        registry.register(Box::new(MockMcpTool {
            name: "beta_search".into(),
            server: "beta".into(),
        }));

        assert_eq!(registry.remove_mcp_server("alpha"), ["alpha_search"]);
        assert!(registry.get("alpha_search").is_none());
        assert!(registry.get("beta_search").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.remove_mcp_server("alpha").is_empty());
    }
}
