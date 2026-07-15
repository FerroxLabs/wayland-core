use std::sync::Arc;

use async_trait::async_trait;

use wcore_config::config::Config;
use wcore_protocol::events::WorkflowChildTerminalState;
use wcore_providers::LlmProvider;
use wcore_swarm::{
    AgentReport, BlackboardCtx, DEFAULT_SHARD_SIZE, FleetDispatcher, FleetReducer, MeshAgent,
    ShardSummary,
};
use wcore_tools::bash::BashTool;
use wcore_tools::edit::EditTool;
use wcore_tools::glob::GlobTool;
use wcore_tools::grep::GrepTool;
use wcore_tools::read::ReadTool;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::write::WriteTool;
use wcore_types::message::{FinishReason, TokenUsage};

use crate::agents::bus::{AgentBus, AgentMessage, now_ms, preview};
use crate::agents::channel_sink::ChannelSink;
use crate::engine::AgentEngine;
use crate::orchestration::council::ProviderResolver;
use crate::output::OutputSink;
use crate::output::null_sink::NullSink;

// Re-export from wcore-types — single source of truth
pub use wcore_types::spawner::{ForkOverrides, Spawner, SubAgentConfig, SubAgentResult};

/// #661 (fail-loud) — build a [`SubAgentResult`] from a sub-agent's terminal
/// [`AgentResult`](crate::engine::AgentResult).
///
/// A run that terminated abnormally — the turn cap, a budget/context ceiling,
/// the retry-cap guardrail, or the runaway-loop breaker — returns `Ok` with
/// empty text and a non-`Stop` finish reason. Copying that into
/// `is_error: false` made the parent LLM read it as "the sub-agent completed and
/// found nothing", so it reasoned from false info. Instead derive `is_error`
/// from the finish reason, and when the terminated body is empty synthesize a
/// cause line so the failure is legible rather than a silent empty success.
fn subagent_ok_result(name: String, result: crate::engine::AgentResult) -> SubAgentResult {
    // A clean EndTurn is `Stop`. `MaxTurns`/`Error` are unambiguous abnormal
    // terminations. `Length` is ambiguous: a run aborted at the context/budget
    // ceiling returns `Length` with EMPTY text (a real failure), but a complete
    // answer that ends exactly at the output-token cap also returns `Length`
    // WITH usable text — a degraded-but-usable answer, not a failure. Flagging
    // the latter would wrongly drop it from council quorum (is_usable), so treat
    // a non-empty `Length` as success; only an empty `Length` is an error.
    let is_error = match result.finish_reason {
        FinishReason::Stop => false,
        FinishReason::Length => result.text.trim().is_empty(),
        FinishReason::MaxTurns | FinishReason::Error => true,
    };
    let text = if is_error && result.text.trim().is_empty() {
        format!(
            "[sub-agent terminated without completing its task: {}]",
            describe_finish_reason(result.finish_reason)
        )
    } else {
        result.text
    };
    SubAgentResult {
        name,
        text,
        usage: result.usage,
        turns: result.turns,
        is_error,
    }
}

fn relay_subagent_terminal(sink: Option<&ChannelSink>, result: &SubAgentResult) {
    let Some(sink) = sink else {
        return;
    };
    let terminal_state = if result.is_error {
        WorkflowChildTerminalState::Failed
    } else {
        WorkflowChildTerminalState::Succeeded
    };
    let terminal_message = if result.is_error {
        result.text.clone()
    } else {
        format!(
            "sub-agent '{}' completed ({} turns)",
            result.name, result.turns
        )
    };
    sink.relay_terminal(terminal_state, &terminal_message);
}

/// Human-readable cause for an abnormal sub-agent termination.
fn describe_finish_reason(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::MaxTurns => "reached the turn limit before finishing",
        FinishReason::Length => "hit a context, budget, or output-length limit",
        FinishReason::Error => "ended with an error",
        // Not reachable from the error branch (Stop == clean completion), but
        // keep the match total.
        FinishReason::Stop => "stopped",
    }
}

/// v0.8.0 Task J — preview cap for `AgentMessage::FirstMessage.content_preview`.
/// Kept small so a chatty parent's prompts don't bloat the broadcast
/// channel; subscribers that need the full prompt can correlate via the
/// agent name + parent_call_id and look it up out-of-band.
const FIRST_MESSAGE_PREVIEW_CHARS: usize = 200;

/// W7 F2 sibling-parameter for `spawn_parallel`. Lives in `wcore-agent`
/// (NOT `wcore-types`) because `ChannelSink` wraps a tokio mpsc Sender —
/// the dep would reverse the crate-dep graph if hung off `SubAgentConfig`.
/// One `SpawnExtras` per `spawn_parallel_with_extras` call; per-task
/// fields (if needed later) can move into a `Vec<SpawnExtras>` indexed-
/// by-config — flagged for W8+.
#[derive(Clone, Default)]
pub struct SpawnExtras {
    /// When `Some`, the sub-agent's engine uses this sink instead of `NullSink`.
    /// Parent's `parent_call_id` is captured in the `ChannelSink` itself.
    pub channel_sink: Option<Arc<ChannelSink>>,
    /// Optional friendly-name forwarded into `SubAgentResult.name` so the parent
    /// can correlate relays with their originating spawn task.
    pub agent_name: Option<String>,
    /// Parent's `call_id` for the `SpawnTool` invocation — used by the
    /// parent-side drain task when wrapping `SubAgentRelay` in `SubAgentEvent`.
    pub parent_call_id: Option<String>,
}

/// v0.8.0 Task J — small RAII helper that ensures every spawn path
/// publishes exactly one terminal lifecycle event. The spawner builds
/// one of these immediately after `Spawned` is published; on drop with
/// the default `outcome` it logs a `Errored("dropped")` so a panic in
/// the engine can't leave subscribers waiting for a terminal event.
/// Successful spawn paths overwrite the outcome before drop.
struct LifecycleGuard {
    bus: Option<Arc<AgentBus>>,
    agent: String,
    outcome: TerminalOutcome,
}

/// Owns parallel child tasks so dropping a parent dispatch future aborts the
/// children instead of detaching them onto the Tokio runtime.
struct SpawnTaskSet(Vec<tokio::task::JoinHandle<SubAgentResult>>);

impl Drop for SpawnTaskSet {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

#[derive(Debug, Clone)]
enum TerminalOutcome {
    /// Default — nothing fired yet. Drop publishes `Errored("dropped before completion")`.
    Pending,
    /// Spawner already published `Completed` / `Errored` — drop is a no-op.
    Published,
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        if let (Some(bus), TerminalOutcome::Pending) = (&self.bus, &self.outcome) {
            bus.publish(AgentMessage::Errored {
                agent: self.agent.clone(),
                error: "sub-agent dropped before completion".to_string(),
            });
        }
    }
}

/// Spawns independent child agents that share the parent's LLM provider.
///
/// Sub-agents use a [`NullSink`] so their streaming output is silently
/// discarded.  Results are collected via `engine.run()` and returned to the
/// parent which emits them as a single `tool_result` event — matching the
/// Claude Code pattern where only the parent writes to stdout.
pub struct AgentSpawner {
    provider: Arc<dyn LlmProvider>,
    base_config: Config,
    /// Immutable sandbox selected by the parent session. Every child registry
    /// receives this exact `Arc`; spawning must never re-read process-global
    /// sandbox settings or select a different backend mid-session.
    sandbox_runtime: Arc<wcore_sandbox::SandboxRegistry>,
    /// Immutable outbound-network authority inherited from the parent session.
    /// Child engines must never fall back to a process-global compatibility
    /// policy after the bootstrap task-local scope has exited.
    egress_policy: wcore_egress::SharedPolicy,
    /// Shared live posture authority for host-backed sessions. Read only when
    /// deriving a child config so runtime de-escalation applies to descendants
    /// that have not started yet.
    approval_manager: Option<Arc<wcore_protocol::ToolApprovalManager>>,
    /// v0.8.0 Task J — optional `AgentBus` for lifecycle event
    /// publication. `None` preserves the legacy "silent spawner"
    /// behaviour expected by older tests; production callers attach the
    /// engine's bus via `with_bus(...)`.
    bus: Option<Arc<AgentBus>>,
    /// Parent cancellation token. Every spawned child engine is bound to a
    /// `child_token()` of this, so a host cancel (Esc) propagates into running
    /// sub-agents and they stop at the next turn boundary instead of burning
    /// LLM calls to completion. Defaults to a detached, never-cancelled token
    /// for legacy callers; production attaches the engine's token via
    /// `with_cancel(...)`.
    cancel: tokio_util::sync::CancellationToken,
    /// Production session handle. Reads the active turn token at spawn time,
    /// so both per-turn host cancellation and immutable session expiry reach
    /// children. Legacy/test spawners continue to use `cancel`.
    session_runtime: Option<crate::cancel::SessionRuntimeHandle>,
    /// Crucible (Mixture-of-Providers) — optional resolver that turns a
    /// per-spawn `SubAgentConfig.provider` spec into a keyed provider. `None`
    /// (the default) preserves single-provider behaviour: every child inherits
    /// `self.provider`. Production bootstrap attaches a `CouncilProviderResolver`
    /// via `with_provider_resolver(...)`. MUST be propagated by
    /// `clone_for_spawn` or fleet/parallel proposers silently fall back to the
    /// parent provider (the cross-provider-diversity guard catches this).
    resolver: Option<Arc<dyn ProviderResolver>>,
    /// Crucible cost governance — the per-session/per-day spend tracker shared
    /// with the engine. `None` ⇒ no aggregate cap (the council enforces only its
    /// per-run pin). MUST be propagated by `clone_for_spawn`.
    budget_tracker: Option<Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>>,
    /// (session_id, user_id) the council charges against — same envelope as the
    /// parent turn. None ⇒ council spend is not charged. Propagated by clone_for_spawn.
    budget_identity: Option<(String, String)>,
    /// Provider-call admission tracker shared by the parent engine and every
    /// spawned child. This is distinct from the cap-less Crucible accumulator
    /// above: it enforces the finite session token/cost envelope.
    provider_budget_tracker: Option<Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>>,
    /// Stable provider-budget identity shared with every child engine.
    budget_session_id: Option<String>,
    /// Parent execution envelope. Spawn/fork paths derive child views from it
    /// so token, cost, process, runtime, and active-agent usage roll up.
    execution_budget: Option<wcore_budget::ExecutionBudgetView>,
    /// Keeps the standalone session's budget watcher alive for exactly as
    /// long as the spawner and its clones can dispatch child work.
    budget_guard: Option<Arc<crate::cancel::BudgetGuard>>,
}

/// Provider-spend, execution, and cancellation authority inherited by a
/// transient spawner that is created after session bootstrap (for example a
/// Council judge or an Anvil seat). Keeping these handles together prevents a
/// convenience constructor from silently minting a fresh budget.
#[derive(Clone)]
pub struct SpawnerBudgetGovernance {
    provider_budget_tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
    budget_session_id: String,
    execution_budget: wcore_budget::ExecutionBudgetView,
    cancel: tokio_util::sync::CancellationToken,
    budget_guard: Option<Arc<crate::cancel::BudgetGuard>>,
}

impl SpawnerBudgetGovernance {
    pub fn new(
        provider_budget_tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
        budget_session_id: impl Into<String>,
        execution_budget: wcore_budget::ExecutionBudgetView,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            provider_budget_tracker,
            budget_session_id: budget_session_id.into(),
            execution_budget,
            cancel,
            budget_guard: None,
        }
    }

    pub(crate) fn with_budget_guard(
        mut self,
        budget_guard: Arc<crate::cancel::BudgetGuard>,
    ) -> Self {
        self.budget_guard = Some(budget_guard);
        self
    }

    /// Stable Core session lineage shared by the parent and every transient
    /// child. Producer contracts use it only when no persisted host session
    /// identity has been bound to the output sink yet.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.budget_session_id
    }
}

impl AgentSpawner {
    pub fn new(provider: Arc<dyn LlmProvider>, config: Config) -> Self {
        let sandbox_runtime = ToolRegistry::new().sandbox_runtime();
        Self {
            provider,
            base_config: config,
            sandbox_runtime,
            egress_policy: wcore_egress::default_policy(),
            approval_manager: None,
            bus: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            session_runtime: None,
            resolver: None,
            budget_tracker: None,
            budget_identity: None,
            provider_budget_tracker: None,
            budget_session_id: None,
            execution_budget: None,
            budget_guard: None,
        }
    }

    /// Bind spawned children to the parent session's immutable sandbox.
    pub fn with_sandbox_runtime(mut self, runtime: Arc<wcore_sandbox::SandboxRegistry>) -> Self {
        self.sandbox_runtime = runtime;
        self
    }

    /// Bind every spawned child engine to the parent's session-owned egress
    /// policy, including children created after bootstrap has returned.
    pub fn with_egress_policy(mut self, policy: wcore_egress::SharedPolicy) -> Self {
        self.egress_policy = policy;
        self
    }

    /// Clone the immutable outbound authority inherited by child agents.
    pub(crate) fn egress_policy(&self) -> wcore_egress::SharedPolicy {
        Arc::clone(&self.egress_policy)
    }

    /// Bind child posture derivation to the host session's live manager.
    pub fn with_approval_manager(
        mut self,
        manager: Arc<wcore_protocol::ToolApprovalManager>,
    ) -> Self {
        self.approval_manager = Some(manager);
        self
    }

    /// Return the sandbox runtime inherited by spawned children.
    pub fn sandbox_runtime(&self) -> &Arc<wcore_sandbox::SandboxRegistry> {
        &self.sandbox_runtime
    }

    fn child_tool_registry(&self, allowed: &[String]) -> ToolRegistry {
        build_tool_registry(allowed, Arc::clone(&self.sandbox_runtime))
    }

    /// Bind the spawner to the parent engine's cancellation token so a host
    /// cancel propagates into every spawned sub-agent. Production bootstrap
    /// attaches the engine's `cancel_token()` here, alongside `with_bus(...)`.
    pub fn with_cancel(mut self, cancel: tokio_util::sync::CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Bind production spawning to the session's shared active-turn handle.
    pub(crate) fn with_session_runtime(
        mut self,
        runtime: crate::cancel::SessionRuntimeHandle,
    ) -> Self {
        self.session_runtime = Some(runtime);
        self
    }

    fn active_cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.session_runtime
            .as_ref()
            .map(crate::cancel::SessionRuntimeHandle::active_turn_token)
            .unwrap_or_else(|| self.cancel.clone())
    }

    /// v0.8.0 Task J — attach an `AgentBus` so every `spawn_one` /
    /// `spawn_parallel*` / `spawn_fork` call publishes lifecycle events
    /// (Spawned → FirstMessage → Completed | Errored). Builder pattern
    /// because production bootstrap (`bootstrap.rs`) constructs the
    /// spawner before the engine's bus is finalised — the bus pointer
    /// is attached at the end of `apply_initialize_outcome` once the
    /// engine has been built.
    pub fn with_bus(mut self, bus: Arc<AgentBus>) -> Self {
        self.bus = Some(bus);
        self
    }

    /// Test/inspection helper — returns the attached `AgentBus` if any.
    pub fn bus(&self) -> Option<&Arc<AgentBus>> {
        self.bus.as_ref()
    }

    /// The attached council provider resolver, if any. The council executor
    /// reads it from the spawner so there is a single resolver source (the one
    /// that also keys per-proposer spawns) — no chance of a mismatched pair.
    pub fn provider_resolver(&self) -> Option<&Arc<dyn ProviderResolver>> {
        self.resolver.as_ref()
    }

    /// Crucible — attach a [`ProviderResolver`] so a `SubAgentConfig.provider`
    /// pin resolves to a keyed provider (a different LLM provider per council
    /// member). Builder pattern: production bootstrap constructs a
    /// `CouncilProviderResolver` once and attaches it here.
    pub fn with_provider_resolver(mut self, resolver: Arc<dyn ProviderResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Crucible — attach the shared per-session/day [`BudgetTracker`] so council
    /// member spend decrements the same envelope as the parent turn.
    pub fn with_budget_tracker(
        mut self,
        tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
    ) -> Self {
        self.budget_tracker = Some(tracker);
        self
    }

    /// The shared budget tracker, if one was attached.
    pub fn budget_tracker(&self) -> Option<&Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>> {
        self.budget_tracker.as_ref()
    }

    /// Crucible — the (session_id, user_id) the council charges against.
    pub fn with_budget_identity(
        mut self,
        session_id: impl Into<String>,
        user_id: impl Into<String>,
    ) -> Self {
        self.budget_identity = Some((session_id.into(), user_id.into()));
        self
    }

    /// The (session_id, user_id) for council charging, if set.
    pub fn budget_identity(&self) -> Option<&(String, String)> {
        self.budget_identity.as_ref()
    }

    /// Attach the finite provider-call envelope shared with the parent engine.
    pub fn with_provider_budget(
        mut self,
        tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
        session_id: impl Into<String>,
    ) -> Self {
        self.provider_budget_tracker = Some(tracker);
        self.budget_session_id = Some(session_id.into());
        self
    }

    /// Attach the parent execution envelope used to derive child budgets.
    pub fn with_execution_budget(mut self, budget: wcore_budget::ExecutionBudgetView) -> Self {
        self.execution_budget = Some(budget);
        self
    }

    /// Install one previously captured session governance bundle.
    pub fn with_budget_governance(mut self, governance: SpawnerBudgetGovernance) -> Self {
        self.provider_budget_tracker = Some(governance.provider_budget_tracker);
        self.budget_session_id = Some(governance.budget_session_id);
        self.execution_budget = Some(governance.execution_budget);
        self.cancel = governance.cancel;
        self.budget_guard = governance.budget_guard;
        self
    }

    /// Capture the complete finite envelope for a transient child spawner.
    /// Production Smart sessions always return `Some`; legacy/test spawners
    /// without an attached provider ledger return `None` rather than fabricating
    /// an independent allowance.
    pub fn budget_governance(&self) -> Option<SpawnerBudgetGovernance> {
        let governance = SpawnerBudgetGovernance::new(
            Arc::clone(self.provider_budget_tracker.as_ref()?),
            self.budget_session_id.as_ref()?.clone(),
            self.execution_budget.as_ref()?.clone(),
            self.active_cancel_token(),
        );
        Some(match self.budget_guard.as_ref() {
            Some(guard) => governance.with_budget_guard(Arc::clone(guard)),
            None => governance,
        })
    }

    fn enter_child_budget(
        &self,
    ) -> Result<
        (
            Option<wcore_budget::ExecutionBudgetView>,
            Option<wcore_budget::AgentDepthGuard>,
        ),
        String,
    > {
        let Some(parent) = self.execution_budget.as_ref() else {
            return Ok((None, None));
        };
        let child = parent.sub_budget(None);
        let guard = child.enter_agent();
        if let Some(reason) = child.first_exceeded_reason() {
            let observed = child.observed_for(reason);
            let limit = child.limit_for(reason);
            drop(guard);
            return Err(format!(
                "child agent not started: budget cap '{reason}' exceeded (limit {limit}, observed {observed})"
            ));
        }
        Ok((Some(child), Some(guard)))
    }

    fn bind_child_budget(
        &self,
        engine: &mut AgentEngine,
        execution_budget: Option<wcore_budget::ExecutionBudgetView>,
    ) {
        if let Some(budget) = execution_budget {
            engine.set_execution_budget(budget);
        }
        if let Some(tracker) = self.provider_budget_tracker.as_ref() {
            engine.set_budget_tracker(Arc::clone(tracker));
        }
        if let Some(session_id) = self.budget_session_id.as_ref() {
            engine.set_budget_session_id(session_id.clone());
        }
    }

    /// Resolve the provider a given sub-agent should run on.
    ///
    /// - **Unpinned** (`sub.provider == None`): inherit the parent provider —
    ///   the single-provider default, regardless of whether a resolver is
    ///   attached.
    /// - **Pinned with a resolver**: resolve the spec to a keyed provider. A
    ///   resolution failure (unknown / keyless) is fatal *for that sub-agent*
    ///   and surfaces as an error [`SubAgentResult`] (the council skips
    ///   keyless members when building the roster, before they reach here).
    /// - **Pinned without a resolver**: a configuration error — a provider was
    ///   pinned but nothing can resolve it. Fail that sub-agent loudly rather
    ///   than silently running it on the parent provider.
    fn provider_for(&self, sub: &SubAgentConfig) -> Result<Arc<dyn LlmProvider>, SubAgentResult> {
        match (&sub.provider, &self.resolver) {
            (None, _) => Ok(self.provider.clone()),
            (Some(spec), Some(resolver)) => resolver
                .resolve_provider(spec)
                .map(|(provider, _model)| provider)
                .map_err(|e| SubAgentResult::error(&sub.name, &format!("provider '{spec}': {e}"))),
            (Some(spec), None) => Err(SubAgentResult::error(
                &sub.name,
                &format!("provider '{spec}' pinned but no provider resolver is attached"),
            )),
        }
    }

    /// Spawn a single sub-agent and wait for result.
    pub async fn spawn_one(&self, sub_config: SubAgentConfig) -> SubAgentResult {
        // Security audit H-7 / M-9: `child_config` inherits the parent's
        // approval posture (no forced `auto_approve = true`), and
        // `build_tool_registry(&[])` defaults to a read-only toolset.
        let config = self.child_config(&sub_config);
        // Crucible — resolve the per-spawn pinned provider (or inherit parent).
        let provider = match self.provider_for(&sub_config) {
            Ok(p) => p,
            Err(result) => return result,
        };
        let (child_budget, _agent_guard) = match self.enter_child_budget() {
            Ok(budget) => budget,
            Err(error) => return SubAgentResult::error(&sub_config.name, &error),
        };

        let tools = self.child_tool_registry(&[]);
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let mut engine = AgentEngine::new_with_provider(provider, config, tools, output);
        self.bind_child_budget(&mut engine, child_budget);
        engine.set_egress_policy(self.egress_policy.clone());
        // Bind the child to the parent cancel token so a host cancel stops it.
        engine.set_cancel_token(self.active_cancel_token().child_token());

        // v0.8.0 Task J — publish Spawned + FirstMessage before
        // entering the engine, then Completed/Errored on the way out.
        // Spawner has no parent_call_id here (legacy direct callers do
        // not pass one in); set None.
        self.publish_spawned(&sub_config.name, None);
        self.publish_first_message(&sub_config.name, &sub_config.prompt);
        let mut guard = self.lifecycle_guard(&sub_config.name);

        let result = engine.run(&sub_config.prompt, "").await;
        let out = match result {
            Ok(result) => {
                self.publish_completed(&sub_config.name, result.turns, result.usage.output_tokens);
                guard.outcome = TerminalOutcome::Published;
                subagent_ok_result(sub_config.name, result)
            }
            Err(e) => {
                self.publish_errored(&sub_config.name, &e.to_string());
                guard.outcome = TerminalOutcome::Published;
                SubAgentResult {
                    name: sub_config.name,
                    text: format!("Sub-agent error: {}", e),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }
            }
        };
        drop(guard);
        out
    }

    /// Spawn multiple sub-agents in parallel.
    ///
    /// W7 F2: legacy shim — delegates to `spawn_parallel_with_extras` with
    /// `SpawnExtras::default()` so behaviour is bit-identical to today's
    /// "anonymous Spawn" call sites. New callers that want sub-agent event
    /// relay should call `spawn_parallel_with_extras` directly.
    pub async fn spawn_parallel(&self, sub_configs: Vec<SubAgentConfig>) -> Vec<SubAgentResult> {
        self.spawn_parallel_with_extras(sub_configs, SpawnExtras::default())
            .await
    }

    /// W7 F2: parallel spawn with channel-sink wiring.
    ///
    /// When `extras.channel_sink` is `Some`, the sub-agent's engine uses it
    /// as its `OutputSink` so every event the sub-agent emits is relayed via
    /// `SubAgentRelay` to the parent for `SubAgentEvent` wrapping. When
    /// `None`, behaviour is bit-identical to the pre-W7 `spawn_parallel`.
    pub async fn spawn_parallel_with_extras(
        &self,
        sub_configs: Vec<SubAgentConfig>,
        extras: SpawnExtras,
    ) -> Vec<SubAgentResult> {
        let mut futures = SpawnTaskSet(
            sub_configs
                .into_iter()
                .map(|config| {
                    let spawner = self.clone_for_spawn();
                    let extras = extras.clone();
                    tokio::spawn(async move { spawner.spawn_one_with_extras(config, extras).await })
                })
                .collect(),
        );

        let mut results = Vec::new();
        for future in &mut futures.0 {
            match future.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(SubAgentResult {
                    name: "unknown".to_string(),
                    text: format!("Task join error: {}", e),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }),
            }
        }
        results
    }

    /// #269 — route a parallel spawn through `FleetDispatcher` for
    /// hierarchical sharding. Each `SubAgentConfig` becomes one
    /// `MeshAgent`; the fleet shards them into batches of
    /// [`DEFAULT_SHARD_SIZE`] (10) and runs every shard concurrently as a
    /// `MeshDispatcher`. Each sub-agent's [`AgentBus`] `Spawned` event
    /// carries `parent_call_id = Some("fleet:<run_id>-shard-<i>-<j>")`
    /// so a subscriber can prove the Fleet path was taken (the wire-
    /// presence test in `fleet_dispatcher_wired_test.rs` checks this).
    ///
    /// `run_id` is a free-form label propagated into the fleet's
    /// blackboard topic prefix; callers in production pass the
    /// `SpawnTool` invocation id.
    pub async fn spawn_via_fleet(
        &self,
        sub_configs: Vec<SubAgentConfig>,
        run_id: impl Into<String>,
    ) -> Vec<SubAgentResult> {
        let tasks = sub_configs
            .into_iter()
            .map(|config| (config, SpawnExtras::default()))
            .collect();
        self.spawn_via_fleet_with_per_task_extras(tasks, run_id)
            .await
    }

    /// Fleet-sharded spawn with one output/terminal sink per task. Fleet keeps
    /// its shard-scoped bus correlation while the supplied `ChannelSink`
    /// independently carries the workflow node correlation to the host.
    pub async fn spawn_via_fleet_with_per_task_extras(
        &self,
        tasks_and_extras: Vec<(SubAgentConfig, SpawnExtras)>,
        run_id: impl Into<String>,
    ) -> Vec<SubAgentResult> {
        let run_id = run_id.into();
        let fleet = FleetDispatcher::new(run_id).with_shard_size(DEFAULT_SHARD_SIZE);

        // Build one MeshAgent per task. Each agent owns a clone of the
        // spawner (cheap — same Arc/Config plumbing the legacy
        // spawn_parallel path uses) and reports back the SubAgentResult
        // serialized into the AgentReport payload so the reducer can
        // reconstruct it on the orchestrator side.
        let agents: Vec<MeshAgent> = tasks_and_extras
            .into_iter()
            .map(|(sub_config, extras)| -> MeshAgent {
                let spawner = self.clone_for_spawn();
                Box::new(move |ctx: BlackboardCtx| {
                    Box::pin(async move {
                        // Wire-presence signal: tag the per-sub-agent
                        // Spawned event with the shard-scoped id so a
                        // bus subscriber can prove the Fleet path ran.
                        let mut extras = extras;
                        extras.parent_call_id = Some(format!("fleet:{}", ctx.agent_id));
                        let result = spawner.spawn_one_with_extras(sub_config, extras).await;
                        let succeeded = !result.is_error;
                        AgentReport {
                            agent_id: ctx.agent_id,
                            payload: sub_agent_result_to_payload(&result),
                            succeeded,
                        }
                    })
                })
            })
            .collect();

        // Reducer: flatten all shard summaries back into the original
        // Vec<SubAgentResult>. Order is shard_id-then-within-shard,
        // which matches input order modulo the shard boundary (the same
        // race-order property the legacy spawn_parallel path has).
        let reducer: FleetReducer<Vec<SubAgentResult>> =
            Box::new(|summaries: Vec<ShardSummary>| {
                summaries
                    .into_iter()
                    .flat_map(|s| {
                        // The shard's payload is the
                        // serde_json::Value::Array we built in
                        // `default_shard_reducer_into_results` below.
                        s.payload
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .map(payload_to_sub_agent_result)
                            .collect::<Vec<_>>()
                    })
                    .collect()
            });

        // Shard reducer factory: each shard collects its AgentReports'
        // payloads (already serialized SubAgentResults) into a JSON array
        // attached to the ShardSummary, so the FleetReducer above can
        // walk them in stable order.
        let shard_factory: Box<dyn Fn() -> wcore_swarm::ShardReducer + Send + Sync> =
            Box::new(|| Box::new(default_shard_reducer_into_results));

        match fleet.dispatch(agents, Some(shard_factory), reducer).await {
            Ok(results) => results,
            Err(err) => {
                // FleetDispatcher only errors on cap-exceeded or shard
                // join failure. Surface as a single error-result so the
                // SpawnTool caller's `is_error` aggregation still works.
                vec![SubAgentResult {
                    name: "fleet".to_string(),
                    text: format!("Fleet dispatch failed: {err}"),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }]
            }
        }
    }

    /// v0.9.4 W1: per-task parallel spawn with individual extras per task.
    ///
    /// Unlike `spawn_parallel_with_extras` (one `SpawnExtras` shared across
    /// all tasks), this variant gives each task its own `SpawnExtras` so each
    /// sub-agent gets a distinct `ChannelSink` and `parent_call_id`. Required
    /// for N distinct `SubAgentView` rows in the bridge (C1/F8 relay fix).
    pub async fn spawn_parallel_with_per_task_extras(
        &self,
        tasks_and_extras: Vec<(SubAgentConfig, SpawnExtras)>,
    ) -> Vec<SubAgentResult> {
        let mut join_terminals = Vec::with_capacity(tasks_and_extras.len());
        let mut handles = Vec::with_capacity(tasks_and_extras.len());
        for (config, extras) in tasks_and_extras {
            let spawner = self.clone_for_spawn();
            join_terminals.push((config.name.clone(), extras.channel_sink.clone()));
            handles.push(tokio::spawn(async move {
                spawner.spawn_one_with_extras(config, extras).await
            }));
        }
        let mut futures = SpawnTaskSet(handles);

        let mut results = Vec::new();
        for (future, (name, terminal_sink)) in futures.0.iter_mut().zip(join_terminals) {
            match future.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    let result = SubAgentResult {
                        name,
                        text: format!("Task join error: {e}"),
                        usage: TokenUsage::default(),
                        turns: 0,
                        is_error: true,
                    };
                    relay_subagent_terminal(terminal_sink.as_deref(), &result);
                    results.push(result);
                }
            }
        }
        results
    }

    /// W7 F2: per-task helper — mirrors `spawn_one`, but installs an
    /// `Arc<ChannelSink>` as `OutputSink` when `extras.channel_sink` is
    /// `Some`. Anonymous (None) call path is byte-identical to `spawn_one`.
    async fn spawn_one_with_extras(
        &self,
        sub_config: SubAgentConfig,
        extras: SpawnExtras,
    ) -> SubAgentResult {
        // Security audit H-7 / M-9: inherit the parent's approval posture via
        // `child_config` (no forced `auto_approve`). Forcing it here would let
        // a single `Delegate`/`Spawn` approval auto-run every child
        // Bash/Write/Edit call with no operator prompt.
        let config = self.child_config(&sub_config);
        let terminal_sink = extras.channel_sink.clone();
        // Crucible — resolve the per-spawn pinned provider (or inherit parent).
        // This is the path the fleet + parallel proposers funnel through, so a
        // resolver that fails to propagate via `clone_for_spawn` surfaces here
        // as a silent fall-back to the parent provider (guarded by tests).
        let provider = match self.provider_for(&sub_config) {
            Ok(p) => p,
            Err(result) => {
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                return result;
            }
        };
        let (child_budget, _agent_guard) = match self.enter_child_budget() {
            Ok(budget) => budget,
            Err(error) => {
                let result = SubAgentResult::error(&sub_config.name, &error);
                relay_subagent_terminal(terminal_sink.as_deref(), &result);
                return result;
            }
        };

        let tools = self.child_tool_registry(&[]);
        let output: Arc<dyn OutputSink> = match extras.channel_sink {
            Some(sink) => sink as Arc<dyn OutputSink>, // sub-agent events flow back through parent
            None => Arc::new(NullSink),                // legacy anonymous behaviour
        };
        let mut engine = AgentEngine::new_with_provider(provider, config, tools, output);
        self.bind_child_budget(&mut engine, child_budget);
        engine.set_egress_policy(self.egress_policy.clone());
        // Bind the child to the parent cancel token so a host cancel stops it.
        engine.set_cancel_token(self.active_cancel_token().child_token());

        // v0.8.0 Task J — Spawned + FirstMessage before the turn,
        // Completed/Errored after. `extras.parent_call_id` (set by
        // SpawnTool's relay path) is carried into the Spawned event so
        // a subscriber can correlate sub-agent lifecycle with the
        // parent's `SpawnTool` invocation.
        self.publish_spawned(&sub_config.name, extras.parent_call_id.clone());
        self.publish_first_message(&sub_config.name, &sub_config.prompt);
        let mut guard = self.lifecycle_guard(&sub_config.name);

        let result = engine.run(&sub_config.prompt, "").await;
        let out = match result {
            Ok(result) => {
                self.publish_completed(&sub_config.name, result.turns, result.usage.output_tokens);
                guard.outcome = TerminalOutcome::Published;
                subagent_ok_result(sub_config.name.clone(), result)
            }
            Err(e) => {
                self.publish_errored(&sub_config.name, &e.to_string());
                guard.outcome = TerminalOutcome::Published;
                SubAgentResult {
                    name: sub_config.name,
                    text: format!("Sub-agent error: {}", e),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }
            }
        };
        drop(guard);
        relay_subagent_terminal(terminal_sink.as_deref(), &out);
        out
    }

    /// Derive a sub-agent's [`Config`] from the parent's `base_config`.
    ///
    /// Security audit H-7 / M-9: this is the single place that builds a child
    /// config. It clones the parent's config (which carries the parent's
    /// `tools.auto_approve` and `tools.allow_list`) and applies only the
    /// per-spawn overrides — it deliberately does NOT flip `auto_approve` to
    /// `true`. The child therefore inherits the parent's approval posture, so a
    /// parent that prompts the operator for Bash/Write/Edit keeps doing so
    /// inside any sub-agent it delegates to.
    fn child_config(&self, sub_config: &SubAgentConfig) -> Config {
        let mut config = self.base_config.clone();
        if let Some(manager) = &self.approval_manager {
            config.set_smart_approval_policy(manager.current_approval_policy());
        }
        config.max_turns = Some(sub_config.max_turns);
        config.max_tokens = sub_config.max_tokens;
        // #112 — a per-spawn cap is ALWAYS deliberate: it must bind on the
        // wire and never be omitted. Without this, (a) a desktop-default
        // session on an omit-safe provider (flux/openrouter/gemini) would
        // omit the child's sized cap and let Spawn/council children emit the
        // served model's full ceiling, busting the sub-agent/CouncilSpend
        // worst-case math; and (b) a child pinned to a different provider
        // would decide omission from the PARENT's omitted-cap signal.
        config.max_tokens_explicit = true;
        // Crucible #3 — honor a per-spawn temperature override. `None` leaves the
        // base config's temperature in place (top-level base is `None`, so the
        // child engine omits the field unless this sets it).
        if let Some(temperature) = sub_config.temperature {
            config.temperature = Some(temperature);
        }
        if let Some(sp) = sub_config.system_prompt.clone() {
            config.system_prompt = Some(sp);
        }
        // Crucible T2 — honor a per-spawn model override. The provider pin
        // (T4) selects the upstream; this sets the model the child requests.
        if let Some(model) = &sub_config.model {
            config.model = model.clone();
        }
        config.session.enabled = false;
        // FIX F — the shadow workflow-detection heuristic is a TOP-LEVEL,
        // user-initiated-turn signal. Sub-agents spawned by a workflow (or any
        // delegation) run their own turns, which are intra-workflow, not user
        // turns; leaving the gate on would pollute the shadow log with recursive
        // detections. Force it off for every child engine — the top-level shadow
        // path (driven by the parent engine, built from the un-mutated config) is
        // unaffected.
        config.observability.workflow_detection_enabled = false;
        // B6 defense-in-depth — the LIVE workflow confirm gate is a top-level,
        // user-initiated pre-LLM intercept. Child engines already lack an
        // approval manager + protocol writer (so the gate's guard short-circuits
        // for them), but force the mode off here too so a workflow's sub-agents
        // can NEVER recursively re-enter the gate regardless of how they are
        // wired.
        config.observability.workflow_live_mode = false;
        config
    }

    fn clone_for_spawn(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            base_config: self.base_config.clone(),
            sandbox_runtime: Arc::clone(&self.sandbox_runtime),
            egress_policy: self.egress_policy.clone(),
            approval_manager: self.approval_manager.clone(),
            bus: self.bus.clone(),
            cancel: self.cancel.clone(),
            session_runtime: self.session_runtime.clone(),
            // CRITICAL (crucible): the resolver MUST be carried into every
            // cloned spawner. The fleet + parallel paths run each proposer on a
            // `clone_for_spawn()` copy; dropping the resolver here would make
            // pinned proposers silently fall back to the parent provider,
            // collapsing the cross-provider council into a single-provider one.
            resolver: self.resolver.clone(),
            // CRITICAL (crucible): the shared budget tracker MUST be carried into
            // every cloned spawner. If it isn't propagated, council members run
            // on the fleet/parallel `clone_for_spawn()` copies and silently lose
            // the per-session/day envelope.
            budget_tracker: self.budget_tracker.clone(),
            budget_identity: self.budget_identity.clone(),
            provider_budget_tracker: self.provider_budget_tracker.clone(),
            budget_session_id: self.budget_session_id.clone(),
            execution_budget: self.execution_budget.clone(),
            budget_guard: self.budget_guard.clone(),
        }
    }

    // ---- v0.8.0 Task J: lifecycle publish helpers ----

    fn publish_spawned(&self, agent: &str, parent_call_id: Option<String>) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::Spawned {
                agent: agent.to_string(),
                parent_call_id,
                timestamp_ms: now_ms(),
            });
        }
    }

    fn publish_first_message(&self, agent: &str, content: &str) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::FirstMessage {
                agent: agent.to_string(),
                content_preview: preview(content, FIRST_MESSAGE_PREVIEW_CHARS),
            });
        }
    }

    fn publish_completed(&self, agent: &str, turns: usize, output_tokens: u64) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::Completed {
                agent: agent.to_string(),
                turns,
                output_tokens,
            });
        }
    }

    fn publish_errored(&self, agent: &str, error: &str) {
        if let Some(bus) = &self.bus {
            bus.publish(AgentMessage::Errored {
                agent: agent.to_string(),
                error: error.to_string(),
            });
        }
    }

    fn lifecycle_guard(&self, agent: &str) -> LifecycleGuard {
        LifecycleGuard {
            bus: self.bus.clone(),
            agent: agent.to_string(),
            outcome: TerminalOutcome::Pending,
        }
    }
}

#[async_trait]
impl Spawner for AgentSpawner {
    async fn spawn_fork(
        &self,
        sub_config: SubAgentConfig,
        overrides: ForkOverrides,
    ) -> SubAgentResult {
        // Security audit H-7 / M-9: inherit the parent's approval posture via
        // `child_config` (no forced `auto_approve`). Combined with the
        // read-only default in `build_tool_registry`, an empty
        // `overrides.allowed_tools` now yields a child with no Bash/Write/Edit
        // and the parent's confirm posture.
        let mut config = self.child_config(&sub_config);
        if let Some(model) = overrides.model.clone() {
            config.model = model;
        }
        // Crucible — resolve the per-fork pinned provider (or inherit parent).
        let provider = match self.provider_for(&sub_config) {
            Ok(p) => p,
            Err(result) => return result,
        };
        let (child_budget, _agent_guard) = match self.enter_child_budget() {
            Ok(budget) => budget,
            Err(error) => return SubAgentResult::error(&sub_config.name, &error),
        };

        let tools = self.child_tool_registry(&overrides.allowed_tools);
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let mut engine = AgentEngine::new_with_provider(provider, config, tools, output);
        self.bind_child_budget(&mut engine, child_budget);
        engine.set_egress_policy(self.egress_policy.clone());
        // Bind the child to the parent cancel token so a host cancel stops it.
        engine.set_cancel_token(self.active_cancel_token().child_token());
        engine.set_initial_reasoning_effort(overrides.effort.clone());

        // v0.8.0 Task J — fork path publishes lifecycle too. Forks
        // don't carry a parent SpawnTool call_id (the `Spawner` trait
        // surface doesn't accept one), so we pass None.
        self.publish_spawned(&sub_config.name, None);
        self.publish_first_message(&sub_config.name, &sub_config.prompt);
        let mut guard = self.lifecycle_guard(&sub_config.name);

        let result = engine.run(&sub_config.prompt, "").await;
        let out = match result {
            Ok(result) => {
                self.publish_completed(&sub_config.name, result.turns, result.usage.output_tokens);
                guard.outcome = TerminalOutcome::Published;
                subagent_ok_result(sub_config.name, result)
            }
            Err(e) => {
                self.publish_errored(&sub_config.name, &e.to_string());
                guard.outcome = TerminalOutcome::Published;
                SubAgentResult {
                    name: sub_config.name,
                    text: format!("Sub-agent error: {}", e),
                    usage: TokenUsage::default(),
                    turns: 0,
                    is_error: true,
                }
            }
        };
        drop(guard);
        out
    }
}

/// #269 — fleet sharding helper: serialize a `SubAgentResult` into the
/// `AgentReport.payload` `serde_json::Value` so the fleet reducer can
/// reconstruct it from the shard summary's payload array. Lossless for
/// the wire-format fields we care about (name/text/usage/turns/is_error).
fn sub_agent_result_to_payload(r: &SubAgentResult) -> serde_json::Value {
    serde_json::json!({
        "name": r.name,
        "text": r.text,
        "input_tokens": r.usage.input_tokens,
        "output_tokens": r.usage.output_tokens,
        "cache_creation_tokens": r.usage.cache_creation_tokens,
        "cache_read_tokens": r.usage.cache_read_tokens,
        "turns": r.turns,
        "is_error": r.is_error,
    })
}

/// #269 — fleet sharding helper: inverse of
/// [`sub_agent_result_to_payload`]. Defensive defaults so a malformed
/// payload (theoretically impossible — we always produce it ourselves)
/// surfaces as an error result rather than panicking.
fn payload_to_sub_agent_result(v: serde_json::Value) -> SubAgentResult {
    let name = v
        .get("name")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let text = v
        .get("text")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let usage = TokenUsage {
        input_tokens: v.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        output_tokens: v.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0),
        cache_creation_tokens: v
            .get("cache_creation_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        cache_read_tokens: v
            .get("cache_read_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
    };
    let turns = v.get("turns").and_then(|n| n.as_u64()).unwrap_or(0) as usize;
    let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(true);
    SubAgentResult {
        name,
        text,
        usage,
        turns,
        is_error,
    }
}

/// #269 — fleet sharding helper: shard reducer that stuffs each
/// `AgentReport.payload` (already a serialized `SubAgentResult`) into a
/// JSON array attached to the `ShardSummary`. The fleet reducer then
/// walks shards in stable order and rehydrates the per-task results.
fn default_shard_reducer_into_results(shard_id: usize, reports: Vec<AgentReport>) -> ShardSummary {
    let successes = reports.iter().filter(|r| r.succeeded).count();
    let failures = reports.iter().filter(|r| !r.succeeded).count();
    let payload =
        serde_json::Value::Array(reports.into_iter().map(|r| r.payload).collect::<Vec<_>>());
    ShardSummary {
        shard_id,
        agent_count: successes + failures,
        successes,
        failures,
        payload,
    }
}

type ToolFactory = fn() -> Box<dyn wcore_tools::Tool>;

/// Sub-agent tools that can read but not mutate host state. When a spawn
/// requests no explicit `allowed_tools`, the child is restricted to this
/// read-only subset (security audit H-7 / M-9): an empty `toolsets` on the
/// model-facing `Delegate`/`Spawn` tool must NOT silently grant the child
/// Bash/Write/Edit. Destructive tools require explicit opt-in via `allowed`.
const READ_ONLY_TOOLS: &[&str] = &["Read", "Grep", "Glob"];

fn build_tool_registry(
    allowed: &[String],
    sandbox_runtime: Arc<wcore_sandbox::SandboxRegistry>,
) -> ToolRegistry {
    let all: &[(&str, ToolFactory)] = &[
        ("Read", || Box::new(ReadTool::new(None))),
        ("Write", || Box::new(WriteTool::new(None))),
        ("Edit", || Box::new(EditTool::new(None))),
        ("Bash", || Box::new(BashTool)),
        ("Grep", || Box::new(GrepTool)),
        ("Glob", || Box::new(GlobTool)),
    ];

    let mut registry = ToolRegistry::new();
    registry.set_sandbox_runtime(sandbox_runtime);
    for (name, make_tool) in all {
        // Security audit H-7 / M-9: an empty `allowed` list no longer means
        // "register everything". It defaults to a read-only subset so a
        // `Delegate` call that omits `toolsets` can never hand a sub-agent
        // Bash/Write/Edit. Callers that genuinely need destructive tools must
        // name them explicitly in `allowed`.
        let permitted = if allowed.is_empty() {
            READ_ONLY_TOOLS.contains(name)
        } else {
            allowed.iter().any(|a| a.as_str() == *name)
        };
        if permitted {
            registry.register(make_tool());
        }
    }
    registry
}

#[cfg(test)]
mod spawn_task_set_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::oneshot;
    use wcore_config::config::Config;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::llm::{LlmEvent, LlmRequest};

    use super::{AgentSpawner, SpawnTaskSet, SubAgentConfig, SubAgentResult};

    struct DropNotify(Option<oneshot::Sender<()>>);

    impl Drop for DropNotify {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test]
    async fn dropping_spawn_task_set_aborts_and_drops_children() {
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let child = tokio::spawn(async move {
            let _drop_notify = DropNotify(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
            SubAgentResult::error("unreachable", "unreachable")
        });
        let tasks = SpawnTaskSet(vec![child]);
        started_rx.await.expect("child must start");

        drop(tasks);

        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("aborted child must be dropped promptly")
            .expect("drop notifier must fire");
    }

    struct HangingProvider {
        started: Mutex<Option<oneshot::Sender<()>>>,
        dropped: Mutex<Option<oneshot::Sender<()>>>,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for HangingProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = self.started.lock().expect("started mutex").take() {
                let _ = tx.send(());
            }
            let _drop_notify = DropNotify(self.dropped.lock().expect("dropped mutex").take());
            std::future::pending().await
        }
    }

    struct CountingErrorProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmProvider for CountingErrorProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Connection("test provider called".into()))
        }
    }

    fn bounded_child(name: &str) -> SubAgentConfig {
        SubAgentConfig {
            name: name.into(),
            prompt: "perform bounded work".into(),
            max_turns: 1,
            max_tokens: 16,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        }
    }

    #[tokio::test]
    async fn parallel_children_cannot_bypass_parent_agent_cap() {
        let provider = Arc::new(CountingErrorProvider {
            calls: AtomicUsize::new(0),
        });
        let budget = wcore_budget::ExecutionBudget {
            max_agent_depth: Some(0),
            ..Default::default()
        }
        .start_root();
        let spawner =
            AgentSpawner::new(provider.clone(), Config::default()).with_execution_budget(budget);

        let results = spawner
            .spawn_parallel(vec![bounded_child("one"), bounded_child("two")])
            .await;

        assert!(results.iter().all(|result| result.is_error));
        assert!(
            results
                .iter()
                .all(|result| result.text.contains("max_agent_depth"))
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn child_provider_call_uses_parent_session_reservation() {
        let provider = Arc::new(CountingErrorProvider {
            calls: AtomicUsize::new(0),
        });
        let tracker = Arc::new(parking_lot::Mutex::new(wcore_budget::BudgetTracker::new(
            wcore_budget::BudgetCap::builder()
                .per_session_tokens(1)
                .build(),
        )));
        let spawner = AgentSpawner::new(provider.clone(), Config::default())
            .with_provider_budget(tracker, "shared-session");

        let result = spawner.spawn_one(bounded_child("bounded")).await;

        assert!(result.is_error, "budget refusal must fail the child loudly");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn transient_governance_reuses_parent_authority_handles() {
        let provider: Arc<dyn LlmProvider> = Arc::new(CountingErrorProvider {
            calls: AtomicUsize::new(0),
        });
        let provider_budget = Arc::new(parking_lot::Mutex::new(wcore_budget::BudgetTracker::new(
            wcore_budget::BudgetCap::builder()
                .per_session_tokens(32)
                .build(),
        )));
        let execution_budget = wcore_budget::ExecutionBudget {
            max_tokens_in: Some(1),
            ..Default::default()
        }
        .start_root();
        let cancel = tokio_util::sync::CancellationToken::new();
        let parent = AgentSpawner::new(Arc::clone(&provider), Config::default())
            .with_provider_budget(Arc::clone(&provider_budget), "shared-session")
            .with_execution_budget(execution_budget.clone())
            .with_cancel(cancel.clone());
        let governance = parent
            .budget_governance()
            .expect("a fully governed parent must export its existing handles");

        let transient =
            AgentSpawner::new(provider, Config::default()).with_budget_governance(governance);

        assert!(Arc::ptr_eq(
            transient
                .provider_budget_tracker
                .as_ref()
                .expect("provider budget must transfer"),
            &provider_budget,
        ));
        assert_eq!(
            transient.budget_session_id.as_deref(),
            Some("shared-session")
        );
        transient
            .execution_budget
            .as_ref()
            .expect("execution budget must transfer")
            .record_tokens(2, 0);
        assert_eq!(
            execution_budget.first_exceeded_reason(),
            Some("max_tokens_in"),
            "the transferred view must share the parent's execution ledger"
        );
        cancel.cancel();
        assert!(
            transient.active_cancel_token().is_cancelled(),
            "the transient spawner must inherit parent cancellation"
        );
    }

    #[tokio::test]
    async fn cancelling_legacy_parallel_spawn_aborts_running_children() {
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let provider = Arc::new(HangingProvider {
            started: Mutex::new(Some(started_tx)),
            dropped: Mutex::new(Some(dropped_tx)),
            calls: AtomicUsize::new(0),
        });
        let spawner = AgentSpawner::new(provider.clone(), Config::default());
        let child = SubAgentConfig {
            name: "hanging-child".into(),
            prompt: "wait".into(),
            max_turns: 2,
            max_tokens: 16,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        };

        // This is the model-facing legacy no-relay path. Its own cancellation
        // must abort the raw child task even though the session token remains live.
        let parent = tokio::spawn(async move { spawner.spawn_parallel(vec![child]).await });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .expect("legacy child must reach the provider")
            .expect("started notifier must fire");

        parent.abort();
        let _ = parent.await;

        tokio::time::timeout(Duration::from_secs(1), dropped_rx)
            .await
            .expect("legacy child must be dropped promptly")
            .expect("drop notifier must fire");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            1,
            "an aborted child must not resume provider activity"
        );
    }
}

#[cfg(test)]
mod crucible_provider_resolution_tests {
    //! Crucible T2/T4 — per-spawn provider resolution + model override.
    //!
    //! These guard the cross-provider council at the spawn layer: a pinned
    //! `SubAgentConfig.provider` must resolve to *that* provider (not the
    //! parent), an unpinned spawn must inherit the parent, and a cloned
    //! spawner (the relay/fleet path) must still carry the resolver.

    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use wcore_config::config::Config;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::llm::{LlmEvent, LlmRequest};

    use super::{AgentSpawner, SubAgentConfig};
    use crate::orchestration::council::{ProviderResolver, ResolveError};

    /// A provider that never streams — identity is all these tests check.
    struct StubProvider;

    #[async_trait]
    impl LlmProvider for StubProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Connection("stub".into()))
        }
    }

    /// Test resolver mapping a spec string to a specific provider `Arc`.
    struct MapResolver {
        map: HashMap<String, Arc<dyn LlmProvider>>,
    }

    impl ProviderResolver for MapResolver {
        fn resolve_provider(
            &self,
            spec: &str,
        ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
            self.map
                .get(spec)
                .cloned()
                .map(|p| (p, None))
                .ok_or_else(|| ResolveError::Unknown(spec.to_string()))
        }
    }

    fn sub(name: &str, provider: Option<&str>) -> SubAgentConfig {
        SubAgentConfig {
            name: name.into(),
            prompt: "x".into(),
            max_turns: 1,
            max_tokens: 16,
            system_prompt: None,
            provider: provider.map(|s| s.into()),
            model: None,
            temperature: None,
        }
    }

    fn resolver_mapping(specs: &[(&str, Arc<dyn LlmProvider>)]) -> Arc<dyn ProviderResolver> {
        let map = specs
            .iter()
            .map(|(s, p)| (s.to_string(), p.clone()))
            .collect();
        Arc::new(MapResolver { map })
    }

    #[test]
    fn provider_for_unpinned_returns_parent() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent.clone(), Config::default());
        let got = spawner.provider_for(&sub("p", None)).expect("unpinned ok");
        assert!(Arc::ptr_eq(&got, &parent));
    }

    #[test]
    fn provider_for_pinned_returns_resolved_not_parent() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let pinned: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent.clone(), Config::default())
            .with_provider_resolver(resolver_mapping(&[("openai", pinned.clone())]));
        let got = spawner
            .provider_for(&sub("p", Some("openai")))
            .expect("pinned ok");
        assert!(Arc::ptr_eq(&got, &pinned), "pinned provider must be used");
        assert!(!Arc::ptr_eq(&got, &parent), "parent must NOT be used");
    }

    #[test]
    fn provider_for_pinned_without_resolver_errors() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default());
        // `Arc<dyn LlmProvider>` is not Debug, so match instead of expect_err.
        let err = match spawner.provider_for(&sub("p", Some("openai"))) {
            Err(e) => e,
            Ok(_) => panic!("pinned-without-resolver must error"),
        };
        assert!(err.is_error);
        assert!(err.text.contains("no provider resolver"));
    }

    #[test]
    fn provider_for_unknown_pinned_errors() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default())
            .with_provider_resolver(resolver_mapping(&[]));
        let err = match spawner.provider_for(&sub("p", Some("nope"))) {
            Err(e) => e,
            Ok(_) => panic!("unknown pinned provider must error"),
        };
        assert!(err.is_error);
    }

    #[test]
    fn clone_for_spawn_preserves_resolver() {
        // The footgun guard: a cloned spawner (relay/fleet path) must still
        // resolve pinned providers — else proposers silently use the parent.
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let pinned: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default())
            .with_provider_resolver(resolver_mapping(&[("openai", pinned.clone())]));
        let cloned = spawner.clone_for_spawn();
        let got = cloned
            .provider_for(&sub("p", Some("openai")))
            .expect("cloned spawner resolves");
        assert!(
            Arc::ptr_eq(&got, &pinned),
            "cloned spawner must still resolve the pinned provider"
        );
    }

    #[test]
    fn clone_for_spawn_preserves_exact_parent_egress_policy() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let policy: wcore_egress::SharedPolicy = Arc::new(wcore_egress::AllowAllPolicy);
        let spawner =
            AgentSpawner::new(parent, Config::default()).with_egress_policy(policy.clone());
        let cloned = spawner.clone_for_spawn();

        assert!(Arc::ptr_eq(&spawner.egress_policy, &policy));
        assert!(Arc::ptr_eq(&cloned.egress_policy, &policy));
    }

    #[test]
    fn child_config_applies_model_override() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let spawner = AgentSpawner::new(parent, Config::default());
        let mut c = sub("p", None);
        c.model = Some("claude-opus-4-8".into());
        let cfg = spawner.child_config(&c);
        assert_eq!(cfg.model, "claude-opus-4-8");
    }

    /// #112 — a per-spawn cap is always deliberate: the child config must mark
    /// it EXPLICIT so the child engine never omits the wire max-tokens field,
    /// even when the parent session omitted `--max-tokens` on an omit-safe
    /// provider (flux/openrouter/gemini). Otherwise Spawn/council children on
    /// a desktop-default flux session would drop their sized cap on the wire
    /// and could emit the served model's full ceiling, busting the sub-agent /
    /// CouncilSpend worst-case math.
    #[test]
    fn child_config_marks_per_spawn_cap_explicit() {
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        // Parent: omit-safe provider compat AND an omitted (defaulted) cap —
        // the exact configuration where the parent itself WOULD omit.
        let base = Config {
            compat: wcore_config::compat::ProviderCompat::flux_router_defaults(),
            max_tokens_explicit: false,
            ..Config::default()
        };
        assert!(base.compat.omit_max_tokens_when_unsized());
        let spawner = AgentSpawner::new(parent, base);
        let cfg = spawner.child_config(&sub("p", None));
        assert!(
            cfg.max_tokens_explicit,
            "a spawned child's per-spawn cap must read as explicit (never omitted on the wire)"
        );
    }

    #[test]
    fn budget_tracker_attaches_and_survives_clone_for_spawn() {
        let tracker = std::sync::Arc::new(parking_lot::Mutex::new(
            wcore_budget::BudgetTracker::new(wcore_budget::BudgetCap::default()),
        ));
        let parent: Arc<dyn LlmProvider> = Arc::new(StubProvider);
        let s = AgentSpawner::new(parent, Config::default()).with_budget_tracker(tracker.clone());
        assert!(s.budget_tracker().is_some());
        assert!(s.clone_for_spawn().budget_tracker().is_some());
    }
}

#[cfg(test)]
mod phase7_tests {
    use std::sync::Arc;

    use super::{AgentSpawner, ForkOverrides, SubAgentConfig, build_tool_registry};
    use wcore_config::config::Config;
    use wcore_providers::LlmProvider;

    fn test_sandbox_runtime() -> Arc<wcore_sandbox::SandboxRegistry> {
        wcore_tools::registry::ToolRegistry::new().sandbox_runtime()
    }

    #[test]
    fn tc_7_1_fork_overrides_default_values() {
        let o = ForkOverrides::default();
        assert!(o.model.is_none());
        assert!(o.effort.is_none());
        assert!(o.allowed_tools.is_empty());
    }

    // Security audit H-7 / M-9: an empty `allowed` list must default to the
    // READ-ONLY subset (Read/Grep/Glob) — never the full toolset. A `Delegate`
    // call that omits `toolsets` must not silently grant the child
    // Bash/Write/Edit.
    #[test]
    fn tc_7_40_build_tool_registry_empty_allowed_is_read_only() {
        let registry = build_tool_registry(&[], test_sandbox_runtime());
        // Read-only tools ARE registered.
        for name in &["Read", "Grep", "Glob"] {
            assert!(
                registry.get(name).is_some(),
                "read-only tool '{name}' should be registered by default"
            );
        }
        // Destructive tools are NOT registered without explicit opt-in.
        for name in &["Write", "Edit", "Bash"] {
            assert!(
                registry.get(name).is_none(),
                "destructive tool '{name}' must NOT be registered on an empty toolset (H-7)"
            );
        }
    }

    // Security audit H-7: destructive tools are reachable ONLY when explicitly
    // named in `allowed` (the opt-in path).
    #[test]
    fn tc_7_42_build_tool_registry_destructive_requires_opt_in() {
        let registry = build_tool_registry(
            &["Bash".to_string(), "Write".to_string()],
            test_sandbox_runtime(),
        );
        assert!(
            registry.get("Bash").is_some(),
            "explicit Bash opt-in honored"
        );
        assert!(
            registry.get("Write").is_some(),
            "explicit Write opt-in honored"
        );
        // A read-only tool not in the explicit list is excluded (explicit list
        // is authoritative — it is NOT additive over the read-only default).
        assert!(
            registry.get("Read").is_none(),
            "Read excluded when an explicit allow-list omits it"
        );
    }

    #[test]
    fn tc_7_43_build_tool_registry_filters_to_allowed() {
        let allowed = vec!["Bash".to_string(), "Read".to_string()];
        let registry = build_tool_registry(&allowed, test_sandbox_runtime());
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
    }

    #[test]
    fn child_registry_inherits_exact_parent_sandbox_runtime() {
        let runtime = test_sandbox_runtime();
        let provider: Arc<dyn LlmProvider> =
            Arc::new(crate::test_utils::ScriptedProvider::new(Vec::new()));
        let spawner = AgentSpawner::new(provider, Config::default())
            .with_sandbox_runtime(Arc::clone(&runtime));
        let registry = spawner.child_tool_registry(&[]);

        assert!(Arc::ptr_eq(&runtime, &registry.sandbox_runtime()));
    }

    #[test]
    fn cloned_spawner_preserves_exact_parent_sandbox_runtime() {
        let runtime = test_sandbox_runtime();
        let provider: Arc<dyn LlmProvider> =
            Arc::new(crate::test_utils::ScriptedProvider::new(Vec::new()));
        let spawner = AgentSpawner::new(provider, Config::default())
            .with_sandbox_runtime(Arc::clone(&runtime));
        let cloned = spawner.clone_for_spawn();

        assert!(Arc::ptr_eq(&runtime, spawner.sandbox_runtime()));
        assert!(Arc::ptr_eq(&runtime, cloned.sandbox_runtime()));
    }

    #[test]
    fn tc_7_sub_agent_config_original_fields_intact() {
        let config = SubAgentConfig {
            name: "test-agent".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: Some("you are helpful".to_string()),
            provider: None,
            model: None,
            temperature: None,
        };
        assert_eq!(config.name, "test-agent");
        assert_eq!(config.max_turns, 5);
    }
}

#[cfg(test)]
mod posture_inheritance_tests {
    //! Security audit H-7 / M-9 — a spawned sub-agent must inherit the parent's
    //! approval posture. The bug was `config.tools.auto_approve = true` forced
    //! on every spawn, so a parent that prompts for Bash/Write/Edit was
    //! silently bypassed by a `Delegate`/`Spawn` call. These tests assert the
    //! child config built by `AgentSpawner::child_config` carries the parent's
    //! typed approval policy, legacy `auto_approve`, and `allow_list` unchanged.

    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use wcore_config::config::{Config, ToolsConfig};
    use wcore_protocol::ToolApprovalManager;
    use wcore_protocol::commands::SessionMode;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::execution_policy::ApprovalPolicy;
    use wcore_types::llm::{LlmEvent, LlmRequest};

    use super::{AgentSpawner, SubAgentConfig};

    /// Minimal `LlmProvider` stub — `child_config` never calls `stream`, so an
    /// immediate error return is sufficient to satisfy the trait bound.
    struct NeverProvider;

    #[async_trait]
    impl LlmProvider for NeverProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            Err(ProviderError::Connection("never called".into()))
        }
    }

    fn config_with_posture(auto_approve: bool, allow_list: Vec<String>) -> Config {
        Config {
            tools: ToolsConfig {
                auto_approve,
                allow_list,
                skills: wcore_config::config::SkillsPermissionConfig::default(),
                verify_edits: false,
                windows_shell: None,
                env_passthrough: Vec::new(),
                sandbox: None,
                allow_no_sandbox: None,
            },
            ..Default::default()
        }
    }

    fn sub_config() -> SubAgentConfig {
        SubAgentConfig {
            name: "child".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 3,
            max_tokens: 512,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        }
    }

    #[test]
    fn parent_auto_approve_false_yields_child_auto_approve_false() {
        let parent = config_with_posture(false, vec!["Read".to_string()]);
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let child = spawner.child_config(&sub_config());

        assert!(
            !child.tools.auto_approve,
            "child must inherit parent's auto_approve=false (H-7 / M-9)"
        );
        assert_eq!(
            child.tools.allow_list,
            vec!["Read".to_string()],
            "child must inherit parent's allow_list unchanged"
        );
    }

    #[test]
    fn parent_auto_approve_true_is_still_honored() {
        // The fix must not invert behavior for a parent that genuinely opted
        // into auto-approve — the child still auto-approves in that case.
        let parent = config_with_posture(true, vec![]);
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let child = spawner.child_config(&sub_config());

        assert!(
            child.tools.auto_approve,
            "child must inherit parent's auto_approve=true"
        );
    }

    #[test]
    fn typed_posture_tracks_live_manager_through_child_clones() {
        let mut parent = config_with_posture(false, vec![]);
        parent.set_smart_approval_policy(ApprovalPolicy::AutoEdit);
        let manager = Arc::new(ToolApprovalManager::new());
        manager.set_mode(SessionMode::AutoEdit);
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent)
            .with_approval_manager(Arc::clone(&manager));

        let direct_child = spawner.child_config(&sub_config());

        assert_eq!(
            direct_child.smart_approval_policy(),
            ApprovalPolicy::AutoEdit,
            "direct children must inherit the typed AutoEdit posture"
        );
        manager.set_mode(SessionMode::Default);
        let cloned_child = spawner.clone_for_spawn().child_config(&sub_config());
        assert_eq!(cloned_child.smart_approval_policy(), ApprovalPolicy::Prompt);
        assert!(
            !cloned_child.tools.auto_approve,
            "runtime de-escalation must revoke bypass for fleet/parallel children"
        );
    }

    /// FIX F — workflow shadow-detection is a top-level/user-turn signal. A
    /// child engine spawned by a workflow must have the gate OFF even when the
    /// parent has it ON, so sub-agent turns don't pollute the shadow log with
    /// recursive intra-workflow detections. Asserted on the cached gate at the
    /// child-config seam (`child_config` is the single place children are built).
    #[test]
    fn child_config_disables_workflow_detection_even_when_parent_enables_it() {
        let mut parent = Config::default();
        parent.observability.workflow_detection_enabled = true;
        // B6 defense-in-depth: the live confirm gate must also be forced off for
        // children so a workflow's sub-agents can never recursively re-enter it.
        parent.observability.workflow_live_mode = true;
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let child = spawner.child_config(&sub_config());

        assert!(
            !child.observability.workflow_detection_enabled,
            "workflow-spawned child must have workflow_detection forced off"
        );
        assert!(
            !child.observability.workflow_live_mode,
            "workflow-spawned child must have the live confirm gate forced off"
        );
    }

    /// Crucible enhancement #1 — a council member must get a minimal,
    /// council-specific system prompt instead of inheriting the host one. With
    /// the parent carrying a sentinel host prompt, the child config built from a
    /// `SubAgentConfig` that supplies an explicit `system_prompt` must equal that
    /// minimal prompt and must NOT contain the host sentinel (which would mean
    /// the multi-K-token host prompt is being re-billed × N members).
    #[test]
    fn council_proposer_system_prompt_replaces_host_prompt() {
        let parent = Config {
            system_prompt: Some("HOST-SECRET-PROMPT".to_string()),
            ..Config::default()
        };
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), parent);

        let sub = SubAgentConfig {
            name: "p".to_string(),
            prompt: "task".to_string(),
            max_turns: 2,
            max_tokens: 16,
            system_prompt: Some("MINIMAL COUNCIL".to_string()),
            provider: None,
            model: None,
            temperature: None,
        };
        let child = spawner.child_config(&sub);

        assert_eq!(
            child.system_prompt.as_deref(),
            Some("MINIMAL COUNCIL"),
            "child must use the explicit minimal council system prompt"
        );
        assert!(
            !child.system_prompt.unwrap().contains("HOST-SECRET-PROMPT"),
            "child must NOT inherit the host system prompt (no re-billing × N)"
        );
    }

    #[tokio::test]
    async fn production_spawner_and_clones_read_latest_session_turn() {
        let root = tokio_util::sync::CancellationToken::new();
        let mut guard = crate::cancel::SessionRuntimeGuard::new(root);
        let runtime = guard.observer();
        let first_turn = tokio_util::sync::CancellationToken::new();
        guard.set_active_turn(first_turn.clone());
        let spawner = AgentSpawner::new(Arc::new(NeverProvider), Config::default())
            .with_session_runtime(runtime.clone());
        let first_child = spawner.active_cancel_token().child_token();

        let second_turn = tokio_util::sync::CancellationToken::new();
        guard.set_active_turn(second_turn.clone());
        let cloned_spawner = spawner.clone_for_spawn();
        let second_child = cloned_spawner.active_cancel_token().child_token();

        first_turn.cancel();
        assert!(first_child.is_cancelled());
        assert!(
            !second_child.is_cancelled(),
            "a completed prior turn must not cancel a later child"
        );
        second_turn.cancel();
        assert!(second_child.is_cancelled());
    }

    /// Rank 7 — a host cancel must propagate into spawned sub-agents. With the
    /// parent token already fired, the child engine observes `is_cancelled()`
    /// at its first turn boundary and returns WITHOUT reaching the provider
    /// (`NeverProvider::stream` errors with "never called" if hit). The absence
    /// of that error proves the child inherited the parent's cancel token.
    #[tokio::test]
    async fn cancelled_parent_short_circuits_spawned_child() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let spawner =
            AgentSpawner::new(Arc::new(NeverProvider), Config::default()).with_cancel(cancel);

        let result = spawner.spawn_one(sub_config()).await;

        assert!(
            !result.text.contains("never called"),
            "a cancelled parent must short-circuit the child before the provider; got: {}",
            result.text
        );
    }
}

#[cfg(test)]
mod fail_loud_tests {
    use super::{relay_subagent_terminal, subagent_ok_result};
    use crate::agents::channel_sink::{
        ChannelSink, SubAgentRelay, SubAgentTerminalRelay, TERMINAL_CAPACITY,
    };
    use wcore_protocol::events::WorkflowChildTerminalState;
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};

    fn agent_result(text: &str, finish: FinishReason) -> crate::engine::AgentResult {
        crate::engine::AgentResult {
            text: text.to_string(),
            // stop_reason is hardcoded to MaxTurns by finish_run_terminated
            // regardless of the real cause, which is why subagent_ok_result
            // branches on finish_reason, not stop_reason.
            stop_reason: StopReason::MaxTurns,
            finish_reason: finish,
            usage: TokenUsage::default(),
            usage_delta: TokenUsage::default(),
            turns: 3,
            active_window_percent: None,
            agent_run_id: None,
        }
    }

    #[test]
    fn terminated_empty_run_is_error_with_synthesized_cause() {
        // #661: a sub-agent that hit the turn cap with no output must be an
        // error carrying a legible cause, not a silent empty success.
        let out = subagent_ok_result("child".into(), agent_result("", FinishReason::MaxTurns));
        assert!(out.is_error, "a non-Stop finish must be flagged is_error");
        assert!(
            out.text.contains("terminated") && out.text.contains("turn limit"),
            "empty terminated body gets a cause line, got: {}",
            out.text
        );
    }

    #[test]
    fn token_capped_answer_with_text_is_usable_not_error() {
        // A complete answer that ends exactly at the output-token cap comes back
        // as Length WITH text — degraded-but-usable, not a failure. Flagging it
        // would wrongly drop it from council quorum. Keep text, is_error=false.
        let out = subagent_ok_result(
            "child".into(),
            agent_result("the answer", FinishReason::Length),
        );
        assert!(!out.is_error, "a non-empty Length result must stay usable");
        assert_eq!(out.text, "the answer");
    }

    #[test]
    fn empty_length_termination_is_error_with_cause() {
        // An EMPTY Length (the context/budget-ceiling abort path) produced no
        // answer → error with a synthesized cause, not a silent empty success.
        let out = subagent_ok_result("child".into(), agent_result("", FinishReason::Length));
        assert!(
            out.is_error,
            "an empty Length termination is a real failure"
        );
        assert!(
            out.text.contains("context, budget, or output-length limit"),
            "cause line names the limit, got: {}",
            out.text
        );
    }

    #[test]
    fn clean_completion_is_success() {
        // A clean EndTurn (FinishReason::Stop) is the only unconditional success.
        let out = subagent_ok_result("child".into(), agent_result("done", FinishReason::Stop));
        assert!(!out.is_error);
        assert_eq!(out.text, "done");
    }

    #[tokio::test]
    async fn final_result_drives_typed_terminal_disposition() {
        let (stream_tx, _stream_rx) = tokio::sync::mpsc::channel::<SubAgentRelay>(1);
        let (terminal_tx, mut terminal_rx) =
            tokio::sync::mpsc::channel::<SubAgentTerminalRelay>(TERMINAL_CAPACITY);
        let sink = ChannelSink::new_with_terminal(
            "workflow:scan".into(),
            "scan".into(),
            stream_tx,
            terminal_tx,
        );
        let result = subagent_ok_result("scan".into(), agent_result("", FinishReason::MaxTurns));

        relay_subagent_terminal(Some(&sink), &result);

        let terminal = terminal_rx.recv().await.expect("terminal result");
        assert_eq!(terminal.terminal_state, WorkflowChildTerminalState::Failed);
        assert_eq!(terminal.relay.inner["type"], "error");
        assert!(
            terminal.relay.inner["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("turn limit"))
        );
    }
}
