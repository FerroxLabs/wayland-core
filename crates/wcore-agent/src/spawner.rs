use std::sync::Arc;

use async_trait::async_trait;

use wcore_config::config::Config;
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
use wcore_types::message::TokenUsage;

use crate::agents::bus::{AgentBus, AgentMessage, now_ms, preview};
use crate::agents::channel_sink::ChannelSink;
use crate::engine::AgentEngine;
use crate::output::OutputSink;
use crate::output::null_sink::NullSink;

// Re-export from wcore-types — single source of truth
pub use wcore_types::spawner::{ForkOverrides, Spawner, SubAgentConfig, SubAgentResult};

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
    /// v0.8.0 Task J — optional `AgentBus` for lifecycle event
    /// publication. `None` preserves the legacy "silent spawner"
    /// behaviour expected by older tests; production callers attach the
    /// engine's bus via `with_bus(...)`.
    bus: Option<Arc<AgentBus>>,
}

impl AgentSpawner {
    pub fn new(provider: Arc<dyn LlmProvider>, config: Config) -> Self {
        Self {
            provider,
            base_config: config,
            bus: None,
        }
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

    /// Spawn a single sub-agent and wait for result.
    pub async fn spawn_one(&self, sub_config: SubAgentConfig) -> SubAgentResult {
        // Security audit H-7 / M-9: `child_config` inherits the parent's
        // approval posture (no forced `auto_approve = true`), and
        // `build_tool_registry(&[])` defaults to a read-only toolset.
        let config = self.child_config(&sub_config);

        let tools = build_tool_registry(&[]);
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let mut engine =
            AgentEngine::new_with_provider(self.provider.clone(), config, tools, output);

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
                SubAgentResult {
                    name: sub_config.name,
                    text: result.text,
                    usage: result.usage,
                    turns: result.turns,
                    is_error: false,
                }
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
        let futures: Vec<_> = sub_configs
            .into_iter()
            .map(|config| {
                let spawner = self.clone_for_spawn();
                let extras = extras.clone();
                tokio::spawn(async move { spawner.spawn_one_with_extras(config, extras).await })
            })
            .collect();

        let mut results = Vec::new();
        for future in futures {
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
        let run_id = run_id.into();
        let fleet = FleetDispatcher::new(run_id).with_shard_size(DEFAULT_SHARD_SIZE);

        // Build one MeshAgent per task. Each agent owns a clone of the
        // spawner (cheap — same Arc/Config plumbing the legacy
        // spawn_parallel path uses) and reports back the SubAgentResult
        // serialized into the AgentReport payload so the reducer can
        // reconstruct it on the orchestrator side.
        let agents: Vec<MeshAgent> = sub_configs
            .into_iter()
            .map(|sub_config| -> MeshAgent {
                let spawner = self.clone_for_spawn();
                Box::new(move |ctx: BlackboardCtx| {
                    Box::pin(async move {
                        // Wire-presence signal: tag the per-sub-agent
                        // Spawned event with the shard-scoped id so a
                        // bus subscriber can prove the Fleet path ran.
                        let extras = SpawnExtras {
                            channel_sink: None,
                            agent_name: None,
                            parent_call_id: Some(format!("fleet:{}", ctx.agent_id)),
                        };
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
        let futures: Vec<_> = tasks_and_extras
            .into_iter()
            .map(|(config, extras)| {
                let spawner = self.clone_for_spawn();
                tokio::spawn(async move { spawner.spawn_one_with_extras(config, extras).await })
            })
            .collect();

        let mut results = Vec::new();
        for future in futures {
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

        let tools = build_tool_registry(&[]);
        // v0.9.4 W1.1b: keep a clone of the sink BEFORE moving it into the
        // engine so we can call emit_info/emit_error AFTER engine.run() returns.
        // The clone is cheap (Arc bump); the engine holds the primary ref.
        let output: Arc<dyn OutputSink> = match extras.channel_sink {
            Some(sink) => sink as Arc<dyn OutputSink>, // sub-agent events flow back through parent
            None => Arc::new(NullSink),                // legacy anonymous behaviour
        };
        let terminal_output = Arc::clone(&output);
        let mut engine =
            AgentEngine::new_with_provider(self.provider.clone(), config, tools, output);

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
                // v0.9.4 W1.1b: emit terminal info event BEFORE the ChannelSink tx
                // drops. The bridge sets SubAgentStatus::Done on `kind == "info"`.
                terminal_output.emit_info(&format!(
                    "sub-agent '{}' completed ({} turns)",
                    sub_config.name, result.turns
                ));
                SubAgentResult {
                    name: sub_config.name,
                    text: result.text,
                    usage: result.usage,
                    turns: result.turns,
                    is_error: false,
                }
            }
            Err(e) => {
                self.publish_errored(&sub_config.name, &e.to_string());
                guard.outcome = TerminalOutcome::Published;
                // v0.9.4 W1.1b: emit terminal error event before tx drops.
                // The bridge sets SubAgentStatus::Failed on `kind == "error"`.
                terminal_output.emit_error(&e.to_string(), false);
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
        config.max_turns = Some(sub_config.max_turns);
        config.max_tokens = sub_config.max_tokens;
        if let Some(sp) = sub_config.system_prompt.clone() {
            config.system_prompt = Some(sp);
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
            bus: self.bus.clone(),
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

        let tools = build_tool_registry(&overrides.allowed_tools);
        let output: Arc<dyn OutputSink> = Arc::new(NullSink);
        let mut engine =
            AgentEngine::new_with_provider(self.provider.clone(), config, tools, output);
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
                SubAgentResult {
                    name: sub_config.name,
                    text: result.text,
                    usage: result.usage,
                    turns: result.turns,
                    is_error: false,
                }
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

fn build_tool_registry(allowed: &[String]) -> ToolRegistry {
    let all: &[(&str, ToolFactory)] = &[
        ("Read", || Box::new(ReadTool::new(None))),
        ("Write", || Box::new(WriteTool::new(None))),
        ("Edit", || Box::new(EditTool::new(None))),
        ("Bash", || Box::new(BashTool)),
        ("Grep", || Box::new(GrepTool)),
        ("Glob", || Box::new(GlobTool)),
    ];

    let mut registry = ToolRegistry::new();
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
mod phase7_tests {
    use super::{ForkOverrides, SubAgentConfig, build_tool_registry};

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
        let registry = build_tool_registry(&[]);
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
        let registry = build_tool_registry(&["Bash".to_string(), "Write".to_string()]);
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
        let registry = build_tool_registry(&allowed);
        assert!(registry.get("Bash").is_some());
        assert!(registry.get("Read").is_some());
        assert!(registry.get("Write").is_none());
    }

    #[test]
    fn tc_7_sub_agent_config_original_fields_intact() {
        let config = SubAgentConfig {
            name: "test-agent".to_string(),
            prompt: "do the task".to_string(),
            max_turns: 5,
            max_tokens: 1024,
            system_prompt: Some("you are helpful".to_string()),
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
    //! `auto_approve` and `allow_list` unchanged.

    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use wcore_config::config::{Config, ToolsConfig};
    use wcore_providers::{LlmProvider, ProviderError};
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
}
