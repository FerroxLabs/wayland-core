//! A3 — `WorkflowRunner`: executes a lowered [`GraphConfig`] by walking it
//! in dependency order and dispatching each `AgentCall` through the
//! [`AgentSpawner`] path: `spawn_one` for a linear chain;
//! `spawn_parallel_with_per_task_extras` for a fan-out up to
//! [`FLEET_FANOUT_THRESHOLD`] siblings (per-task `ChannelSink` relay); and
//! `spawn_via_fleet` (real `FleetDispatcher` sharding) for any wider fan-out.
//!
//! ## Why a separate executor (not the walker)
//!
//! The per-turn `ExecutionGraph::execute` walker (`orchestration/graph.rs`)
//! is **inert for multi-agent work**: the engine's first-dispatch-wins guard
//! (`orchestration/node_executor.rs:202-207`) limits a turn to one real
//! `AgentCall`. `WorkflowRunner` runs **outside** the engine's per-turn
//! one-batch contract — each `spawn_one` builds a fresh `AgentEngine` with its
//! own turn loop, so all stages run for real. This is the assumption
//! `tests/workflow_runner_spike.rs` proved; this module generalises that
//! kernel from a flat sequence to an arbitrary lowered `GraphConfig`.
//!
//! ## What it does
//!
//! 1. Maintains a `serde_json::Value` object as the running state.
//! 2. Walks nodes in topological order (Kahn over the edge set), entry first.
//!    Nodes that become ready in the same wave and are sibling `AgentCall`s
//!    (a fan-out sharing a predecessor) are dispatched **concurrently**; a
//!    linear chain naturally resolves one node per wave (sequential).
//! 3. For each `AgentCall`: resolve the node's `InputMapper` against the
//!    current state, build a `SubAgentConfig` whose prompt is the authored
//!    node prompt with the resolved input injected, dispatch, then write
//!    `result.text` onto `state[node_id]`.
//! 4. `Aggregator` nodes fold their inbound siblings' outputs per the node's
//!    `StateReducer` (`Replace`/`SumNumbers`/`Collect`) into the aggregator's
//!    state key.
//! 5. `PassThrough` copies state through; `End` terminates the walk;
//!    `Predicate`/`Loop` get a minimal bounded handling (documented below).
//!
//! ## The prompt side-table
//!
//! The lowered `GraphConfig` carries only node ids + input mappers — the
//! authored prompts (and schema refs) are discarded by A1's lowering. The
//! runner therefore takes a [`WorkflowPlan`] (graph + per-node prompts +
//! schema table), which [`WorkflowPlan::parse`] / [`WorkflowPlan::from_workflow`]
//! build from the RON in a single pass alongside the existing lowering.
//!
//! ## Schema validation and pipelining
//!
//! - **Schema validation (A4):** `schema::validate(&result.text, schema)` runs
//!   at every agent-return site (bounded by `MAX_SCHEMA_RETRIES`); there is no
//!   stub placeholder.
//! - **No-barrier pipeline (A5):** linear chains that opt into item-level
//!   streaming dispatch through [`pipeline::run_pipeline`] rather than the
//!   per-stage PassThrough path.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;
use tokio::sync::Semaphore;

use super::super::graph::{AggregationStrategy, GraphConfig, InputMapper, Node, StateReducer};
use super::dsl::{self, AgentSpec, Step, Workflow};
use super::error::WorkflowParseError;
use super::limits::{self, DispatchBudget};
use super::meta::WorkflowMeta;
use super::pipeline;
use super::schema::{self, WorkflowSchema};
use crate::agents::channel_sink::{ChannelSink, SubAgentRelay};
use crate::output::OutputSink;
use crate::spawner::{AgentSpawner, SpawnExtras, SubAgentConfig, SubAgentResult};

/// Default per-stage turn budget. Workflow stages are single-shot
/// instructions, so a small budget keeps a stuck stage from burning the
/// whole run. A node that genuinely needs more raises it per-node via the
/// `AgentSpec.max_turns` override, carried through `GraphConfig.node_budgets`
/// and applied by [`node_turn_budget`] / [`node_token_budget`] below.
const DEFAULT_MAX_TURNS: usize = 8;

/// Default per-stage token budget.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Resolve a node's turn budget: the per-node `AgentSpec.max_turns` override
/// (lowered into `graph.node_budgets`) when present, else `DEFAULT_MAX_TURNS`.
fn node_turn_budget(graph: &GraphConfig, node_id: &str) -> usize {
    graph
        .node_budgets
        .get(node_id)
        .and_then(|b| b.max_turns)
        .map(|t| t as usize)
        .unwrap_or(DEFAULT_MAX_TURNS)
}

/// Resolve a node's token budget: the per-node `AgentSpec.max_tokens` override
/// when present, else `DEFAULT_MAX_TOKENS`.
fn node_token_budget(graph: &GraphConfig, node_id: &str) -> u32 {
    graph
        .node_budgets
        .get(node_id)
        .and_then(|b| b.max_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS)
}

/// Resolve a node's pinned provider spec from `graph.node_providers` (lowered
/// from `AgentSpec.provider`). `None` ⇒ the node inherits the spawner's
/// provider. Mirrors the `node_*_budget` side-table lookups above.
fn node_provider(graph: &GraphConfig, node_id: &str) -> Option<String> {
    graph
        .node_providers
        .get(node_id)
        .and_then(|p| p.provider.clone())
}

/// Resolve a node's pinned model from `graph.node_providers`.
fn node_pinned_model(graph: &GraphConfig, node_id: &str) -> Option<String> {
    graph
        .node_providers
        .get(node_id)
        .and_then(|p| p.model.clone())
}

/// Bound on `Loop`/predicate-gated iteration so a malformed predicate can
/// never spin forever. Mirrors the walker's `max_iters` discipline. Exposed to
/// the cost estimator (FIX B) so its `Loop` agent count uses the SAME cap the
/// runner's `run_loop` enforces: `agents.len() * min(max_iters, LOOP_ITER_CAP)`.
pub(crate) const LOOP_ITER_CAP: usize = 16;

/// How many times a schema-bearing `AgentCall` is re-dispatched after a
/// validation mismatch before the run fails. The first attempt plus N retries
/// means up to `1 + MAX_SCHEMA_RETRIES` total dispatches for that node.
const MAX_SCHEMA_RETRIES: usize = 2;

/// Fan-out width at or below which the runner keeps the per-task relay path
/// (`spawn_parallel_with_per_task_extras`, one `ChannelSink` per sub-agent) and
/// above which it routes the wave through `FleetDispatcher` sharding
/// (`spawn_via_fleet`). The threshold is the fleet's own shard size: a wave that
/// fits in a single shard gains nothing from the fleet's hierarchical reduction
/// but DOES lose the per-task `ChannelSink` relay the bridge depends on, so it
/// stays on the relay path. A wave wider than one shard is exactly the "scales
/// to 100 agents" case the fleet exists for, so it shards. Equal to
/// [`wcore_swarm::DEFAULT_SHARD_SIZE`] (10) by construction.
const FLEET_FANOUT_THRESHOLD: usize = wcore_swarm::DEFAULT_SHARD_SIZE;

/// Errors raised while executing a lowered workflow graph.
#[derive(Debug, Error)]
pub enum WorkflowRunError {
    /// The graph's declared entry node has no matching node entry.
    #[error("unknown entry node `{0}`")]
    UnknownEntry(String),

    /// An edge pointed at a node id that is not in the graph.
    #[error("edge targets unknown node `{0}`")]
    UnknownTarget(String),

    /// The graph contains a cycle (or an unsatisfiable dependency), so the
    /// topological walk could not make progress. Carries the ids that never
    /// became ready.
    #[error("graph has a cycle or unreachable nodes: {0:?}")]
    Cycle(Vec<String>),

    /// A node referenced a prompt that the plan's prompt table does not
    /// define. Only `AgentCall` nodes require a prompt.
    #[error("agent node `{0}` has no prompt in the workflow plan")]
    MissingPrompt(String),

    /// One or more stages failed. Carries the partial state and the
    /// per-stage results gathered before the failure surfaced so callers
    /// never discard completed work (PLAN C1 partial-results kernel).
    #[error("stage `{stage}` failed: {message}")]
    StageFailed {
        stage: String,
        message: String,
        partial: Box<WorkflowRunResult>,
    },

    /// A schema-bearing stage's output still failed schema validation after
    /// `MAX_SCHEMA_RETRIES` re-dispatches. Carries the last validation error
    /// and the partial result gathered so far (same kernel as `StageFailed`).
    #[error("stage `{stage}` output failed schema validation after {attempts} attempts: {message}")]
    SchemaValidationFailed {
        stage: String,
        attempts: usize,
        message: String,
        partial: Box<WorkflowRunResult>,
    },

    /// A dispatched node id could not be resolved back to a node in the graph.
    /// Normally unreachable — the walk only dispatches ids drawn from the node
    /// map — but propagated rather than panicked if the invariant is violated.
    #[error("internal: dispatched node `{0}` is not present in the graph")]
    NodeNotInGraph(String),

    /// The run hit the per-run sub-agent dispatch budget
    /// ([`limits::MAX_TOTAL_DISPATCHES`]). The central DoS backstop: it counts
    /// every dispatch on every path (single, fan-out, fleet, pipeline item,
    /// schema retry, loop iteration), so it is robust to retries and
    /// runtime-injected `over:` arrays. Carries the partial result gathered so
    /// far (same kernel as `StageFailed`).
    #[error(
        "workflow dispatch budget exceeded: would dispatch sub-agent #{attempted} (limit {limit})"
    )]
    DispatchBudgetExceeded {
        limit: usize,
        attempted: usize,
        partial: Box<WorkflowRunResult>,
    },
}

/// One executed node's outcome, in execution order.
#[derive(Debug, Clone)]
pub struct StageResult {
    /// The graph node id.
    pub node_id: String,
    /// The agent text output (empty for non-`AgentCall` nodes).
    pub text: String,
    /// Whether the underlying sub-agent reported an error.
    pub is_error: bool,
    /// Turn count the sub-agent ran (0 for non-`AgentCall` nodes).
    pub turns: usize,
}

/// An agent stage's output after schema resolution: the structured `value` to
/// store in state (the validated JSON for schema nodes, a JSON string for
/// schema-less nodes) alongside the raw `text` + `turns` for the stage record.
pub(crate) struct ResolvedStage {
    pub(crate) value: Value,
    pub(crate) text: String,
    pub(crate) turns: usize,
}

/// Why [`WorkflowRunner::resolve_schema`] failed: either the output never
/// validated within the retry budget, or a retry would have exceeded the
/// per-run dispatch budget (FIX 1). Distinguished so the caller surfaces the
/// right typed [`WorkflowRunError`].
enum ResolveSchemaErr {
    /// Schema validation failed after `attempts` dispatches.
    Validation {
        attempts: usize,
        message: String,
        last: SubAgentResult,
    },
    /// A retry would have exceeded the dispatch budget (`attempted` = the count
    /// the charge would have reached). `last` is the most recent result so the
    /// caller can still record the partial stage.
    BudgetExceeded {
        attempted: usize,
        last: SubAgentResult,
    },
}

/// Runner-side seam handed to the no-barrier [`pipeline`] scheduler so it can
/// dispatch + schema-validate stage agents without re-implementing A4's logic
/// or owning the schema table. Borrows the compiled schema defs from the plan;
/// carries the per-stage turn/token budget the runner uses everywhere.
pub(crate) struct PipelineStageDispatch<'a> {
    /// schema name → compiled body (from [`WorkflowPlan::schema_defs`]).
    pub(crate) schema_defs: &'a HashMap<String, WorkflowSchema>,
    pub(crate) max_turns: usize,
    pub(crate) max_tokens: u32,
    /// FIX 1 — the per-run dispatch budget, shared with the rest of the run.
    /// Charged before every pipeline stage/retry dispatch.
    pub(crate) budget: &'a DispatchBudget,
}

/// Outcome of resolving a pipeline stage's schema. `Dropped` drops the item to
/// `null` (a per-item failure the pipeline tolerates); `BudgetExceeded` is a
/// hard run-abort signal (`attempted` = the count the charge would have reached)
/// the pipeline propagates so the runner aborts the whole run (FIX 1).
pub(crate) enum StageSchemaErr {
    Dropped { message: String, turns: usize },
    BudgetExceeded { attempted: usize, turns: usize },
}

/// Resolve one pipeline stage's just-returned output against its declared
/// schema (if any), retrying on mismatch — the per-item analogue of
/// [`WorkflowRunner::resolve_schema`].
///
/// - **No schema:** store the raw text as a JSON string.
/// - **Schema, valid:** store the parsed structured `Value`.
/// - **Schema, invalid:** re-dispatch the same stage (for this item) up to
///   `MAX_SCHEMA_RETRIES` times, appending the validation error each time. On
///   exhausting the budget, return [`StageSchemaErr::Dropped`] so the caller
///   drops the item to `null`. A retry that would exceed the per-run dispatch
///   budget returns [`StageSchemaErr::BudgetExceeded`] (a hard run-abort).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_stage_schema(
    spawner: &AgentSpawner,
    dispatch: &PipelineStageDispatch<'_>,
    pipeline_id: &str,
    item_index: usize,
    stage: &AgentSpec,
    first: SubAgentResult,
    input: &Value,
    sem: &Arc<Semaphore>,
) -> Result<ResolvedStage, StageSchemaErr> {
    // No schema → store text verbatim as a JSON string.
    let Some(schema_name) = &stage.schema else {
        return Ok(ResolvedStage {
            value: Value::String(first.text.clone()),
            text: first.text,
            turns: first.turns,
        });
    };
    // A missing compiled body means "no constraint" (defensive — the lowering
    // verified the ref resolves and the plan compiled every body).
    let Some(schema) = dispatch.schema_defs.get(schema_name) else {
        return Ok(ResolvedStage {
            value: Value::String(first.text.clone()),
            text: first.text,
            turns: first.turns,
        });
    };

    let mut result = first;
    for attempt in 0..=MAX_SCHEMA_RETRIES {
        match schema::validate(&result.text, schema) {
            Ok(value) => {
                return Ok(ResolvedStage {
                    value,
                    text: result.text,
                    turns: result.turns,
                });
            }
            Err(err) => {
                if attempt == MAX_SCHEMA_RETRIES {
                    return Err(StageSchemaErr::Dropped {
                        message: err.to_string(),
                        turns: result.turns,
                    });
                }
                // FIX 1 — a schema retry is a dispatch; charge it before
                // re-dispatching so per-item retries cannot escape the run budget.
                if let Err(attempted) = dispatch.budget.try_charge() {
                    return Err(StageSchemaErr::BudgetExceeded {
                        attempted,
                        turns: result.turns,
                    });
                }
                let correction = format!(
                    "Your previous output did not match the required schema: {err}. \
                     Return only valid JSON matching the schema."
                );
                let prompt = format!("{}\n\n{correction}", build_prompt(&stage.prompt, input));
                // Every schema-retry dispatch runs under the pipeline's
                // concurrency regime too, so acquire+hold a permit for it —
                // otherwise retries would escape the cap. A closed semaphore
                // (never closed by the runner) drops the item.
                let Ok(permit) = sem.clone().acquire_owned().await else {
                    return Err(StageSchemaErr::Dropped {
                        message: "pipeline semaphore closed".to_string(),
                        turns: result.turns,
                    });
                };
                result = spawner
                    .spawn_one(SubAgentConfig {
                        name: format!("{pipeline_id}[{item_index}]:{}", stage.id),
                        prompt,
                        max_turns: dispatch.max_turns,
                        max_tokens: dispatch.max_tokens,
                        system_prompt: None,
                        // The pipeline stage IS an AgentSpec, so its provider /
                        // model pin is read directly (no node_providers lookup).
                        provider: stage.provider.clone(),
                        model: stage.model.clone(),
                        temperature: None,
                    })
                    .await;
                drop(permit);
                if result.is_error {
                    return Err(StageSchemaErr::Dropped {
                        message: result.text.clone(),
                        turns: result.turns,
                    });
                }
            }
        }
    }
    Err(StageSchemaErr::Dropped {
        message: "schema validation exhausted retries".to_string(),
        turns: result.turns,
    })
}

/// Result of executing a workflow graph to completion.
#[derive(Debug, Clone)]
pub struct WorkflowRunResult {
    /// The final merged state object after the walk.
    pub final_state: Value,
    /// Per-node results in execution order.
    pub stage_results: Vec<StageResult>,
}

/// A ready-to-execute workflow: the lowered graph plus the side-tables the
/// graph IR does not carry (per-node prompts, the named schema table, and
/// the author meta).
pub struct WorkflowPlan {
    /// The lowered execution graph.
    pub graph: GraphConfig,
    /// node_id → authored prompt for every `AgentCall` node.
    pub prompts: HashMap<String, String>,
    /// node_id → named schema ref, for nodes that declared one.
    pub schemas: HashMap<String, String>,
    /// schema name → compiled schema body (A4). Built from the workflow's
    /// `schemas` table; the runner resolves `schemas[node_id]` against this.
    pub schema_defs: HashMap<String, WorkflowSchema>,
    /// pipeline_id → no-barrier pipeline spec, for `Pipeline` steps that
    /// declared `over: Some(ref)`. The lowering emits a single placeholder
    /// node per such pipeline; the runner detects its id here and delegates
    /// to [`pipeline::run_pipeline`] instead of treating it as a PassThrough.
    /// Classic (no-`over`) pipelines do NOT appear here — they lower to a
    /// node chain the Kahn walker dispatches directly.
    pub pipelines: HashMap<String, PipelineDef>,
    /// Author meta (name/description/est_agents).
    pub meta: WorkflowMeta,
}

/// A lowered no-barrier pipeline step: the `over` state ref plus the ordered
/// stage specs. Held in [`WorkflowPlan::pipelines`] keyed by the step id.
#[derive(Debug, Clone)]
pub struct PipelineDef {
    /// State ref resolving to the array of items to stream (e.g. `changed_files`).
    pub over: String,
    /// Ordered stages each item flows through.
    pub stages: Vec<AgentSpec>,
}

impl WorkflowPlan {
    /// Parse RON into a plan: lowers onto a `GraphConfig` (reusing A1's
    /// [`dsl::parse_workflow`]) and extracts the per-node prompt + schema
    /// side-tables the graph IR drops.
    pub fn parse(src: &str) -> Result<Self, WorkflowParseError> {
        dsl::guard_ron_size_and_depth(src)?;
        let workflow: Workflow =
            ron::from_str(src).map_err(|e| WorkflowParseError::Ron(e.to_string()))?;
        Self::from_workflow(workflow)
    }

    /// Build a plan from an already-parsed [`Workflow`] IR (e.g. a B7
    /// synthesizer holding the IR without re-serialising to RON).
    pub fn from_workflow(workflow: Workflow) -> Result<Self, WorkflowParseError> {
        // Collect prompts/schemas and the no-barrier pipeline side-table
        // before lowering moves the workflow.
        let mut prompts = HashMap::new();
        let mut schemas = HashMap::new();
        let mut pipelines = HashMap::new();
        for phase in &workflow.phases {
            for step in &phase.steps {
                collect_specs(step, &mut prompts, &mut schemas, &mut pipelines);
            }
        }
        // Compile every named schema body (the `schemas` table values) into the
        // validatable subset up front, so a malformed schema definition is a
        // parse-time error rather than a per-run surprise. A1's lowering already
        // verified that each node's `schema: Some("name")` ref resolves to a key
        // here; only the *bodies* remain to compile.
        let mut schema_defs = HashMap::with_capacity(workflow.schemas.len());
        for (name, body) in &workflow.schemas {
            let compiled =
                WorkflowSchema::parse(body).map_err(|e| WorkflowParseError::InvalidSchema {
                    name: name.clone(),
                    message: e.to_string(),
                })?;
            schema_defs.insert(name.clone(), compiled);
        }
        let (graph, meta) = dsl::lower(workflow)?;
        // FIX 4 — graph blow-up guard: a document whose phases lower to an
        // enormous node set is rejected so the Kahn walk cannot be driven into
        // a pathological size.
        if graph.nodes.len() > limits::MAX_WORKFLOW_NODES {
            return Err(WorkflowParseError::TooManyNodes {
                count: graph.nodes.len(),
                limit: limits::MAX_WORKFLOW_NODES,
            });
        }
        Ok(Self {
            graph,
            prompts,
            schemas,
            schema_defs,
            pipelines,
            meta,
        })
    }
}

/// Walk a step, recording each contained `AgentSpec`'s prompt and schema ref
/// keyed by the node id the lowering will assign (always `spec.id`).
fn collect_specs(
    step: &Step,
    prompts: &mut HashMap<String, String>,
    schemas: &mut HashMap<String, String>,
    pipelines: &mut HashMap<String, PipelineDef>,
) {
    let mut record = |spec: &AgentSpec| {
        prompts.insert(spec.id.clone(), spec.prompt.clone());
        if let Some(schema) = &spec.schema {
            schemas.insert(spec.id.clone(), schema.clone());
        }
    };
    match step {
        Step::Agent(spec) => record(spec),
        // A no-barrier pipeline (`over: Some`) lowers to a single placeholder
        // node, not a stage chain — its stages live in the side-table, not the
        // node prompt/schema maps. A classic pipeline (`over: None`) lowers to a
        // node chain, so its stages ARE node prompts/schemas.
        Step::Pipeline {
            id,
            over: Some(over),
            stages,
        } => {
            pipelines.insert(
                id.clone(),
                PipelineDef {
                    over: over.clone(),
                    stages: stages.clone(),
                },
            );
        }
        Step::Pipeline {
            over: None, stages, ..
        } => stages.iter().for_each(&mut record),
        Step::Parallel { branches, .. } => branches.iter().for_each(record),
    }
}

/// Executes a lowered workflow graph by dispatching agents through an
/// [`AgentSpawner`], independent of the per-turn walker.
pub struct WorkflowRunner<'a> {
    spawner: &'a AgentSpawner,
    /// Shared concurrency cap for no-barrier pipeline stage dispatch. Bounds
    /// total in-flight pipeline stage agents so a wide pipeline cannot starve
    /// the relay/heartbeat tasks (PLAN A5 / gemini's starvation flag). A
    /// single semaphore is reused across every pipeline step in a run.
    pipeline_sem: Arc<Semaphore>,
    /// ForgeFlows-Live Phase 1: parent `OutputSink` for sub-agent event relay.
    /// When `Some`, every dispatched workflow sub-agent runs with a per-task
    /// [`ChannelSink`] whose events are wrapped as `SubAgentEvent` and emitted
    /// up to the parent — exactly like `SpawnTool::spawn_with_relay`. When
    /// `None`, sub-agents run with a `NullSink` (legacy behaviour) so nothing
    /// regresses for callers that never wire a parent.
    parent_output: Option<Arc<dyn OutputSink>>,
}

impl<'a> WorkflowRunner<'a> {
    pub fn new(spawner: &'a AgentSpawner) -> Self {
        Self::with_pipeline_concurrency(spawner, pipeline::DEFAULT_PIPELINE_CONCURRENCY)
    }

    /// Construct a runner with an explicit no-barrier-pipeline concurrency cap.
    /// Used by tests asserting the cap is honoured; production uses
    /// [`WorkflowRunner::new`]'s default.
    pub fn with_pipeline_concurrency(spawner: &'a AgentSpawner, cap: usize) -> Self {
        Self {
            spawner,
            pipeline_sem: Arc::new(Semaphore::new(cap.max(1))),
            parent_output: None,
        }
    }

    /// ForgeFlows-Live Phase 1 builder: attach the parent's `OutputSink` so each
    /// dispatched sub-agent's events relay back via `emit_sub_agent_event`,
    /// mirroring [`crate::spawn_tool::SpawnTool::with_parent_output`]. Without
    /// it the runner dispatches with a `NullSink` (sub-agent events dropped).
    pub fn with_parent_output(mut self, output: Arc<dyn OutputSink>) -> Self {
        self.parent_output = Some(output);
        self
    }

    /// Execute `plan` against an `initial` state object, returning the final
    /// state and per-stage results. On a stage failure, returns
    /// [`WorkflowRunError::StageFailed`] carrying the partial result rather
    /// than discarding completed work.
    pub async fn run(
        &self,
        plan: &WorkflowPlan,
        initial: Value,
    ) -> Result<WorkflowRunResult, WorkflowRunError> {
        let graph = &plan.graph;
        let node_map: HashMap<&str, &Node> =
            graph.nodes.iter().map(|(id, n)| (id.as_str(), n)).collect();

        if !node_map.contains_key(graph.entry.as_str()) {
            return Err(WorkflowRunError::UnknownEntry(graph.entry.clone()));
        }
        for edge in &graph.edges {
            if !node_map.contains_key(edge.to.as_str()) {
                return Err(WorkflowRunError::UnknownTarget(edge.to.clone()));
            }
            if !node_map.contains_key(edge.from.as_str()) {
                return Err(WorkflowRunError::UnknownTarget(edge.from.clone()));
            }
        }

        // Indegree over the edge set (multiple edges to the same node — e.g.
        // every fan-out branch into an aggregator — each count once).
        let mut indegree: HashMap<&str, usize> =
            graph.nodes.iter().map(|(id, _)| (id.as_str(), 0)).collect();
        for edge in &graph.edges {
            *indegree.entry(edge.to.as_str()).or_insert(0) += 1;
        }

        let mut state = match initial {
            Value::Object(_) => initial,
            // Normalise a non-object initial (incl. Null) to an empty object
            // so `state[node_id]` writes always have a home.
            _ => Value::Object(serde_json::Map::new()),
        };
        let mut stage_results: Vec<StageResult> = Vec::new();
        let mut done: HashSet<&str> = HashSet::new();

        // Conditional-branch liveness overlay (fixes the Predicate edge-guard
        // gap). A node is "live" once reached by a *taken* edge (guard
        // satisfied) from a live source; roots (no incoming edges) are live
        // from the start. A node that drains to indegree 0 without ever
        // becoming live is a pruned branch — e.g. the not-taken side of a
        // `Node::Predicate` — and is marked done as a no-op so the graph still
        // terminates, but is never dispatched. This gives the indegree
        // (AND-join) scheduler correct conditional (OR-join) semantics,
        // mirroring the `ExecutionGraph` walker's `edge.when` handling. For a
        // graph with no guarded edges every reachable node stays live, so
        // behaviour is byte-for-byte unchanged from the pre-fix scheduler.
        let mut live: HashSet<&str> = indegree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| *id)
            .collect();

        // FIX 1 — the central DoS backstop. A per-run counter charged before
        // EVERY sub-agent dispatch on every path (Kahn single + fan-out, fleet
        // per sub-config, pipeline per-item-per-stage, schema retries, loop
        // iterations). Once a charge would exceed the budget the run aborts with
        // the partial result. This is the only bound robust to all shapes,
        // including retries and runtime-injected `over:` arrays.
        let budget = DispatchBudget::new();

        // Kahn's algorithm, but processing each wave (all currently-ready
        // nodes) together so a fan-out of sibling AgentCalls dispatches
        // concurrently. A linear chain yields one node per wave.
        loop {
            let ready: Vec<&str> = indegree
                .iter()
                .filter(|(id, deg)| **deg == 0 && !done.contains(*id))
                .map(|(id, _)| *id)
                .collect();
            if ready.is_empty() {
                break;
            }

            // Deterministic order within a wave (HashMap iteration is not).
            let mut ready = ready;
            ready.sort_unstable();

            // Partition the wave: AgentCalls dispatch (possibly concurrently);
            // everything else resolves synchronously against current state.
            let mut agent_nodes: Vec<&str> = Vec::new();
            for id in &ready {
                // Pruned-branch skip: a node that drained to ready without ever
                // becoming live sits on the not-taken side of an upstream guard
                // / Predicate. Mark it done (so its own successors still drain
                // and the terminal cycle-check passes) but never run or dispatch
                // it. Unguarded graphs keep every reachable node live, so this
                // branch never fires there — behaviour is unchanged.
                if !live.contains(id) {
                    done.insert(id);
                    continue;
                }
                match node_map[id] {
                    Node::AgentCall { .. } => agent_nodes.push(id),
                    Node::End => {
                        // Terminal sink: mark done, contributes nothing.
                        done.insert(id);
                        stage_results.push(StageResult {
                            node_id: id.to_string(),
                            text: String::new(),
                            is_error: false,
                            turns: 0,
                        });
                    }
                    Node::PassThrough => {
                        // A5: a `Pipeline(over: ...)` step lowers to a single
                        // PassThrough placeholder. If this id is in the pipeline
                        // side-table, delegate to the no-barrier scheduler;
                        // otherwise it is a true pass-through (synthetic fan-out
                        // root) — no state change.
                        if let Some(def) = plan.pipelines.get(*id) {
                            self.run_no_barrier_pipeline(
                                plan,
                                id,
                                def,
                                &mut state,
                                &mut stage_results,
                                &budget,
                            )
                            .await?;
                        } else {
                            stage_results.push(StageResult {
                                node_id: id.to_string(),
                                text: String::new(),
                                is_error: false,
                                turns: 0,
                            });
                        }
                        done.insert(id);
                    }
                    Node::Predicate { condition } => {
                        // Minimal v1: record the boolean to
                        // `state["predicate_result"]` (mirrors the walker) so
                        // downstream nodes can read it. Edge guards are not yet
                        // honoured by this executor (documented simplification).
                        let val = condition.evaluate(&state);
                        if let Value::Object(m) = &mut state {
                            m.insert("predicate_result".to_string(), Value::Bool(val));
                        }
                        done.insert(id);
                        stage_results.push(StageResult {
                            node_id: id.to_string(),
                            text: String::new(),
                            is_error: false,
                            turns: 0,
                        });
                    }
                    Node::Aggregator { strategy } => {
                        // Fold inbound siblings' outputs into the aggregator's
                        // state key per the node's StateReducer / strategy.
                        self.apply_aggregator(graph, id, strategy, &mut state);
                        done.insert(id);
                        stage_results.push(StageResult {
                            node_id: id.to_string(),
                            text: String::new(),
                            is_error: false,
                            turns: 0,
                        });
                    }
                    Node::Loop {
                        agents,
                        done_check,
                        max_iters,
                    } => {
                        // Minimal bounded loop: run the per-iteration agent
                        // sequence sequentially, capped by min(max_iters, cap),
                        // stopping early when done_check holds. Documented
                        // simplification: loops do not fan out and inherit the
                        // global LOOP_ITER_CAP safety bound.
                        let cap = (*max_iters).min(LOOP_ITER_CAP);
                        self.run_loop(
                            plan,
                            id,
                            agents,
                            done_check,
                            cap,
                            &mut state,
                            &mut stage_results,
                            &budget,
                        )
                        .await?;
                        done.insert(id);
                    }
                }
            }

            // Dispatch the AgentCall sub-wave. One node → sequential
            // (`spawn_one`); a fan-out of ≤ FLEET_FANOUT_THRESHOLD siblings →
            // the per-task relay path; a wider fan-out → FleetDispatcher
            // sharding (`spawn_via_fleet`). See `dispatch_agents`.
            //
            // A5: pipeline no-barrier streaming re-enters here — a linear
            // chain currently dispatches one node per Kahn wave (per-stage).
            if !agent_nodes.is_empty() {
                // FIX 1 — charge the whole wave up front (one charge per node
                // dispatched, fleet fan-out included). If the wave would exceed
                // the budget, abort with the partial result before dispatching.
                if let Err(attempted) = budget.try_charge_n(agent_nodes.len()) {
                    return Err(self.budget_exceeded(attempted, &state, &stage_results));
                }
                let outputs = self.dispatch_agents(plan, &agent_nodes, &state).await;
                // FIX A — two-pass wave processing so a failing sibling never
                // discards completed siblings dispatched in the same wave. PASS 1
                // fully processes and COMMITS every non-error output (state insert
                // + schema resolve + StageResult push + mark done); a sibling that
                // errored (LLM-layer error, schema-validation exhaustion, or a
                // retry tripping the dispatch budget) is recorded as a failed
                // StageResult and its typed error is DEFERRED into `pending_error`
                // (first error wins). PASS 2 (after the loop) returns the deferred
                // error carrying the partial result that now includes every
                // successful sibling — no completed work is dropped.
                let mut pending_error: Option<WorkflowRunError> = None;
                for (id, result) in outputs {
                    if result.is_error {
                        // Record the failed stage; defer the typed error so the
                        // remaining (possibly successful) siblings still commit.
                        stage_results.push(StageResult {
                            node_id: id.clone(),
                            text: result.text.clone(),
                            is_error: true,
                            turns: result.turns,
                        });
                        if pending_error.is_none() {
                            pending_error = Some(WorkflowRunError::StageFailed {
                                stage: id.clone(),
                                message: result.text,
                                // Partial is filled in PASS 2 once the whole
                                // wave has committed (see below).
                                partial: Box::new(WorkflowRunResult {
                                    final_state: Value::Null,
                                    stage_results: Vec::new(),
                                }),
                            });
                        }
                        continue;
                    }

                    // A4 — schema-validated output with retry-on-mismatch. A node
                    // that declared `schema: Some(name)` must return JSON matching
                    // the compiled schema; on mismatch we re-dispatch the same node
                    // (up to MAX_SCHEMA_RETRIES) with the validation error appended
                    // so the agent can correct itself, then store the *validated*
                    // `Value` (not the raw text) so downstream refs see structured
                    // data. Schema-less nodes store their text unchanged.
                    let result = match self
                        .resolve_schema(plan, &id, result, &state, &budget)
                        .await
                    {
                        Ok(resolved) => resolved,
                        Err(ResolveSchemaErr::BudgetExceeded { attempted, last }) => {
                            stage_results.push(StageResult {
                                node_id: id.clone(),
                                text: last.text.clone(),
                                is_error: true,
                                turns: last.turns,
                            });
                            if pending_error.is_none() {
                                // Budget breach is a hard, wave-level abort, but we
                                // still let the rest of the wave commit so its
                                // partial is complete. Recorded as the deferred
                                // error; partial filled in PASS 2.
                                pending_error = Some(WorkflowRunError::DispatchBudgetExceeded {
                                    limit: limits::MAX_TOTAL_DISPATCHES,
                                    attempted,
                                    partial: Box::new(WorkflowRunResult {
                                        final_state: Value::Null,
                                        stage_results: Vec::new(),
                                    }),
                                });
                            }
                            continue;
                        }
                        Err(ResolveSchemaErr::Validation {
                            attempts,
                            message,
                            last,
                        }) => {
                            stage_results.push(StageResult {
                                node_id: id.clone(),
                                text: last.text.clone(),
                                is_error: true,
                                turns: last.turns,
                            });
                            if pending_error.is_none() {
                                pending_error = Some(WorkflowRunError::SchemaValidationFailed {
                                    stage: id.clone(),
                                    attempts,
                                    message,
                                    partial: Box::new(WorkflowRunResult {
                                        final_state: Value::Null,
                                        stage_results: Vec::new(),
                                    }),
                                });
                            }
                            continue;
                        }
                    };

                    if let Value::Object(m) = &mut state {
                        m.insert(id.clone(), result.value);
                    }
                    stage_results.push(StageResult {
                        node_id: id.clone(),
                        text: result.text,
                        is_error: false,
                        turns: result.turns,
                    });
                    // The id originates from a node the wave just dispatched, so
                    // it is always present in `node_map`. Use the same defensive
                    // resolution as the rest of the runner, but DEFER (not return)
                    // on the unreachable miss so the partial is never bypassed.
                    match node_id_ref(&node_map, &id) {
                        Ok(node_ref) => {
                            done.insert(node_ref);
                        }
                        Err(err) => {
                            if pending_error.is_none() {
                                pending_error = Some(err);
                            }
                        }
                    }
                }

                // PASS 2 — if any sibling failed, surface the deferred error now,
                // backfilling its partial with the fully-committed wave (state +
                // every successful sibling's StageResult).
                if let Some(err) = pending_error {
                    return Err(self.with_partial(err, &state, &stage_results));
                }
            }

            // Decrement indegree of every successor of the just-finished
            // wave's nodes so the next wave's frontier opens up.
            for id in &ready {
                if !done.contains(id) {
                    // A Loop error short-circuits above; if a node is still not
                    // done here it is a logic bug — skip to avoid double-dec.
                    continue;
                }
                let source_live = live.contains(id);
                for edge in graph.edges.iter().filter(|e| e.from == *id) {
                    // Evaluate the edge guard against current state; an
                    // unguarded edge (when=None) is always taken.
                    let taken = edge
                        .when
                        .as_ref()
                        .map(|c| c.evaluate(&state))
                        .unwrap_or(true);
                    // Always drain indegree so a join downstream of a pruned
                    // branch still reaches 0 and the run terminates.
                    if let Some(deg) = indegree.get_mut(edge.to.as_str()) {
                        *deg = deg.saturating_sub(1);
                    }
                    // Propagate liveness ONLY along edges actually taken from a
                    // live source. A pruned source (not live) drains its
                    // successors but never marks them live, so a subtree reached
                    // only via not-taken edges stays pruned.
                    if taken && source_live {
                        live.insert(edge.to.as_str());
                    }
                }
            }
        }

        // Any node never marked done means a cycle / unreachable dependency.
        let stuck: Vec<String> = graph
            .nodes
            .iter()
            .map(|(id, _)| id.as_str())
            .filter(|id| !done.contains(id))
            .map(|id| id.to_string())
            .collect();
        if !stuck.is_empty() {
            return Err(WorkflowRunError::Cycle(stuck));
        }

        Ok(WorkflowRunResult {
            final_state: state,
            stage_results,
        })
    }

    /// Resolve a just-returned agent output against the node's declared schema
    /// (if any), retrying on mismatch.
    ///
    /// - **No schema:** the text is stored verbatim as a JSON string.
    /// - **Schema, valid first try:** the parsed `Value` is stored (structured).
    /// - **Schema, invalid:** re-dispatch the same node up to
    ///   `MAX_SCHEMA_RETRIES` times, each time appending the validation error to
    ///   the prompt so the agent can correct. On success, store the validated
    ///   `Value`. After the retry budget is exhausted, return
    ///   [`ResolveSchemaErr::Validation`] so the caller can surface a typed
    ///   [`WorkflowRunError::SchemaValidationFailed`]. If a retry would exceed
    ///   the per-run dispatch budget, return [`ResolveSchemaErr::BudgetExceeded`].
    async fn resolve_schema(
        &self,
        plan: &WorkflowPlan,
        id: &str,
        first: SubAgentResult,
        state: &Value,
        budget: &DispatchBudget,
    ) -> Result<ResolvedStage, ResolveSchemaErr> {
        // No schema ref → store the raw text as a JSON string (legacy behaviour).
        let Some(schema_name) = plan.schemas.get(id) else {
            return Ok(ResolvedStage {
                value: Value::String(first.text.clone()),
                text: first.text,
                turns: first.turns,
            });
        };
        // A schema ref with no compiled body cannot happen: A1 verified the ref
        // resolves and `WorkflowPlan::from_workflow` compiled every body. Guard
        // defensively rather than unwrap — a missing def means "no constraint".
        let Some(schema) = plan.schema_defs.get(schema_name) else {
            return Ok(ResolvedStage {
                value: Value::String(first.text.clone()),
                text: first.text,
                turns: first.turns,
            });
        };

        let mut result = first;
        // Attempt 0 is the original dispatch; attempts 1..=N are retries.
        for attempt in 0..=MAX_SCHEMA_RETRIES {
            match schema::validate(&result.text, schema) {
                Ok(value) => {
                    return Ok(ResolvedStage {
                        value,
                        text: result.text,
                        turns: result.turns,
                    });
                }
                Err(err) => {
                    if attempt == MAX_SCHEMA_RETRIES {
                        return Err(ResolveSchemaErr::Validation {
                            attempts: attempt + 1,
                            message: err.to_string(),
                            last: result,
                        });
                    }
                    // FIX 1 — every schema retry is a dispatch; charge it before
                    // re-dispatching so retries cannot escape the run budget.
                    if let Err(attempted) = budget.try_charge() {
                        return Err(ResolveSchemaErr::BudgetExceeded {
                            attempted,
                            last: result,
                        });
                    }
                    // Re-dispatch the same node with the validation error
                    // appended so the agent returns corrected JSON.
                    let correction = format!(
                        "Your previous output did not match the required schema: {err}. \
                         Return only valid JSON matching the schema."
                    );
                    result = self.dispatch_one(plan, id, state, Some(&correction)).await;
                    if result.is_error {
                        // An LLM-layer error during a retry is itself a schema
                        // failure for this stage — surface it with the budget
                        // consumed so far.
                        return Err(ResolveSchemaErr::Validation {
                            attempts: attempt + 1,
                            message: result.text.clone(),
                            last: result,
                        });
                    }
                }
            }
        }
        // Unreachable: the loop returns on the final attempt. Guard anyway.
        Err(ResolveSchemaErr::Validation {
            attempts: MAX_SCHEMA_RETRIES + 1,
            message: "schema validation exhausted retries".to_string(),
            last: result,
        })
    }

    /// Dispatch a single `AgentCall` node, optionally appending `suffix` to its
    /// composed prompt (used for the schema-correction retry). Mirrors the
    /// single-node path of [`Self::dispatch_agents`].
    async fn dispatch_one(
        &self,
        plan: &WorkflowPlan,
        id: &str,
        state: &Value,
        suffix: Option<&str>,
    ) -> SubAgentResult {
        let input = plan
            .graph
            .nodes
            .iter()
            .find(|(n, _)| n == id)
            .and_then(|(_, n)| match n {
                Node::AgentCall { input_mapper, .. } => Some(input_mapper.apply(state)),
                _ => None,
            })
            .unwrap_or(Value::Null);
        let base = match plan.prompts.get(id) {
            Some(p) => build_prompt(p, &input),
            None => id.to_string(),
        };
        let prompt = match suffix {
            Some(s) => format!("{base}\n\n{s}"),
            None => base,
        };
        self.spawner
            .spawn_one(SubAgentConfig {
                name: id.to_string(),
                prompt,
                max_turns: node_turn_budget(&plan.graph, id),
                max_tokens: node_token_budget(&plan.graph, id),
                system_prompt: None,
                provider: node_provider(&plan.graph, id),
                model: node_pinned_model(&plan.graph, id),
                temperature: None,
            })
            .await
    }

    /// Dispatch a sub-wave of `AgentCall` nodes. Returns `(node_id, result)`
    /// pairs, one per dispatched node. One node uses `spawn_one`; a fan-out of
    /// ≤ [`FLEET_FANOUT_THRESHOLD`] siblings uses the per-task relay path; a
    /// wider fan-out shards through `FleetDispatcher` (`spawn_via_fleet`).
    /// Results are correlated back to node ids by id, never by position.
    async fn dispatch_agents(
        &self,
        plan: &WorkflowPlan,
        agent_nodes: &[&str],
        state: &Value,
    ) -> Vec<(String, SubAgentResult)> {
        // Build a SubAgentConfig per node (prompt with resolved input injected).
        let mut configs: Vec<(String, SubAgentConfig)> = Vec::with_capacity(agent_nodes.len());
        for id in agent_nodes {
            // `agent_nodes` only holds ids the walk classified as `AgentCall`,
            // so the lookup is normally infallible. Skip defensively (rather
            // than panic) if an id is absent or not an `AgentCall`.
            let Some((_, Node::AgentCall { input_mapper, .. })) =
                plan.graph.nodes.iter().find(|(n, _)| n == id)
            else {
                continue;
            };
            let prompt = match plan.prompts.get(*id) {
                Some(p) => build_prompt(p, &input_mapper.apply(state)),
                // No authored prompt — fall back to the node id so the run
                // still progresses rather than panicking. Surfaced upstream
                // as MissingPrompt only if strictness is wanted later.
                None => (*id).to_string(),
            };
            configs.push((
                (*id).to_string(),
                SubAgentConfig {
                    name: (*id).to_string(),
                    prompt,
                    max_turns: node_turn_budget(&plan.graph, id),
                    max_tokens: node_token_budget(&plan.graph, id),
                    system_prompt: None,
                    provider: node_provider(&plan.graph, id),
                    model: node_pinned_model(&plan.graph, id),
                    temperature: None,
                },
            ));
        }

        if configs.len() == 1 {
            // ForgeFlows-Live Phase 1: when a parent sink is wired, route even
            // the single-node path through the relay so its sub-agent events
            // reach the parent. `dispatch_via_relay` handles N==1 as a fan-out
            // of one. When `None`, keep the legacy `spawn_one` (NullSink) path.
            if self.parent_output.is_some() {
                return self.dispatch_via_relay(configs).await;
            }
            let (id, cfg) = configs.into_iter().next().expect("len checked == 1");
            let result = self.spawner.spawn_one(cfg).await;
            return vec![(id, result)];
        }

        // Threshold-based dispatch. A wave wider than one shard
        // (`FLEET_FANOUT_THRESHOLD`) routes through `FleetDispatcher` sharding
        // so it scales to the fleet cap (100); a narrower wave keeps the
        // per-task relay path (each task gets its own `SpawnExtras`/`ChannelSink`
        // so the bridge sees one `SubAgentView` per sub-agent — the C1/F8 relay
        // fix). Both paths return `(node_id, result)` pairs the caller maps back
        // to nodes by id; neither relies on positional ordering.
        if configs.len() > FLEET_FANOUT_THRESHOLD {
            self.dispatch_agents_via_fleet(configs).await
        } else {
            self.dispatch_agents_via_relay(configs).await
        }
    }

    /// Per-task relay fan-out (≤ [`FLEET_FANOUT_THRESHOLD`] siblings): each task
    /// carries its own `SpawnExtras` so its `ChannelSink`/lifecycle wiring stays
    /// per-sub-agent. Results return in input order, so a positional zip back to
    /// the node ids is correct here.
    async fn dispatch_agents_via_relay(
        &self,
        configs: Vec<(String, SubAgentConfig)>,
    ) -> Vec<(String, SubAgentResult)> {
        // ForgeFlows-Live Phase 1: when a parent sink is wired, each task gets a
        // real `ChannelSink` so its events relay up as `SubAgentEvent`. Without
        // a parent, fall through to `SpawnExtras::default()` (NullSink) so the
        // legacy unmonitored path is byte-for-byte unchanged.
        if self.parent_output.is_some() {
            return self.dispatch_via_relay(configs).await;
        }
        let ids: Vec<String> = configs.iter().map(|(id, _)| id.clone()).collect();
        let tasks: Vec<(SubAgentConfig, SpawnExtras)> = configs
            .into_iter()
            .map(|(_, cfg)| (cfg, SpawnExtras::default()))
            .collect();
        let results = self
            .spawner
            .spawn_parallel_with_per_task_extras(tasks)
            .await;
        ids.into_iter().zip(results).collect()
    }

    /// ForgeFlows-Live Phase 1 — relay fan-out that mirrors
    /// [`crate::spawn_tool::SpawnTool::spawn_with_relay`]: build one shared
    /// stream-drain channel, a dedicated lifecycle channel per task, per-task
    /// [`SpawnExtras`] carrying a `ChannelSink` keyed by `workflow:<node_id>`,
    /// dispatch via `spawn_parallel_with_per_task_extras`, then flush the
    /// lifecycle receivers AFTER the stream drain (W5.5 H-1: terminal
    /// Done/Failed events survive a full stream channel). Results return in
    /// input order, so a positional zip back to the node ids is correct.
    ///
    /// PRECONDITION: only called when `self.parent_output.is_some()`.
    async fn dispatch_via_relay(
        &self,
        configs: Vec<(String, SubAgentConfig)>,
    ) -> Vec<(String, SubAgentResult)> {
        // SAFETY: guarded by every caller — only invoked when the parent sink
        // is wired (mirrors `SpawnTool::spawn_with_relay`'s precondition).
        let parent_output = Arc::clone(
            self.parent_output
                .as_ref()
                .expect("dispatch_via_relay precondition: parent_output is Some"),
        );

        let ids: Vec<String> = configs.iter().map(|(id, _)| id.clone()).collect();

        // One shared stream drain channel; each task's ChannelSink gets a
        // clone of tx for best-effort stream events.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<SubAgentRelay>(
            crate::agents::channel_sink::CHANNEL_CAPACITY,
        );

        // W5.5 H-1: one dedicated lifecycle channel per task. Collected as a Vec
        // of receivers so we can flush them after the stream drain.
        let mut lifecycle_rxs: Vec<tokio::sync::mpsc::Receiver<SubAgentRelay>> =
            Vec::with_capacity(configs.len());

        // Build per-task SpawnExtras with a distinct parent_call_id + ChannelSink.
        let tasks: Vec<(SubAgentConfig, SpawnExtras)> = configs
            .into_iter()
            .map(|(id, cfg)| {
                // Unique id per node — the bridge keys SubAgentView on this.
                let parent_call_id = format!("workflow:{id}");
                // W5.5 H-1: dedicated lifecycle channel (capacity 2, never shared).
                let (ltx, lrx) = tokio::sync::mpsc::channel::<SubAgentRelay>(
                    crate::agents::channel_sink::LIFECYCLE_CAPACITY,
                );
                lifecycle_rxs.push(lrx);
                let sink = Arc::new(ChannelSink::new_with_lifecycle(
                    parent_call_id.clone(),
                    cfg.name.clone(),
                    tx.clone(),
                    ltx,
                ));
                let extras = SpawnExtras {
                    channel_sink: Some(sink),
                    agent_name: Some(cfg.name.clone()),
                    parent_call_id: Some(parent_call_id),
                };
                (cfg, extras)
            })
            .collect();

        // Drop the original tx so the drain exits when all per-task senders drop.
        drop(tx);

        // Drain task: wrap each SubAgentRelay in a SubAgentEvent via the parent.
        let drain_output = Arc::clone(&parent_output);
        let drain = tokio::spawn(async move {
            while let Some(relay) = rx.recv().await {
                drain_output.emit_sub_agent_event(
                    &relay.parent_call_id,
                    &relay.agent_name,
                    &relay.inner,
                );
            }
        });

        let results = self
            .spawner
            .spawn_parallel_with_per_task_extras(tasks)
            .await;

        // Wait for the stream drain to flush all pending stream relays. Every
        // per-task ChannelSink has been dropped by now (all tasks completed), so
        // rx.recv() returns None and the drain task exits promptly.
        let _ = drain.await;

        // W5.5 H-1: flush lifecycle events AFTER the stream drain. The terminal
        // Done/Failed event for each task sits in its dedicated lifecycle channel.
        for mut lrx in lifecycle_rxs {
            while let Some(relay) = lrx.recv().await {
                parent_output.emit_sub_agent_event(
                    &relay.parent_call_id,
                    &relay.agent_name,
                    &relay.inner,
                );
            }
        }

        ids.into_iter().zip(results).collect()
    }

    /// Fleet-sharded fan-out (> [`FLEET_FANOUT_THRESHOLD`] siblings): route the
    /// wave through `AgentSpawner::spawn_via_fleet`, which shards into batches of
    /// `DEFAULT_SHARD_SIZE` with hierarchical reduction.
    ///
    /// **Result correlation.** `spawn_via_fleet` returns results in
    /// shard-then-within-shard order, NOT input order, so a positional zip would
    /// mislabel them. Each `SubAgentConfig.name` is set to the node id, and that
    /// `name` round-trips through the fleet payload into `SubAgentResult.name`,
    /// so we correlate by `result.name` back to the node id. Any result whose
    /// `name` is not a dispatched id (e.g. the synthetic `"fleet"` error result
    /// the spawner emits on a dispatch-level failure) is mapped onto every
    /// not-yet-matched node so the caller still surfaces the failure per node
    /// rather than silently dropping it.
    async fn dispatch_agents_via_fleet(
        &self,
        configs: Vec<(String, SubAgentConfig)>,
    ) -> Vec<(String, SubAgentResult)> {
        let ids: Vec<String> = configs.iter().map(|(id, _)| id.clone()).collect();
        // The fleet run_id seeds the `fleet:<run_id>-shard-<i>-<j>` parent_call_id
        // tag; a workflow-scoped label keeps it distinguishable on the bus.
        let run_id = format!("workflow-fanout-{}", ids.len());
        let sub_configs: Vec<SubAgentConfig> = configs.into_iter().map(|(_, cfg)| cfg).collect();
        let results = self.spawner.spawn_via_fleet(sub_configs, run_id).await;

        // Index results by name so each node id picks up its own result
        // regardless of shard ordering. A name can in principle repeat across
        // results only if two nodes shared an id, which the graph forbids.
        let mut by_name: HashMap<String, SubAgentResult> = HashMap::with_capacity(results.len());
        let mut unmatched: Vec<SubAgentResult> = Vec::new();
        for r in results {
            if ids.contains(&r.name) {
                by_name.insert(r.name.clone(), r);
            } else {
                // A fleet-level error result (name == "fleet") or any stray that
                // does not correspond to a dispatched id.
                unmatched.push(r);
            }
        }

        ids.into_iter()
            .map(|id| {
                let result = by_name.remove(&id).unwrap_or_else(|| {
                    // No per-node result: surface the first unmatched (typically
                    // the fleet dispatch-level error) so this node reports the
                    // failure instead of vanishing.
                    let text = unmatched
                        .first()
                        .map(|r| r.text.clone())
                        .unwrap_or_else(|| format!("fleet dispatch returned no result for `{id}`"));
                    SubAgentResult {
                        name: id.clone(),
                        text,
                        usage: wcore_types::message::TokenUsage::default(),
                        turns: 0,
                        is_error: true,
                    }
                });
                (id, result)
            })
            .collect()
    }

    /// A5 — run a no-barrier `Pipeline(over: ...)` step: resolve its `over`
    /// collection from the running state, stream every item through all stages
    /// with no barrier (via [`pipeline::run_pipeline`]), then write the
    /// order-preserving result array (with `null` holes for dropped items) onto
    /// `state[pipeline_id]`. A per-item stage failure drops only that item — the
    /// run as a whole continues. Returns `Err` only when a hard resource bound
    /// trips: the `over:` cardinality cap (FIX 3) or the dispatch budget (FIX 1).
    async fn run_no_barrier_pipeline(
        &self,
        plan: &WorkflowPlan,
        pipeline_id: &str,
        def: &PipelineDef,
        state: &mut Value,
        stage_results: &mut Vec<StageResult>,
        budget: &DispatchBudget,
    ) -> Result<(), WorkflowRunError> {
        // Resolve `over` against the running state. A non-array (or missing) ref
        // yields an empty item list — the pipeline runs zero items and writes an
        // empty array, never panicking.
        let resolved = InputMapper::Select {
            path: def.over.clone(),
        }
        .apply(state);
        let items: Vec<Value> = resolved.as_array().cloned().unwrap_or_default();

        // GAP-1/GAP-2: a zero-item fan must be VISIBLE, not a silent "completed
        // with nothing". Three distinct causes all dispatch zero sub-agents:
        //  - `over` resolves to `[]` (e.g. a clean git tree → empty changed_files),
        //  - `over` is missing (`Null`) — the synthesizer referenced a key nothing
        //    seeded/produced (e.g. `source_files` instead of `changed_files`),
        //  - `over` is present but not a list.
        // Record a stage explaining WHICH so the run output names the reason
        // instead of reporting an empty success. A missing/wrong-typed ref is a
        // malformed-plan signal (`is_error`); an empty array is a legitimate
        // no-work outcome.
        if items.is_empty() {
            let (reason, is_error) = match &resolved {
                Value::Array(_) => (
                    format!("`{}` was empty — nothing to fan over", def.over),
                    false,
                ),
                Value::Null => (
                    format!(
                        "`{}` was not found in the workflow state — the step fans over a \
                         key nothing produced",
                        def.over
                    ),
                    true,
                ),
                other => (
                    format!(
                        "`{}` is {}, not a list — nothing to fan over",
                        def.over,
                        json_value_kind(other)
                    ),
                    true,
                ),
            };
            stage_results.push(StageResult {
                node_id: pipeline_id.to_string(),
                text: format!("Pipeline `{pipeline_id}` ran 0 agents: {reason}."),
                is_error,
                turns: 0,
            });
            // Still write the empty array so downstream refs resolve to `[]`
            // rather than a missing key, matching the run_pipeline no-item path.
            if let Value::Object(m) = state {
                m.insert(pipeline_id.to_string(), Value::Array(Vec::new()));
            }
            return Ok(());
        }

        // FIX 3 — cardinality cap. The `over:` collection is resolved from the
        // running state, so it can be runtime-injected by the caller. Reject an
        // oversized collection BEFORE building one future per item (which would
        // otherwise allocate/poll unboundedly → OOM). Charge it to the dispatch
        // budget channel so the failure surfaces with the partial result.
        if items.len() > limits::MAX_OVER_CARDINALITY {
            return Err(WorkflowRunError::DispatchBudgetExceeded {
                limit: limits::MAX_OVER_CARDINALITY,
                attempted: items.len(),
                partial: Box::new(WorkflowRunResult {
                    final_state: state.clone(),
                    stage_results: stage_results.clone(),
                }),
            });
        }

        // No-barrier pipeline stages are NOT graph nodes (the step lowers to a
        // single placeholder node), so they have no entry in
        // `graph.node_budgets`; they run on the defaults. Per-stage overrides
        // here would need a `PipelineDef`/`PipelineStageDispatch` carry — out of
        // scope for the per-node (graph-dispatched AgentCall) budget override.
        let dispatch = PipelineStageDispatch {
            schema_defs: &plan.schema_defs,
            max_turns: DEFAULT_MAX_TURNS,
            max_tokens: DEFAULT_MAX_TOKENS,
            budget,
        };
        let outcome = pipeline::run_pipeline(
            self.spawner,
            &dispatch,
            pipeline_id,
            &items,
            &def.stages,
            Arc::clone(&self.pipeline_sem),
        )
        .await;

        if let Value::Object(m) = state {
            m.insert(pipeline_id.to_string(), Value::Array(outcome.items));
        }
        stage_results.extend(outcome.stage_results);

        // FIX 1 — if any pipeline dispatch tripped the budget, abort the run
        // with the partial result. The budget is shared across the whole run, so
        // checking it after the pipeline drains catches the breach regardless of
        // which item hit it.
        if let Some(attempted) = outcome.budget_breached {
            return Err(self.budget_exceeded(attempted, state, stage_results));
        }
        Ok(())
    }

    /// Fold an aggregator's inbound-sibling outputs into `state[agg_id]` per
    /// the per-key [`StateReducer`] override (when one exists for that key) or
    /// the node's [`AggregationStrategy`] default.
    fn apply_aggregator(
        &self,
        graph: &GraphConfig,
        agg_id: &str,
        strategy: &AggregationStrategy,
        state: &mut Value,
    ) {
        // Gather each inbound sibling's recorded output (a string we wrote to
        // `state[sibling_id]`).
        let inbound: Vec<Value> = graph
            .edges
            .iter()
            .filter(|e| e.to == agg_id)
            .filter_map(|e| state.get(&e.from).cloned())
            .collect();

        let folded = match graph.state_reducers.get(agg_id) {
            Some(StateReducer::SumNumbers) => {
                let sum: f64 = inbound.iter().filter_map(|v| v.as_f64()).sum();
                serde_json::Number::from_f64(sum)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
            Some(StateReducer::Collect) => Value::Array(inbound),
            Some(StateReducer::Replace) => inbound.into_iter().last().unwrap_or(Value::Null),
            None => match strategy {
                // FIX C — v1 fold: both `ConcatOutputs` (from `JoinStrategy::Concat`)
                // and `MergeObjects` (from `JoinStrategy::Merge`/`Collect`) currently
                // collect siblings into an array, identical to the `Collect` reducer.
                // True field-concat / deep-merge reducers are NOT yet wired; the DSL
                // documents these as array-fold aliases in v1, so this array result is
                // the honest, documented behaviour (no surprising silent semantics).
                AggregationStrategy::ConcatOutputs | AggregationStrategy::MergeObjects => {
                    Value::Array(inbound)
                }
                AggregationStrategy::First => inbound
                    .into_iter()
                    .find(|v| !v.is_null())
                    .unwrap_or(Value::Null),
                AggregationStrategy::Last => inbound.into_iter().last().unwrap_or(Value::Null),
            },
        };

        if let Value::Object(m) = state {
            m.insert(agg_id.to_string(), folded);
        }
    }

    /// Minimal bounded loop: run the per-iteration agent sequence
    /// sequentially up to `cap`, stopping early when `done_check` holds.
    #[allow(clippy::too_many_arguments)]
    async fn run_loop(
        &self,
        plan: &WorkflowPlan,
        loop_id: &str,
        agents: &[(String, InputMapper)],
        done_check: &super::super::graph::Predicate,
        cap: usize,
        state: &mut Value,
        stage_results: &mut Vec<StageResult>,
        budget: &DispatchBudget,
    ) -> Result<(), WorkflowRunError> {
        for _ in 0..cap {
            for (agent_name, mapper) in agents {
                // FIX 1 — every loop-iteration agent dispatch is charged so a
                // loop (even within LOOP_ITER_CAP) cannot escape the run budget.
                if let Err(attempted) = budget.try_charge() {
                    return Err(self.budget_exceeded(attempted, state, stage_results));
                }
                let prompt = build_prompt(
                    plan.prompts
                        .get(agent_name)
                        .map(String::as_str)
                        .unwrap_or(agent_name),
                    &mapper.apply(state),
                );
                let cfg = SubAgentConfig {
                    name: agent_name.clone(),
                    prompt,
                    max_turns: node_turn_budget(&plan.graph, agent_name),
                    max_tokens: node_token_budget(&plan.graph, agent_name),
                    system_prompt: None,
                    provider: node_provider(&plan.graph, agent_name),
                    model: node_pinned_model(&plan.graph, agent_name),
                    temperature: None,
                };
                let result = self.spawner.spawn_one(cfg).await;
                if result.is_error {
                    return Err(self.fail(
                        &format!("{loop_id}:{agent_name}"),
                        result.text,
                        state,
                        stage_results,
                    ));
                }
                if let Value::Object(m) = state {
                    m.insert(agent_name.clone(), Value::String(result.text.clone()));
                }
                stage_results.push(StageResult {
                    node_id: format!("{loop_id}:{agent_name}"),
                    text: result.text,
                    is_error: false,
                    turns: result.turns,
                });
            }
            if done_check.evaluate(state) {
                break;
            }
        }
        Ok(())
    }

    /// Build a [`WorkflowRunError::StageFailed`] carrying the partial result.
    fn fail(
        &self,
        stage: &str,
        message: String,
        state: &Value,
        stage_results: &[StageResult],
    ) -> WorkflowRunError {
        WorkflowRunError::StageFailed {
            stage: stage.to_string(),
            message,
            partial: Box::new(WorkflowRunResult {
                final_state: state.clone(),
                stage_results: stage_results.to_vec(),
            }),
        }
    }

    /// Backfill a deferred wave error's `partial` with the fully-committed
    /// wave (FIX A). The two-pass wave loop builds the typed error eagerly (so
    /// it can keep the first failure's `stage`/`attempts`/`message`) with an
    /// empty placeholder partial, then calls this once the whole wave has
    /// committed so the partial carries state + every successful sibling.
    fn with_partial(
        &self,
        err: WorkflowRunError,
        state: &Value,
        stage_results: &[StageResult],
    ) -> WorkflowRunError {
        let fresh = || {
            Box::new(WorkflowRunResult {
                final_state: state.clone(),
                stage_results: stage_results.to_vec(),
            })
        };
        match err {
            WorkflowRunError::StageFailed { stage, message, .. } => WorkflowRunError::StageFailed {
                stage,
                message,
                partial: fresh(),
            },
            WorkflowRunError::SchemaValidationFailed {
                stage,
                attempts,
                message,
                ..
            } => WorkflowRunError::SchemaValidationFailed {
                stage,
                attempts,
                message,
                partial: fresh(),
            },
            WorkflowRunError::DispatchBudgetExceeded {
                limit, attempted, ..
            } => WorkflowRunError::DispatchBudgetExceeded {
                limit,
                attempted,
                partial: fresh(),
            },
            // Non-partial-bearing variants (e.g. NodeNotInGraph) pass through.
            other => other,
        }
    }

    /// Build a [`WorkflowRunError::DispatchBudgetExceeded`] carrying the partial
    /// result. `attempted` is the dispatch count the charge would have reached.
    fn budget_exceeded(
        &self,
        attempted: usize,
        state: &Value,
        stage_results: &[StageResult],
    ) -> WorkflowRunError {
        WorkflowRunError::DispatchBudgetExceeded {
            limit: limits::MAX_TOTAL_DISPATCHES,
            attempted,
            partial: Box::new(WorkflowRunResult {
                final_state: state.clone(),
                stage_results: stage_results.to_vec(),
            }),
        }
    }
}

/// Resolve a node id reference back to the `&str` key stored in `node_map`,
/// so `done` (a `HashSet<&str>` borrowing the graph) can hold it.
///
/// Normally infallible — `id` always originates from a node the walk just
/// dispatched, so it is present in `node_map`. Returns
/// [`WorkflowRunError::NodeNotInGraph`] rather than panicking if that
/// invariant is ever violated.
/// A human label for a JSON value's kind, for the zero-item `over` diagnostic
/// (GAP-1/GAP-2). Phrased to read inline: "`source_files` is an object, not a
/// list".
fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "an object",
        Value::String(_) => "a string",
        Value::Number(_) => "a number",
        Value::Bool(_) => "a boolean",
        Value::Null => "null",
        Value::Array(_) => "a list",
    }
}

fn node_id_ref<'m>(
    node_map: &HashMap<&'m str, &Node>,
    id: &str,
) -> Result<&'m str, WorkflowRunError> {
    node_map
        .keys()
        .find(|k| **k == id)
        .copied()
        .ok_or_else(|| WorkflowRunError::NodeNotInGraph(id.to_string()))
}

/// Compose a sub-agent prompt from the authored template and the resolved
/// input. The input is appended as a clearly-delimited context block when it
/// is non-null; a null input leaves the prompt unchanged.
pub(crate) fn build_prompt(template: &str, input: &Value) -> String {
    if input.is_null() {
        return template.to_string();
    }
    let rendered = match input {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    format!("{template}\n\n--- input ---\n{rendered}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_appends_non_null_input() {
        let p = build_prompt("do it", &Value::String("ctx".into()));
        assert!(p.contains("do it"));
        assert!(p.contains("ctx"));
    }

    #[test]
    fn build_prompt_leaves_null_input_unchanged() {
        assert_eq!(build_prompt("do it", &Value::Null), "do it");
    }

    #[test]
    fn plan_parse_extracts_prompts_and_schemas() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    schemas: { "findings": "{ \"type\": \"object\" }" },
    phases: [Phase(title: "p", steps: [
        Agent((id: "scan", prompt: "scan it")),
        Pipeline(id: "pl", stages: [
            (id: "lint", prompt: "lint it"),
            (id: "verify", prompt: "verify it", schema: Some("findings"), input: Some("lint")),
        ]),
    ])],
)
"#;
        let plan = WorkflowPlan::parse(src).expect("should parse");
        assert_eq!(
            plan.prompts.get("scan").map(String::as_str),
            Some("scan it")
        );
        assert_eq!(
            plan.prompts.get("verify").map(String::as_str),
            Some("verify it")
        );
        assert_eq!(
            plan.schemas.get("verify").map(String::as_str),
            Some("findings")
        );
        // Non-schema nodes do not appear in the schema table.
        assert!(!plan.schemas.contains_key("lint"));
        // The named schema body was compiled into the def table.
        assert!(plan.schema_defs.contains_key("findings"));
    }

    #[test]
    fn node_budget_resolution_uses_override_then_falls_back_to_defaults() {
        // `big` overrides both dimensions, `wide` overrides only turns, `plain`
        // overrides nothing. These are exactly the values the 4 SubAgentConfig
        // construction sites feed into `max_turns`/`max_tokens`.
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "big", prompt: "lots", max_turns: Some(40), max_tokens: Some(16000))),
        Agent((id: "wide", prompt: "wide", max_turns: Some(20))),
        Agent((id: "plain", prompt: "default")),
    ])],
)
"#;
        let plan = WorkflowPlan::parse(src).expect("should parse");
        let g = &plan.graph;

        // A node that set both: both overrides flow through.
        assert_eq!(node_turn_budget(g, "big"), 40);
        assert_eq!(node_token_budget(g, "big"), 16000);

        // A node that set only turns: turns overridden, tokens fall back.
        assert_eq!(node_turn_budget(g, "wide"), 20);
        assert_eq!(node_token_budget(g, "wide"), DEFAULT_MAX_TOKENS);

        // A node with no override: both fall back to the runner defaults
        // (the silent-truncation regression this fix closes).
        assert_eq!(node_turn_budget(g, "plain"), DEFAULT_MAX_TURNS);
        assert_eq!(node_token_budget(g, "plain"), DEFAULT_MAX_TOKENS);

        // An id with no node at all also falls back rather than panicking.
        assert_eq!(node_turn_budget(g, "absent"), DEFAULT_MAX_TURNS);
        assert_eq!(node_token_budget(g, "absent"), DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn plan_parse_rejects_malformed_schema_body() {
        // `findings` body has an unsupported `type`, so the plan must fail to
        // build with a typed InvalidSchema error rather than at run time.
        let src = r#"
Workflow(
    meta: (name: "x"),
    schemas: { "findings": "{ \"type\": \"tuple\" }" },
    phases: [Phase(title: "p", steps: [
        Agent((id: "scan", prompt: "scan", schema: Some("findings"))),
    ])],
)
"#;
        // `WorkflowPlan` carries a non-`Debug` `GraphConfig`, so match the Ok
        // side without formatting it.
        match WorkflowPlan::parse(src) {
            Ok(_) => panic!("expected InvalidSchema error, got Ok"),
            Err(WorkflowParseError::InvalidSchema { name, .. }) => assert_eq!(name, "findings"),
            Err(other) => panic!("expected InvalidSchema, got {other:?}"),
        }
    }
}
