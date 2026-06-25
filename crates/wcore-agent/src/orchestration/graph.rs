//! W8b.2.B Task C.1 — `ExecutionGraph` core.
//!
//! A small, self-contained directed-graph executor. The graph is
//! intentionally additive: it lives alongside the existing per-turn
//! orchestration in [`super`] and is wired into the main loop only via
//! the `Direct` template in Task C.5.
//!
//! ## Departure from the literal plan
//!
//! The plan's sketch references `AgentRegistry` and `AgentBus` (W7 F2
//! surfaces). Threading those types into `GraphContext` would couple
//! the graph crate to the agent runtime and explode the dependency
//! surface — well beyond what this sub-wave needs. Instead we accept a
//! [`NodeExecutor`] trait. Production wires it to the existing agent
//! dispatch path (Task C.5 / W8b.2.B.1); tests stub it with a scripted
//! handler map. Same shape, smaller blast radius.
//!
//! ## What the walker does
//!
//! 1. Pull all nodes that have at least one inbound edge satisfied (or
//!    are the entry node).
//! 2. Run every ready node concurrently via `futures::future::join_all`.
//! 3. Merge each node's output into a shared `Value` state map using
//!    either the per-key [`StateReducer`] override or
//!    `AggregationStrategy`-aware default merge.
//! 4. Check [`CancellationToken`] before each tick.
//! 5. Stop when no further node is reachable.
//!
//! Cycles are NOT detected ad-hoc. The `IterativeLoop` template
//! (Task C.2) bounds iteration explicitly through a `max_iters` counter
//! encoded in state; the walker treats it like any other state-gated
//! edge.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// A node identifier in the graph. We use `String` for ergonomic
/// construction; the graph is small (≤ tens of nodes typically) so the
/// allocation cost is negligible next to per-agent latency.
pub type NodeId = String;

/// How a node transforms the inbound state before passing it to the
/// agent executor (or before evaluating a predicate).
#[derive(Debug, Clone)]
pub enum InputMapper {
    /// Pass the entire state object through unchanged.
    PassThrough,
    /// Pull a subfield by a dotted path. A single segment (no `.`)
    /// resolves as a flat top-level key — identical to the original
    /// behavior. A dotted path (`"review.findings"`, `"stage1.0.id"`)
    /// descends object keys and array indices. A path that fails to
    /// resolve yields `Value::Null`, matching the original missing-key
    /// contract (never panics, never errors — see [`InputMapper::apply`]).
    Select { path: String },
    /// Inject a literal JSON object as the input, ignoring state.
    Literal { value: Value },
}

impl InputMapper {
    pub fn apply(&self, state: &Value) -> Value {
        match self {
            InputMapper::PassThrough => state.clone(),
            InputMapper::Select { path } => resolve_dotted_path(state, path)
                .cloned()
                .unwrap_or(Value::Null),
            InputMapper::Literal { value } => value.clone(),
        }
    }
}

/// Resolve a dotted path against `state`, descending object keys and
/// array indices one segment at a time.
///
/// Contract:
/// - A single segment (no `.`) is a flat top-level key lookup — byte-for-byte
///   the original `state.get(path)` behavior, so existing flat-key callers and
///   the hot-path walker do not regress. (A flat key that itself contains a `.`
///   is no longer reachable as one literal key — that was never a supported
///   shape here; all existing keys are dot-free identifiers.)
/// - Each subsequent segment descends: object keys via `Value::get(&str)`,
///   array indices via `Value::get(usize)` when the segment parses as a
///   `usize`.
/// - Any segment that fails to resolve returns `None`. The caller maps that to
///   `Value::Null`, preserving the original missing-key semantics
///   (`state.get(path).cloned().unwrap_or(Value::Null)`) — never panics, never
///   errors. We deliberately keep `None`/`Null` rather than introducing a typed
///   error so flat-key callers stay unaffected.
fn resolve_dotted_path<'a>(state: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = state;
    for segment in path.split('.') {
        cursor = match cursor {
            Value::Array(_) => {
                let index: usize = segment.parse().ok()?;
                cursor.get(index)?
            }
            _ => cursor.get(segment)?,
        };
    }
    Some(cursor)
}

/// How sibling outputs are folded into the merged state at an
/// `Aggregator` node.
#[derive(Debug, Clone)]
pub enum AggregationStrategy {
    /// Each input becomes one element of a new array under the
    /// `output` key. Order is undefined (parallel siblings).
    ConcatOutputs,
    /// Deep-merge every input object into the running state, with
    /// later writes winning conflicts. Useful for fan-in of distinct
    /// keys.
    MergeObjects,
    /// Take the first non-null input.
    First,
    /// Take the last input.
    Last,
}

/// A predicate evaluated against the running state, used both by
/// `Node::Predicate` and by `EdgeCondition::Predicate`.
#[derive(Debug, Clone)]
pub enum Predicate {
    /// True when `state[path]` is present (and not Null).
    StateContainsKey { path: String },
    /// True when `state[path] == value`.
    StateEquals { path: String, value: Value },
    /// True when `state[path]` is a number greater than `threshold`.
    StateGt { path: String, threshold: f64 },
    /// Always true.
    Always,
    /// Always false.
    Never,
}

impl Predicate {
    pub fn evaluate(&self, state: &Value) -> bool {
        match self {
            Predicate::StateContainsKey { path } => {
                state.get(path).map(|v| !v.is_null()).unwrap_or(false)
            }
            Predicate::StateEquals { path, value } => state.get(path) == Some(value),
            Predicate::StateGt { path, threshold } => state
                .get(path)
                .and_then(|v| v.as_f64())
                .map(|v| v > *threshold)
                .unwrap_or(false),
            Predicate::Always => true,
            Predicate::Never => false,
        }
    }
}

/// Edge guard: when present, the edge is taken only if `evaluate`
/// returns true against the current state.
#[derive(Debug, Clone)]
pub enum EdgeCondition {
    /// Wraps a [`Predicate`].
    Predicate(Predicate),
    /// Direct equality check (sugar for `Predicate::StateEquals`).
    StateEquals { path: String, value: Value },
}

impl EdgeCondition {
    pub fn evaluate(&self, state: &Value) -> bool {
        match self {
            EdgeCondition::Predicate(p) => p.evaluate(state),
            EdgeCondition::StateEquals { path, value } => state.get(path) == Some(value),
        }
    }
}

/// Per-key override for how an aggregator merges this state field.
/// Default merge (none specified) is "last writer wins" for scalars
/// and `MergeObjects` for objects.
#[derive(Debug, Clone)]
pub enum StateReducer {
    /// Replace with the latest value seen.
    Replace,
    /// Sum numeric values across all sibling outputs.
    SumNumbers,
    /// Collect each value into an array.
    Collect,
}

/// A graph node. Each variant has a different runtime semantic.
#[derive(Clone)]
pub enum Node {
    /// Call an agent (or any [`NodeExecutor`] handler) by name.
    AgentCall {
        agent: String,
        input_mapper: InputMapper,
    },
    /// Evaluate a [`Predicate`] and record the boolean to
    /// `state["predicate_result"]` for downstream edges.
    Predicate { condition: Predicate },
    /// Fan-in: merge all inbound siblings' state writes per `strategy`.
    Aggregator { strategy: AggregationStrategy },
    /// Pure pass-through (used as a synthetic fanout root).
    PassThrough,
    /// Bounded loop: per iteration call every `agents` entry in order
    /// (sharing state mutations across calls in the same iteration),
    /// then check `done_check`. Stops on done OR after `max_iters`
    /// iterations. Used by `iterative_loop()` (1 agent per iter) and
    /// `self_critique()` (doer→critic per iter).
    Loop {
        agents: Vec<(String, InputMapper)>,
        done_check: Predicate,
        max_iters: usize,
    },
    /// Terminal sink (optional; not strictly required since the walker
    /// stops on empty frontier).
    End,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub when: Option<EdgeCondition>,
}

/// Per-node turn/token budget overrides. Either field `None` means "use the
/// runner's `DEFAULT_MAX_TURNS`/`DEFAULT_MAX_TOKENS` for that dimension". Lives
/// in [`GraphConfig::node_budgets`] as a side-table (keyed by node id) so the
/// budget can be authored per workflow node without widening the [`Node`] enum
/// or touching every `Node::AgentCall` construction/match site.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeBudget {
    /// Max agent turns for this node; `None` → runner default.
    pub max_turns: Option<u32>,
    /// Max output tokens for this node; `None` → runner default.
    pub max_tokens: Option<u32>,
}

/// A per-node provider pin, stored in [`GraphConfig::node_providers`] as a
/// side-table (keyed by node id) so a workflow node can target a specific LLM
/// provider/model without widening the [`Node`] enum. `provider` is
/// `Option<String>` so a pin can carry a *model-only* override (no provider
/// change). Both fields `None` ⇒ inherit the spawner's provider/model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderPin {
    /// Pin to a named provider (resolved by `CouncilProviderResolver`); `None`
    /// ⇒ inherit the spawner's provider.
    pub provider: Option<String>,
    /// Optional model override; `None` ⇒ inherit the resolved provider's
    /// default model.
    pub model: Option<String>,
}

/// A buildable graph spec: nodes by id + directed edges + a single
/// entry. Per-key state reducer overrides live alongside.
pub struct GraphConfig {
    pub nodes: Vec<(NodeId, Node)>,
    pub edges: Vec<Edge>,
    pub entry: NodeId,
    pub state_reducers: HashMap<String, StateReducer>,
    /// Per-node turn/token budget overrides, keyed by node id. A node absent
    /// here (or present with a `None` field) falls back to the runner's
    /// `DEFAULT_MAX_TURNS`/`DEFAULT_MAX_TOKENS`. The `ExecutionGraph` walker
    /// ignores this table (it dispatches through the `NodeExecutor` trait, not
    /// `SubAgentConfig`); only `WorkflowRunner` reads it.
    pub node_budgets: HashMap<String, NodeBudget>,
    /// Per-node provider/model pins, keyed by node id. A node absent here
    /// inherits the spawner's provider/model. Like [`Self::node_budgets`], the
    /// `ExecutionGraph` walker ignores this table (it dispatches through the
    /// `NodeExecutor` trait, not `SubAgentConfig`); only `WorkflowRunner` reads
    /// it to set `SubAgentConfig.{provider,model}`.
    pub node_providers: HashMap<String, ProviderPin>,
}

impl GraphConfig {
    /// Empty graph with a declared entry node (you still have to add
    /// the entry node before [`ExecutionGraph::execute`] resolves it).
    pub fn empty(entry: impl Into<NodeId>) -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            entry: entry.into(),
            state_reducers: HashMap::new(),
            node_budgets: HashMap::new(),
            node_providers: HashMap::new(),
        }
    }

    /// Single-agent direct graph. Convenience for the Direct template.
    pub fn single_node(agent: impl Into<String>, input_mapper: InputMapper) -> Self {
        let agent = agent.into();
        let mut g = Self::empty(agent.clone());
        g.add_agent(&agent, input_mapper);
        g
    }

    pub fn add_agent(&mut self, agent: impl Into<NodeId>, input_mapper: InputMapper) {
        let id: NodeId = agent.into();
        let name = id.clone();
        self.nodes.push((
            id,
            Node::AgentCall {
                agent: name,
                input_mapper,
            },
        ));
    }

    pub fn add_predicate(&mut self, id: impl Into<NodeId>, condition: Predicate) {
        self.nodes.push((id.into(), Node::Predicate { condition }));
    }

    pub fn add_aggregator(&mut self, id: impl Into<NodeId>, strategy: AggregationStrategy) {
        self.nodes.push((id.into(), Node::Aggregator { strategy }));
    }

    pub fn add_passthrough(&mut self, id: impl Into<NodeId>) {
        self.nodes.push((id.into(), Node::PassThrough));
    }

    pub fn add_end(&mut self, id: impl Into<NodeId>) {
        self.nodes.push((id.into(), Node::End));
    }

    /// Add a bounded `Loop` node. `agents` is the per-iteration call
    /// sequence: each entry is `(agent_name, input_mapper)`. The loop
    /// stops when `done_check` evaluates true OR `max_iters` is hit.
    pub fn add_loop(
        &mut self,
        id: impl Into<NodeId>,
        agents: Vec<(String, InputMapper)>,
        done_check: Predicate,
        max_iters: usize,
    ) {
        self.nodes.push((
            id.into(),
            Node::Loop {
                agents,
                done_check,
                max_iters,
            },
        ));
    }

    pub fn add_edge(
        &mut self,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        when: Option<EdgeCondition>,
    ) {
        self.edges.push(Edge {
            from: from.into(),
            to: to.into(),
            when,
        });
    }

    /// Set the provider/model pin for a node id, inserting into the
    /// [`Self::node_providers`] side-table. Mirrors how budgets are recorded.
    /// `WorkflowRunner` reads this pin to set `SubAgentConfig.{provider,model}`
    /// when dispatching the node.
    pub fn set_node_provider(&mut self, id: &str, pin: ProviderPin) {
        self.node_providers.insert(id.to_string(), pin);
    }

    /// Inspector — true if this config matches the shape produced by
    /// `direct()`. Templates use this in tests.
    pub fn is_direct(&self) -> bool {
        self.nodes.len() == 1
            && self.edges.is_empty()
            && matches!(self.nodes[0].1, Node::AgentCall { .. })
    }

    /// Inspector — true when the graph fans out from the entry node
    /// to ≥2 concurrent AgentCalls.
    pub fn is_parallel_fanout(&self) -> bool {
        let entry_out: Vec<_> = self.edges.iter().filter(|e| e.from == self.entry).collect();
        entry_out.len() >= 2
            && entry_out.iter().all(|e| {
                self.nodes
                    .iter()
                    .find(|(id, _)| id == &e.to)
                    .map(|(_, n)| matches!(n, Node::AgentCall { .. }))
                    .unwrap_or(false)
            })
    }
}

/// Pluggable agent runner. Production wires this to the existing
/// per-agent dispatch path; tests stub it with a scripted handler.
#[async_trait]
pub trait NodeExecutor: Send + Sync {
    async fn run_agent(&self, agent: &str, input: &Value) -> Result<Value, String>;
}

/// Execution context. Cheap to build per `execute` call.
pub struct GraphContext {
    pub cancel: CancellationToken,
    pub executor: Arc<dyn NodeExecutor>,
}

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("execution cancelled")]
    Cancelled,
    #[error("unknown entry node `{0}`")]
    UnknownEntry(String),
    #[error("edge to unknown node `{0}`")]
    UnknownTarget(String),
    #[error("agent `{agent}` failed: {message}")]
    AgentFailed { agent: String, message: String },
}

/// Result of a graph run: the final merged state.
#[derive(Debug, Clone)]
pub struct GraphResult {
    pub final_state: Value,
}

pub struct ExecutionGraph;

impl ExecutionGraph {
    /// Walk the graph from `config.entry`, returning the final merged
    /// state. Cancellation is checked at every tick; cancelled
    /// executions short-circuit with [`GraphError::Cancelled`].
    pub async fn execute(
        config: GraphConfig,
        initial: Value,
        ctx: GraphContext,
    ) -> Result<GraphResult, GraphError> {
        let GraphConfig {
            nodes,
            edges,
            entry,
            state_reducers,
            // The walker dispatches via `NodeExecutor`, not `SubAgentConfig`,
            // so per-node turn/token budgets and provider pins do not apply
            // here (only `WorkflowRunner` consumes them).
            node_budgets: _,
            node_providers: _,
        } = config;

        // Validate entry up-front for clean error messaging.
        let node_map: HashMap<NodeId, Node> = nodes.into_iter().collect();
        if !node_map.contains_key(&entry) {
            return Err(GraphError::UnknownEntry(entry));
        }

        let mut state = initial;
        let mut frontier: BTreeSet<NodeId> = BTreeSet::new();
        frontier.insert(entry);
        let mut visited: BTreeSet<NodeId> = BTreeSet::new();

        while !frontier.is_empty() {
            if ctx.cancel.is_cancelled() {
                return Err(GraphError::Cancelled);
            }

            // Resolve every node in the frontier; bail if any id is
            // unknown (would only happen with a hand-rolled bad config).
            let mut frontier_nodes: Vec<(NodeId, Node)> = Vec::with_capacity(frontier.len());
            for id in frontier.iter() {
                let node = node_map
                    .get(id)
                    .cloned()
                    .ok_or_else(|| GraphError::UnknownTarget(id.clone()))?;
                frontier_nodes.push((id.clone(), node));
            }

            // Aggregators only fire when ALL their inbound siblings have
            // completed; defer those to a later tick.
            let mut runnable_now: Vec<(NodeId, Node)> = Vec::with_capacity(frontier_nodes.len());
            let mut deferred: BTreeSet<NodeId> = BTreeSet::new();
            for (id, node) in frontier_nodes {
                if let Node::Aggregator { .. } = &node {
                    let inbound_pending = edges
                        .iter()
                        .filter(|e| e.to == id)
                        .any(|e| !visited.contains(&e.from) && !frontier.contains(&e.from));
                    let inbound_in_frontier = edges
                        .iter()
                        .filter(|e| e.to == id)
                        .any(|e| frontier.contains(&e.from) && e.from != id);
                    if inbound_pending || inbound_in_frontier {
                        deferred.insert(id);
                        continue;
                    }
                }
                runnable_now.push((id, node));
            }

            // Outputs collected this tick, in arbitrary order.
            let mut tick_outputs: Vec<(NodeId, Value)> = Vec::with_capacity(runnable_now.len());

            // 1) Synchronous nodes resolve immediately.
            // 2) AgentCalls collect into a futures vec for join_all.
            // 3) Loop nodes drive their inner sequence sequentially
            //    (since each iteration depends on the previous one's
            //    state mutations).
            let mut agent_futures = Vec::new();
            for (id, node) in &runnable_now {
                match node {
                    Node::AgentCall {
                        agent,
                        input_mapper,
                    } => {
                        let input = input_mapper.apply(&state);
                        let cancel = ctx.cancel.clone();
                        let executor = ctx.executor.clone();
                        let agent_name = agent.clone();
                        let id_clone = id.clone();
                        // Spawn each AgentCall on its own task so blocking
                        // work inside `run_agent` doesn't serialise siblings.
                        agent_futures.push(tokio::spawn(async move {
                            let res = tokio::select! {
                                _ = cancel.cancelled() => Err("cancelled".to_string()),
                                r = executor.run_agent(&agent_name, &input) => r,
                            };
                            (id_clone, agent_name, res)
                        }));
                    }
                    Node::Predicate { condition } => {
                        let mut out = serde_json::Map::new();
                        out.insert(
                            "predicate_result".to_string(),
                            Value::Bool(condition.evaluate(&state)),
                        );
                        tick_outputs.push((id.clone(), Value::Object(out)));
                    }
                    Node::Loop {
                        agents,
                        done_check,
                        max_iters,
                    } => {
                        let mut iter = 0usize;
                        while iter < *max_iters {
                            if ctx.cancel.is_cancelled() {
                                return Err(GraphError::Cancelled);
                            }
                            for (agent_name, mapper) in agents {
                                let input = mapper.apply(&state);
                                let res = tokio::select! {
                                    _ = ctx.cancel.cancelled() => {
                                        return Err(GraphError::Cancelled);
                                    }
                                    r = ctx.executor.run_agent(agent_name, &input) => r,
                                };
                                let v = res.map_err(|e| GraphError::AgentFailed {
                                    agent: agent_name.clone(),
                                    message: e,
                                })?;
                                merge_into_state(&mut state, &v, &state_reducers);
                            }
                            iter += 1;
                            if done_check.evaluate(&state) {
                                break;
                            }
                        }
                        tick_outputs.push((id.clone(), Value::Null));
                    }
                    Node::Aggregator { .. } | Node::PassThrough | Node::End => {
                        tick_outputs.push((id.clone(), Value::Null));
                    }
                }
            }
            let agent_results = futures::future::join_all(agent_futures).await;
            for join_res in agent_results {
                let (id, agent, res) = join_res.map_err(|e| GraphError::AgentFailed {
                    agent: "<join>".to_string(),
                    message: e.to_string(),
                })?;
                match res {
                    Ok(v) => tick_outputs.push((id, v)),
                    Err(e) if e == "cancelled" => return Err(GraphError::Cancelled),
                    Err(e) => {
                        return Err(GraphError::AgentFailed { agent, message: e });
                    }
                }
            }

            // Merge tick outputs into running state. Group by destination
            // aggregator so we can apply per-strategy folding when
            // multiple siblings flow into an aggregator node simultaneously.
            // For non-aggregator targets we just do scalar replacement.
            //
            // The simplest correct merge: write each output into state,
            // honoring per-key StateReducer overrides where present.
            let mut grouped_by_aggregator: HashMap<NodeId, Vec<Value>> = HashMap::new();
            let mut visited_this_tick: BTreeSet<NodeId> = BTreeSet::new();
            for (id, value) in &tick_outputs {
                visited_this_tick.insert(id.clone());
                // Find every aggregator child this node feeds.
                for e in edges.iter().filter(|e| &e.from == id) {
                    if let Some(Node::Aggregator { .. }) = node_map.get(&e.to) {
                        grouped_by_aggregator
                            .entry(e.to.clone())
                            .or_default()
                            .push(value.clone());
                    }
                }
                // Always also merge directly into state so downstream
                // edges (non-aggregator) see the latest values.
                merge_into_state(&mut state, value, &state_reducers);
            }

            // Apply aggregator strategies (deferred aggregators below
            // will see their inputs flow through the next tick).
            // NOTE: per-key state reducers (`SumNumbers`, `Collect`)
            // already fired inside `merge_into_state` above. Aggregator
            // strategies here are an *additional* shape transform
            // (e.g. `ConcatOutputs` builds the array under
            // `output`). They must NOT clobber values for which a
            // reducer ran.
            for (agg_id, values) in grouped_by_aggregator {
                if let Some(Node::Aggregator { strategy }) = node_map.get(&agg_id) {
                    apply_aggregation(&mut state, strategy, &values, &state_reducers);
                }
            }

            // Advance frontier: every node reachable via at least one
            // satisfied edge from the just-visited set, plus any
            // deferred aggregators whose inputs are now visited.
            for id in &visited_this_tick {
                visited.insert(id.clone());
            }
            let mut next: BTreeSet<NodeId> = BTreeSet::new();
            for e in &edges {
                if !visited_this_tick.contains(&e.from) && !deferred.contains(&e.from) {
                    continue;
                }
                let target_ready = e.when.as_ref().map(|c| c.evaluate(&state)).unwrap_or(true);
                if !target_ready {
                    continue;
                }
                if !node_map.contains_key(&e.to) {
                    return Err(GraphError::UnknownTarget(e.to.clone()));
                }
                if visited.contains(&e.to) {
                    continue;
                }
                next.insert(e.to.clone());
            }
            // Re-add deferred aggregators whose inbound siblings are
            // all visited now (allows them to fire next tick).
            for id in &deferred {
                let all_in_visited = edges
                    .iter()
                    .filter(|e| &e.to == id)
                    .all(|e| visited.contains(&e.from));
                if all_in_visited {
                    next.insert(id.clone());
                    visited.remove(id);
                }
            }
            frontier = next;
        }

        Ok(GraphResult { final_state: state })
    }
}

fn merge_into_state(state: &mut Value, value: &Value, reducers: &HashMap<String, StateReducer>) {
    let Some(obj) = value.as_object() else {
        return;
    };
    let state_obj = match state {
        Value::Object(m) => m,
        _ => {
            *state = Value::Object(serde_json::Map::new());
            // SAFETY: the immediately-preceding line wrote a
            // `Value::Object(...)` into `*state`; `as_object_mut`
            // therefore cannot return None.
            state.as_object_mut().unwrap()
        }
    };
    for (k, v) in obj {
        match reducers.get(k) {
            Some(StateReducer::SumNumbers) => {
                let existing = state_obj.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
                let added = v.as_f64().unwrap_or(0.0);
                state_obj.insert(
                    k.clone(),
                    serde_json::Number::from_f64(existing + added)
                        .map(Value::Number)
                        .unwrap_or(Value::Null),
                );
            }
            Some(StateReducer::Collect) => {
                let entry = state_obj
                    .entry(k.clone())
                    .or_insert_with(|| Value::Array(vec![]));
                if let Value::Array(a) = entry {
                    a.push(v.clone());
                } else {
                    *entry = Value::Array(vec![v.clone()]);
                }
            }
            Some(StateReducer::Replace) | None => {
                state_obj.insert(k.clone(), v.clone());
            }
        }
    }
}

fn apply_aggregation(
    state: &mut Value,
    strategy: &AggregationStrategy,
    values: &[Value],
    reducers: &HashMap<String, StateReducer>,
) {
    match strategy {
        AggregationStrategy::ConcatOutputs => {
            let arr: Vec<Value> = values
                .iter()
                .filter_map(|v| v.get("output").cloned())
                .collect();
            if let Value::Object(m) = state {
                m.insert("output".to_string(), Value::Array(arr));
            } else {
                let mut m = serde_json::Map::new();
                m.insert("output".to_string(), Value::Array(arr));
                *state = Value::Object(m);
            }
        }
        AggregationStrategy::MergeObjects => {
            // No-op: per-output merging (with reducers) already ran in
            // `merge_into_state` while processing each child's output.
            // Re-merging here would clobber `SumNumbers`/`Collect`
            // reducer results.
            let _ = (state, values, reducers);
        }
        AggregationStrategy::First => {
            if let Some(first) = values.iter().find(|v| !v.is_null())
                && let Value::Object(o) = first
                && let Value::Object(state_obj) = state
            {
                for (k, val) in o {
                    if !reducers.contains_key(k) {
                        state_obj.insert(k.clone(), val.clone());
                    }
                }
            }
        }
        AggregationStrategy::Last => {
            if let Some(last) = values.last()
                && let Value::Object(o) = last
                && let Value::Object(state_obj) = state
            {
                for (k, val) in o {
                    if !reducers.contains_key(k) {
                        state_obj.insert(k.clone(), val.clone());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_mapper_passthrough_clones_state() {
        let s = serde_json::json!({"a": 1});
        assert_eq!(InputMapper::PassThrough.apply(&s), s);
    }

    #[test]
    fn input_mapper_select_returns_null_when_missing() {
        let s = serde_json::json!({"a": 1});
        let m = InputMapper::Select {
            path: "missing".into(),
        };
        assert_eq!(m.apply(&s), Value::Null);
    }

    #[test]
    fn input_mapper_select_flat_key_unchanged() {
        // Regression: a single-segment path must resolve identically to the
        // original flat-key behavior (`state.get(path)`), including for the
        // exact value, not a deep-copy mismatch.
        let s = serde_json::json!({"a": {"b": 1}, "c": 2});
        assert_eq!(
            InputMapper::Select { path: "a".into() }.apply(&s),
            serde_json::json!({"b": 1})
        );
        assert_eq!(
            InputMapper::Select { path: "c".into() }.apply(&s),
            serde_json::json!(2)
        );
    }

    #[test]
    fn input_mapper_select_nested_path_resolves() {
        let s = serde_json::json!({
            "review": { "findings": ["x", "y"] },
            "stage1": { "output": { "items": 42 } }
        });
        assert_eq!(
            InputMapper::Select {
                path: "review.findings".into()
            }
            .apply(&s),
            serde_json::json!(["x", "y"])
        );
        assert_eq!(
            InputMapper::Select {
                path: "stage1.output.items".into()
            }
            .apply(&s),
            serde_json::json!(42)
        );
    }

    #[test]
    fn input_mapper_select_array_index_segment() {
        let s = serde_json::json!({
            "items": [{"id": "first"}, {"id": "second"}]
        });
        assert_eq!(
            InputMapper::Select {
                path: "items.1.id".into()
            }
            .apply(&s),
            serde_json::json!("second")
        );
        // Out-of-bounds index resolves to Null (missing-path contract).
        assert_eq!(
            InputMapper::Select {
                path: "items.9.id".into()
            }
            .apply(&s),
            Value::Null
        );
    }

    #[test]
    fn input_mapper_select_missing_nested_path_returns_null() {
        // Preserves the original missing-key contract: never panic, never
        // error — a non-resolving nested path yields Value::Null.
        let s = serde_json::json!({"review": {"findings": []}});
        assert_eq!(
            InputMapper::Select {
                path: "review.missing.deeper".into()
            }
            .apply(&s),
            Value::Null
        );
        // Descending into a scalar also yields Null rather than panicking.
        assert_eq!(
            InputMapper::Select {
                path: "review.findings.notindex".into()
            }
            .apply(&s),
            Value::Null
        );
    }

    #[test]
    fn predicate_evaluates_state_equals() {
        let s = serde_json::json!({"k": "v"});
        let p = Predicate::StateEquals {
            path: "k".into(),
            value: serde_json::json!("v"),
        };
        assert!(p.evaluate(&s));
    }

    #[test]
    fn predicate_evaluates_state_gt() {
        let s = serde_json::json!({"score": 7.5});
        assert!(
            Predicate::StateGt {
                path: "score".into(),
                threshold: 5.0
            }
            .evaluate(&s)
        );
        assert!(
            !Predicate::StateGt {
                path: "score".into(),
                threshold: 8.0
            }
            .evaluate(&s)
        );
    }

    #[test]
    fn is_direct_detects_single_agent_graph() {
        let g = GraphConfig::single_node("main", InputMapper::PassThrough);
        assert!(g.is_direct());
    }

    #[test]
    fn unknown_entry_node_errors() {
        let cfg = GraphConfig::empty("ghost");
        let exec = Arc::new(NoopExec);
        let result = futures::executor::block_on(ExecutionGraph::execute(
            cfg,
            Value::Null,
            GraphContext {
                cancel: CancellationToken::new(),
                executor: exec,
            },
        ));
        assert!(matches!(result, Err(GraphError::UnknownEntry(_))));
    }

    struct NoopExec;
    #[async_trait]
    impl NodeExecutor for NoopExec {
        async fn run_agent(&self, _agent: &str, _input: &Value) -> Result<Value, String> {
            Ok(Value::Null)
        }
    }
}
