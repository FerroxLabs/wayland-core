//! RON workflow IR + lowering onto the existing [`GraphConfig`].
//!
//! This is the declarative front-end (SPEC §4 "front-door B"). An author
//! writes a workflow in RON; [`parse_workflow`] parses it into the IR
//! types below and *lowers* it onto a [`GraphConfig`] built with the
//! existing `empty()` + `add_*` + `add_edge` builders. No new execution
//! engine is introduced — the lowered `GraphConfig` is the same IR the
//! per-turn templates produce; only the front-door differs.
//!
//! ## RON shape (SPEC §5)
//!
//! ```ron
//! Workflow(
//!     meta: (name: "review-changes", description: "...", est_agents: 4),
//!     schemas: { "findings": "{...json schema...}" },
//!     phases: [
//!         Phase(
//!             title: "analyze",
//!             steps: [
//!                 Agent((id: "scan", prompt: "scan the diff")),
//!                 Pipeline(id: "review", stages: [
//!                     (id: "lint",   prompt: "lint", schema: None, model: None, input: None),
//!                     (id: "verify", prompt: "verify", schema: Some("findings"), model: None,
//!                                    input: Some("lint")),
//!                 ]),
//!                 Parallel(id: "vote", branches: [
//!                     (id: "a", prompt: "...", schema: None, model: None, input: None),
//!                     (id: "b", prompt: "...", schema: None, model: None, input: None),
//!                 ], join: Collect),
//!             ],
//!         ),
//!     ],
//! )
//! ```
//!
//! ## Lowering rules
//!
//! - `Agent`    → one [`GraphConfig::add_agent`] node, fed from the
//!   running state via its `input` (flat-key [`InputMapper::Select`] or
//!   `PassThrough`).
//! - `Pipeline` → a chain of agent nodes wired with [`GraphConfig::add_edge`];
//!   each stage reads the previous stage's output through a flat-key
//!   `Select` when it declares an `input`, else `PassThrough`.
//! - `Parallel` → sibling agent nodes feeding an [`AggregationStrategy`]
//!   aggregator, modeled on `parallel_fanout` / `multi_agent_consensus`.
//!
//! Cross-stage data refs use [`InputMapper::Select`] with **flat keys**
//! only — nested JSON-pointer support is being added in parallel by task
//! A2; this lowering does not depend on it.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::super::graph::{
    AggregationStrategy, GraphConfig, InputMapper, NodeBudget, ProviderPin, StateReducer,
};
use super::error::WorkflowParseError;
use super::limits;
use super::meta::WorkflowMeta;

/// Reject an attacker-controlled RON document that is too large or too deeply
/// nested **before** handing it to `ron::from_str`. RON 0.8 recurses without a
/// depth limit, so deep nesting overflows the stack (an uncatchable abort);
/// this guard turns both DoS shapes into typed, catchable parse errors.
///
/// Shared by [`parse_workflow`] and [`super::runner::WorkflowPlan::parse`] —
/// every `ron::from_str` on workflow input must call it first.
pub(crate) fn guard_ron_size_and_depth(src: &str) -> Result<(), WorkflowParseError> {
    if src.len() > limits::MAX_RON_BYTES {
        return Err(WorkflowParseError::TooLarge {
            size: src.len(),
            limit: limits::MAX_RON_BYTES,
        });
    }
    if let Err(depth) = limits::check_nesting_depth(src) {
        return Err(WorkflowParseError::TooDeep {
            depth,
            limit: limits::MAX_NESTING_DEPTH,
        });
    }
    Ok(())
}

/// Top-level RON workflow document.
#[derive(Debug, Clone, Deserialize)]
pub struct Workflow {
    pub meta: WorkflowMeta,
    /// Named JSON Schema table. A step's `schema: Some("name")` must
    /// resolve to a key here. Schemas are opaque strings at parse time;
    /// task A4 interprets them.
    #[serde(default)]
    pub schemas: HashMap<String, String>,
    pub phases: Vec<Phase>,
}

/// A named group of steps (AgentNav header label at runtime).
#[derive(Debug, Clone, Deserialize)]
pub struct Phase {
    #[serde(default)]
    pub title: String,
    pub steps: Vec<Step>,
}

/// One step inside a phase.
#[derive(Debug, Clone, Deserialize)]
pub enum Step {
    /// A single sub-agent call.
    Agent(AgentSpec),
    /// An ordered chain of sub-agent calls.
    ///
    /// Two modes, selected by `over`:
    ///
    /// - **Chain (no `over`):** the whole running state flows through the
    ///   stages once, each stage feeding the next. Lowered to a chain of
    ///   agent nodes wired by plain edges (the original A1 behaviour).
    /// - **No-barrier pipeline (`over: Some(ref)`):** `ref` resolves to an
    ///   array in the running state; **each item flows through all stages
    ///   independently** with no barrier between stages (item A may be in
    ///   stage 3 while item B is still in stage 1). A stage error drops that
    ///   item to `null` and skips its remaining stages. This is the SPEC §3
    ///   item-2 mechanic, executed by [`super::pipeline::run_pipeline`]. It
    ///   lowers to a single placeholder graph node (named `id`) that the
    ///   runner recognises and delegates to the no-barrier scheduler.
    Pipeline {
        id: String,
        /// When `Some`, a flat-key/dotted ref into the running state that must
        /// resolve to an array; each element streams through `stages`
        /// independently (no barrier). When `None`, the classic chain.
        #[serde(default)]
        over: Option<String>,
        stages: Vec<AgentSpec>,
    },
    /// Sibling sub-agent calls that run concurrently and fan into an
    /// aggregator.
    Parallel {
        id: String,
        branches: Vec<AgentSpec>,
        #[serde(default)]
        join: JoinStrategy,
    },
}

/// How a [`Step::Parallel`] folds its branch outputs.
///
/// **v1 fold semantics (FIX C).** All three variants currently fold the
/// branch outputs into a JSON **array** at the aggregator's state key — the
/// runner's `apply_aggregator` renders `Collect`, `Merge`, and `Concat`
/// identically (see `runner::WorkflowRunner::apply_aggregator`). `Merge` and
/// `Concat` are therefore accepted **aliases of the array fold in v1**, not
/// distinct deep-merge / string-concat reducers — those richer semantics are
/// not yet wired, so this enum deliberately does not claim them. Authors who
/// need a specific fold shape should use `Collect` (the explicit array) until
/// a future task implements true object-merge / field-concat reducers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum JoinStrategy {
    /// Collect each branch's output into an array (the consensus shape).
    #[default]
    Collect,
    /// v1: alias of [`JoinStrategy::Collect`] — folds branch outputs into an
    /// array. (Reserved for a future deep-merge reducer; NOT yet implemented.)
    Merge,
    /// v1: alias of [`JoinStrategy::Collect`] — folds branch outputs into an
    /// array. (Reserved for a future field-concat reducer; NOT yet implemented.)
    Concat,
}

/// A single agent invocation spec.
///
/// `prompt` is required; everything else is optional and defaults to
/// `None`/empty so terse RON stays readable.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentSpec {
    /// Stable id for the lowered graph node. Required so cross-stage
    /// refs and AgentNav grouping have a stable handle.
    pub id: String,
    /// The instruction the sub-agent runs.
    pub prompt: String,
    /// Optional named schema (must exist in [`Workflow::schemas`]).
    #[serde(default)]
    pub schema: Option<String>,
    /// Optional model override (e.g. a cheaper Haiku-tier model).
    #[serde(default)]
    pub model: Option<String>,
    /// Crucible — optional provider pin (`"openai"` or `"openai:gpt-5.5"`). When
    /// set (or `model` is set), this node lowers a [`ProviderPin`] into the
    /// graph's `node_providers` side-table so the runner spawns it on that
    /// provider. `None` ⇒ the node inherits the spawner's provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Optional flat-key reference into the running state. When set, the
    /// node reads that key via [`InputMapper::Select`]; otherwise it
    /// receives the whole state via [`InputMapper::PassThrough`].
    #[serde(default)]
    pub input: Option<String>,
    /// Optional per-node turn budget override. When `None` the runner uses its
    /// `DEFAULT_MAX_TURNS`. Lets a complex node opt out of the global 8-turn cap
    /// that would otherwise silently truncate it.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Optional per-node output-token budget override. When `None` the runner
    /// uses its `DEFAULT_MAX_TOKENS` (4096).
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

impl AgentSpec {
    /// The input mapper this spec lowers to. Flat keys only (task A2
    /// adds nested support independently).
    fn input_mapper(&self) -> InputMapper {
        match &self.input {
            Some(key) => InputMapper::Select { path: key.clone() },
            None => InputMapper::PassThrough,
        }
    }
}

impl JoinStrategy {
    fn aggregation(self) -> AggregationStrategy {
        match self {
            JoinStrategy::Collect | JoinStrategy::Merge => AggregationStrategy::MergeObjects,
            JoinStrategy::Concat => AggregationStrategy::ConcatOutputs,
        }
    }
}

/// Parse a RON workflow and lower it onto a [`GraphConfig`].
///
/// Returns the lowered graph plus the author's [`WorkflowMeta`]. On any
/// structural problem (empty phase, dangling ref, unknown schema,
/// duplicate id, …) returns a typed [`WorkflowParseError`] with a
/// field/location pointer — never panics.
pub fn parse_workflow(src: &str) -> Result<(GraphConfig, WorkflowMeta), WorkflowParseError> {
    guard_ron_size_and_depth(src)?;
    let workflow: Workflow =
        ron::from_str(src).map_err(|e| WorkflowParseError::Ron(e.to_string()))?;
    lower(workflow)
}

/// Lower an already-parsed [`Workflow`] onto a [`GraphConfig`].
///
/// Split out from [`parse_workflow`] so callers holding an IR (e.g. a
/// synthesizer in task B7) can lower without re-serializing to RON.
pub fn lower(workflow: Workflow) -> Result<(GraphConfig, WorkflowMeta), WorkflowParseError> {
    if workflow.phases.is_empty() {
        return Err(WorkflowParseError::EmptyWorkflow {
            name: workflow.meta.name.clone(),
        });
    }

    // Flatten every step in phase order into a single sequential spine.
    // Steps within a workflow run in declaration order; the previous
    // step's terminal node(s) feed the next step's entry node. This
    // mirrors how `sequential_pipeline` chains stages, generalized to
    // mixed Agent/Pipeline/Parallel steps.
    let mut builder = Lowering::new(&workflow.schemas);

    for phase in &workflow.phases {
        if phase.steps.is_empty() {
            return Err(WorkflowParseError::EmptyPhase {
                phase: phase.title.clone(),
            });
        }
        for step in &phase.steps {
            builder.add_step(&phase.title, step)?;
        }
    }

    let Some(entry) = builder.entry else {
        // Unreachable in practice: we returned EmptyWorkflow/EmptyPhase
        // above, and every step type registers at least one node. Guard
        // anyway rather than unwrap.
        return Err(WorkflowParseError::EmptyWorkflow {
            name: workflow.meta.name.clone(),
        });
    };

    let mut graph = GraphConfig::empty(entry);
    graph.nodes = builder.nodes;
    graph.edges = builder.edges;
    graph.state_reducers = builder.state_reducers;
    graph.node_budgets = builder.node_budgets;
    graph.node_providers = builder.node_providers;

    Ok((graph, workflow.meta))
}

/// Internal accumulator that threads the "previous terminals" between
/// steps and enforces the structural invariants (unique ids, known
/// schemas, resolvable refs).
struct Lowering<'a> {
    schemas: &'a HashMap<String, String>,
    nodes: Vec<(String, super::super::graph::Node)>,
    edges: Vec<super::super::graph::Edge>,
    state_reducers: HashMap<String, StateReducer>,
    /// Per-node turn/token budget overrides, keyed by node id. Only nodes whose
    /// `AgentSpec` set `max_turns`/`max_tokens` get an entry; the runner falls
    /// back to its defaults for absent (or `None`-field) nodes.
    node_budgets: HashMap<String, NodeBudget>,
    /// Crucible — per-node provider pins, keyed by node id. Only nodes whose
    /// `AgentSpec` set `provider` and/or `model` get an entry; the runner
    /// inherits the spawner's provider for absent nodes. Mirrors `node_budgets`.
    node_providers: HashMap<String, ProviderPin>,
    /// Entry node of the whole graph (first node registered).
    entry: Option<String>,
    /// Terminal node ids of the previously-added step. The next step's
    /// entry node(s) get an inbound edge from each of these so steps run
    /// in declaration order.
    prev_terminals: Vec<String>,
    /// All node ids seen so far — for duplicate detection and ref
    /// resolution.
    seen_ids: HashSet<String>,
}

impl<'a> Lowering<'a> {
    fn new(schemas: &'a HashMap<String, String>) -> Self {
        Self {
            schemas,
            nodes: Vec::new(),
            edges: Vec::new(),
            state_reducers: HashMap::new(),
            node_budgets: HashMap::new(),
            node_providers: HashMap::new(),
            entry: None,
            prev_terminals: Vec::new(),
            seen_ids: HashSet::new(),
        }
    }

    /// The reserved prefix for lowering-minted synthetic node ids (e.g. the
    /// `__fan_root__<id>` root a `Parallel` step creates). User-authored ids may
    /// not use it — see [`WorkflowParseError::ReservedId`] (FIX D).
    const RESERVED_PREFIX: &'static str = "__";

    /// Register a *user-authored* node id, rejecting duplicates, empty names,
    /// and the reserved synthetic prefix. Synthetic ids the lowering mints go
    /// through [`Self::claim_synthetic_id`] instead, which skips the prefix
    /// check.
    fn claim_id(&mut self, phase: &str, id: &str) -> Result<(), WorkflowParseError> {
        // FIX D — a user id starting with the reserved `__` prefix could collide
        // with a synthetic id the lowering mints (e.g. `__fan_root__vote` vs a
        // `Parallel(id: "vote")` root), surfacing a confusing `DuplicateNodeId`
        // for an id the author never wrote. Reject it explicitly at its source.
        if id.starts_with(Self::RESERVED_PREFIX) {
            return Err(WorkflowParseError::ReservedId {
                id: id.to_string(),
                prefix: Self::RESERVED_PREFIX.to_string(),
            });
        }
        self.claim_synthetic_id(phase, id)
    }

    /// Register a node id (duplicate + empty-name checks only), WITHOUT the
    /// reserved-prefix guard. Used for lowering-minted synthetic ids, which are
    /// the only legitimate users of the reserved prefix.
    fn claim_synthetic_id(&mut self, phase: &str, id: &str) -> Result<(), WorkflowParseError> {
        if id.is_empty() {
            return Err(WorkflowParseError::EmptyAgentName {
                phase: phase.to_string(),
            });
        }
        if !self.seen_ids.insert(id.to_string()) {
            return Err(WorkflowParseError::DuplicateNodeId { id: id.to_string() });
        }
        Ok(())
    }

    /// Validate an [`AgentSpec`]'s schema + input references.
    fn check_refs(&self, spec: &AgentSpec) -> Result<(), WorkflowParseError> {
        if let Some(schema) = &spec.schema
            && !self.schemas.contains_key(schema)
        {
            return Err(WorkflowParseError::MissingSchema {
                step: spec.id.clone(),
                schema: schema.clone(),
            });
        }
        // A flat-key `input` ref must point at a node id produced earlier
        // in the workflow (its output lands on `state[that_id]`). The
        // very first node has nothing to reference, which is caught by
        // the "not yet seen" check too.
        if let Some(key) = &spec.input
            && !self.seen_ids.contains(key)
        {
            return Err(WorkflowParseError::DanglingRef {
                step: spec.id.clone(),
                reference: key.clone(),
            });
        }
        Ok(())
    }

    fn push_agent(&mut self, spec: &AgentSpec) {
        let id = spec.id.clone();
        // Record any per-node budget override so the runner can use it instead
        // of DEFAULT_MAX_TURNS/DEFAULT_MAX_TOKENS. Skip the entry entirely when
        // neither dimension is set so the side-table stays sparse.
        if spec.max_turns.is_some() || spec.max_tokens.is_some() {
            self.node_budgets.insert(
                id.clone(),
                NodeBudget {
                    max_turns: spec.max_turns,
                    max_tokens: spec.max_tokens,
                },
            );
        }
        // Crucible — record a per-node provider pin when the spec sets `provider`
        // and/or `model`. This finally consumes `spec.model` at the graph level
        // (previously parsed then dropped). Covers Agent, Pipeline-stage, and
        // Parallel-branch nodes since all funnel through `push_agent`.
        if spec.provider.is_some() || spec.model.is_some() {
            self.node_providers.insert(
                id.clone(),
                ProviderPin {
                    provider: spec.provider.clone(),
                    model: spec.model.clone(),
                },
            );
        }
        // Mirror `GraphConfig::add_agent` exactly (node id == agent name).
        self.nodes.push((
            id.clone(),
            super::super::graph::Node::AgentCall {
                agent: id,
                input_mapper: spec.input_mapper(),
            },
        ));
    }

    fn push_edge(&mut self, from: &str, to: &str) {
        self.edges.push(super::super::graph::Edge {
            from: from.to_string(),
            to: to.to_string(),
            when: None,
        });
    }

    /// Wire each previous terminal into `entry_node` so steps execute in
    /// declaration order, then record the graph entry on first use.
    fn link_into(&mut self, entry_node: &str) {
        if self.entry.is_none() {
            self.entry = Some(entry_node.to_string());
        }
        let prev = std::mem::take(&mut self.prev_terminals);
        for term in prev {
            self.push_edge(&term, entry_node);
        }
    }

    fn add_step(&mut self, phase: &str, step: &Step) -> Result<(), WorkflowParseError> {
        match step {
            Step::Agent(spec) => {
                self.claim_id(phase, &spec.id)?;
                self.check_refs(spec)?;
                self.link_into(&spec.id);
                self.push_agent(spec);
                self.prev_terminals = vec![spec.id.clone()];
            }
            Step::Pipeline { id, over, stages } => {
                if stages.is_empty() {
                    return Err(WorkflowParseError::EmptyPipeline {
                        phase: phase.to_string(),
                    });
                }
                match over {
                    // No-barrier pipeline: the stages do NOT become individual
                    // graph nodes (they run per-item inside the runner, not as a
                    // chain the Kahn walker dispatches). Lower the whole step to
                    // a single placeholder node named `id` so the walker reaches
                    // it and delegates to the no-barrier scheduler. Stage ids
                    // are still claimed (for uniqueness) and ref-checked, but the
                    // `over` ref points into the *running state* (a prior node's
                    // output), so it is validated against `seen_ids` here.
                    Some(over_ref) => {
                        self.claim_id(phase, id)?;
                        // `over` resolves against the *running state* at exec
                        // time, which includes both prior-node outputs AND the
                        // caller's initial state (e.g. a host injecting
                        // `changed_files`). An empty ref is the one thing we can
                        // statically reject as a certain authoring mistake; any
                        // non-empty ref that fails to resolve to an array at
                        // runtime simply runs zero items (handled in the runner).
                        if over_ref.is_empty() {
                            return Err(WorkflowParseError::DanglingRef {
                                step: id.clone(),
                                reference: over_ref.clone(),
                            });
                        }
                        for stage in stages {
                            self.claim_id(phase, &stage.id)?;
                            // A stage's `schema` ref must resolve; its per-item
                            // `input` ref selects a field of the *item value*, not
                            // a prior node, so it is NOT checked against seen_ids.
                            if let Some(schema) = &stage.schema
                                && !self.schemas.contains_key(schema)
                            {
                                return Err(WorkflowParseError::MissingSchema {
                                    step: stage.id.clone(),
                                    schema: schema.clone(),
                                });
                            }
                        }
                        // One placeholder node carries the step. PassThrough so
                        // the walker's default handling is inert; the runner
                        // detects the id in its pipeline side-table and runs the
                        // no-barrier scheduler instead.
                        self.link_into(id);
                        self.nodes
                            .push((id.clone(), super::super::graph::Node::PassThrough));
                        self.prev_terminals = vec![id.clone()];
                    }
                    // Classic chain: each stage is an agent node, wired in order
                    // (original A1 behaviour, unchanged).
                    None => {
                        // Reserve the pipeline id so it can't collide with a
                        // stage id even though it isn't itself a node.
                        self.claim_id(phase, id)?;
                        let first = stages[0].id.clone();
                        self.claim_id(phase, &stages[0].id)?;
                        self.check_refs(&stages[0])?;
                        self.link_into(&first);
                        self.push_agent(&stages[0]);
                        let mut prev = first;
                        for stage in &stages[1..] {
                            self.claim_id(phase, &stage.id)?;
                            self.check_refs(stage)?;
                            self.push_agent(stage);
                            self.push_edge(&prev, &stage.id);
                            prev = stage.id.clone();
                        }
                        self.prev_terminals = vec![prev];
                    }
                }
            }
            Step::Parallel { id, branches, join } => {
                if branches.len() < 2 {
                    return Err(WorkflowParseError::DegenerateParallel {
                        phase: phase.to_string(),
                        found: branches.len(),
                    });
                }
                // A synthetic passthrough root fans out to the branches,
                // and an aggregator (named `id`) fans them back in —
                // exactly the `parallel_fanout` / `multi_agent_consensus`
                // shape.
                self.claim_id(phase, id)?;
                let root = format!("__fan_root__{id}");
                // Synthetic root — claim it WITHOUT the reserved-prefix guard
                // (it legitimately uses the `__` prefix `claim_id` forbids).
                self.claim_synthetic_id(phase, &root)?;
                self.nodes
                    .push((root.clone(), super::super::graph::Node::PassThrough));
                self.link_into(&root);
                for branch in branches {
                    self.claim_id(phase, &branch.id)?;
                    self.check_refs(branch)?;
                    self.push_agent(branch);
                    self.push_edge(&root, &branch.id);
                }
                self.nodes.push((
                    id.clone(),
                    super::super::graph::Node::Aggregator {
                        strategy: join.aggregation(),
                    },
                ));
                for branch in branches {
                    self.push_edge(&branch.id, id);
                }
                // `Collect` join semantics: the aggregator folds each branch's
                // output into an array. The runner's `apply_aggregator` looks
                // the reducer up by the aggregator NODE id (`id`), so register
                // it under that key — not the literal `"output"`, which the
                // lookup never queries.
                if *join == JoinStrategy::Collect {
                    self.state_reducers
                        .insert(id.clone(), StateReducer::Collect);
                }
                self.prev_terminals = vec![id.clone()];
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::graph::Node;

    /// A representative workflow exercising all three step kinds, a
    /// cross-stage flat-key ref, and a named schema.
    const SAMPLE: &str = r#"
Workflow(
    meta: (name: "review-changes", description: "review a diff", est_agents: 5),
    schemas: { "findings": "{ \"type\": \"object\" }" },
    phases: [
        Phase(
            title: "analyze",
            steps: [
                Agent((id: "scan", prompt: "scan the diff")),
                Pipeline(id: "review", stages: [
                    (id: "lint", prompt: "lint it"),
                    (id: "verify", prompt: "verify", schema: Some("findings"), input: Some("lint")),
                ]),
                Parallel(id: "vote", branches: [
                    (id: "judge_a", prompt: "judge a"),
                    (id: "judge_b", prompt: "judge b"),
                ], join: Collect),
            ],
        ),
    ],
)
"#;

    fn node<'a>(g: &'a GraphConfig, id: &str) -> Option<&'a Node> {
        g.nodes.iter().find(|(n, _)| n == id).map(|(_, n)| n)
    }

    fn has_edge(g: &GraphConfig, from: &str, to: &str) -> bool {
        g.edges.iter().any(|e| e.from == from && e.to == to)
    }

    /// Extract the error from a parse result. `GraphConfig` does not
    /// derive `Debug`, so `Result::unwrap_err` (which requires `Debug`
    /// on the Ok side) can't be used directly.
    fn parse_err(src: &str) -> WorkflowParseError {
        match parse_workflow(src) {
            Ok(_) => panic!("expected a parse error, got Ok"),
            Err(e) => e,
        }
    }

    #[test]
    fn sample_lowers_to_expected_graph() {
        let (g, meta) = parse_workflow(SAMPLE).expect("sample should parse");

        assert_eq!(meta.name, "review-changes");
        assert_eq!(meta.est_agents, 5);

        // Entry is the first declared node.
        assert_eq!(g.entry, "scan");

        // Every agent node is present and is an AgentCall.
        for id in ["scan", "lint", "verify", "judge_a", "judge_b"] {
            assert!(
                matches!(node(&g, id), Some(Node::AgentCall { .. })),
                "missing agent node {id}"
            );
        }
        // The aggregator + synthetic fan root exist.
        assert!(matches!(node(&g, "vote"), Some(Node::Aggregator { .. })));
        assert!(matches!(
            node(&g, "__fan_root__vote"),
            Some(Node::PassThrough)
        ));

        // Sequential spine: scan -> lint, pipeline chain lint -> verify,
        // verify -> fan root, fan root -> each branch, each branch -> agg.
        assert!(has_edge(&g, "scan", "lint"));
        assert!(has_edge(&g, "lint", "verify"));
        assert!(has_edge(&g, "verify", "__fan_root__vote"));
        assert!(has_edge(&g, "__fan_root__vote", "judge_a"));
        assert!(has_edge(&g, "__fan_root__vote", "judge_b"));
        assert!(has_edge(&g, "judge_a", "vote"));
        assert!(has_edge(&g, "judge_b", "vote"));

        // `verify` reads `lint`'s output via a flat-key Select.
        // `Node` does not derive `Debug`, so match without `{:?}`.
        match node(&g, "verify") {
            Some(Node::AgentCall { input_mapper, .. }) => match input_mapper {
                InputMapper::Select { path } => assert_eq!(path, "lint"),
                _ => panic!("expected a flat-key Select input mapper on `verify`"),
            },
            _ => panic!("expected `verify` to be an AgentCall node"),
        }

        // Collect join installs the Collect reducer keyed by the aggregator
        // NODE id (`vote`) — the key `apply_aggregator` looks up — not the
        // literal `"output"`, which the runner never queries.
        assert!(matches!(
            g.state_reducers.get("vote"),
            Some(StateReducer::Collect)
        ));
        assert!(!g.state_reducers.contains_key("output"));
    }

    #[test]
    fn per_node_budget_overrides_lower_into_node_budgets() {
        // `big` sets both overrides, `wide` sets only one, `plain` sets none.
        let src = r#"
Workflow(
    meta: (name: "budgets"),
    phases: [
        Phase(title: "p", steps: [
            Agent((id: "big", prompt: "do a lot", max_turns: Some(40), max_tokens: Some(16000))),
            Agent((id: "wide", prompt: "wide only", max_turns: Some(20))),
            Agent((id: "plain", prompt: "default budget")),
        ]),
    ],
)
"#;
        let (g, _meta) = parse_workflow(src).expect("should parse");

        // A node that set both dimensions lowers to a NodeBudget carrying both.
        assert_eq!(
            g.node_budgets.get("big"),
            Some(&NodeBudget {
                max_turns: Some(40),
                max_tokens: Some(16000),
            }),
        );
        // A node that set only one dimension carries that one; the other is None
        // so the runner falls back to its default for that dimension.
        assert_eq!(
            g.node_budgets.get("wide"),
            Some(&NodeBudget {
                max_turns: Some(20),
                max_tokens: None,
            }),
        );
        // A node with no override has no entry at all — the side-table stays
        // sparse and the runner uses DEFAULT_MAX_TURNS/DEFAULT_MAX_TOKENS.
        assert!(!g.node_budgets.contains_key("plain"));
    }

    #[test]
    fn provider_and_model_lower_into_node_providers() {
        // `both` pins provider+model, `prov` provider-only, `mdl` model-only,
        // `plain` neither (so the side-table stays sparse).
        let src = r#"
Workflow(
    meta: (name: "council"),
    phases: [
        Phase(title: "p", steps: [
            Agent((id: "both", prompt: "x", provider: Some("openai"), model: Some("gpt-5.5"))),
            Agent((id: "prov", prompt: "x", provider: Some("anthropic"))),
            Agent((id: "mdl", prompt: "x", model: Some("claude-opus-4-8"))),
            Agent((id: "plain", prompt: "x")),
        ]),
    ],
)
"#;
        let (g, _meta) = parse_workflow(src).expect("should parse");

        assert_eq!(
            g.node_providers.get("both"),
            Some(&ProviderPin {
                provider: Some("openai".into()),
                model: Some("gpt-5.5".into()),
            }),
        );
        assert_eq!(
            g.node_providers.get("prov"),
            Some(&ProviderPin {
                provider: Some("anthropic".into()),
                model: None,
            }),
        );
        assert_eq!(
            g.node_providers.get("mdl"),
            Some(&ProviderPin {
                provider: None,
                model: Some("claude-opus-4-8".into()),
            }),
        );
        assert!(!g.node_providers.contains_key("plain"));
    }

    #[test]
    fn parallel_branches_lower_provider_pins() {
        // The council topology: a Parallel step whose branches each pin a
        // different provider. All branches funnel through `push_agent`, so the
        // pins must land in node_providers.
        let src = r#"
Workflow(
    meta: (name: "council-fanout"),
    phases: [
        Phase(title: "p", steps: [
            Parallel(id: "fuse", branches: [
                (id: "p_openai", prompt: "answer", provider: Some("openai")),
                (id: "p_anthropic", prompt: "answer", provider: Some("anthropic")),
            ], join: Collect),
        ]),
    ],
)
"#;
        let (g, _meta) = parse_workflow(src).expect("should parse");

        assert_eq!(
            g.node_providers
                .get("p_openai")
                .and_then(|p| p.provider.as_deref()),
            Some("openai"),
        );
        assert_eq!(
            g.node_providers
                .get("p_anthropic")
                .and_then(|p| p.provider.as_deref()),
            Some("anthropic"),
        );
    }

    #[test]
    fn err_invalid_ron_syntax() {
        let err = parse_err("this is not ron");
        assert!(matches!(err, WorkflowParseError::Ron(_)));
    }

    #[test]
    fn err_empty_workflow_no_phases() {
        let src = r#"Workflow(meta: (name: "x"), phases: [])"#;
        let err = parse_err(src);
        assert!(matches!(err, WorkflowParseError::EmptyWorkflow { .. }));
    }

    #[test]
    fn err_empty_phase() {
        let src = r#"Workflow(meta: (name: "x"), phases: [Phase(title: "p", steps: [])])"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::EmptyPhase { phase } => assert_eq!(phase, "p"),
            other => panic!("expected EmptyPhase, got {other:?}"),
        }
    }

    #[test]
    fn err_missing_schema() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "a", prompt: "p", schema: Some("nope"))),
    ])],
)
"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::MissingSchema { step, schema } => {
                assert_eq!(step, "a");
                assert_eq!(schema, "nope");
            }
            other => panic!("expected MissingSchema, got {other:?}"),
        }
    }

    #[test]
    fn err_dangling_ref() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "a", prompt: "p", input: Some("ghost"))),
    ])],
)
"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::DanglingRef { step, reference } => {
                assert_eq!(step, "a");
                assert_eq!(reference, "ghost");
            }
            other => panic!("expected DanglingRef, got {other:?}"),
        }
    }

    #[test]
    fn err_duplicate_node_id() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "dup", prompt: "one")),
        Agent((id: "dup", prompt: "two")),
    ])],
)
"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::DuplicateNodeId { id } => assert_eq!(id, "dup"),
            other => panic!("expected DuplicateNodeId, got {other:?}"),
        }
    }

    #[test]
    fn err_degenerate_parallel() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Parallel(id: "v", branches: [(id: "only", prompt: "p")], join: Collect),
    ])],
)
"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::DegenerateParallel { phase, found } => {
                assert_eq!(phase, "p");
                assert_eq!(found, 1);
            }
            other => panic!("expected DegenerateParallel, got {other:?}"),
        }
    }

    #[test]
    fn err_empty_pipeline() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", stages: []),
    ])],
)
"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::EmptyPipeline { phase } => assert_eq!(phase, "p"),
            other => panic!("expected EmptyPipeline, got {other:?}"),
        }
    }

    #[test]
    fn err_empty_agent_name() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "", prompt: "p")),
    ])],
)
"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::EmptyAgentName { phase } => assert_eq!(phase, "p"),
            other => panic!("expected EmptyAgentName, got {other:?}"),
        }
    }

    #[test]
    fn err_reserved_id_prefix() {
        // FIX D — a user id colliding with the synthetic fan-root prefix must be
        // rejected as ReservedId (not a confusing DuplicateNodeId).
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "__fan_root__x", prompt: "p")),
    ])],
)
"#;
        let err = parse_err(src);
        match err {
            WorkflowParseError::ReservedId { id, prefix } => {
                assert_eq!(id, "__fan_root__x");
                assert_eq!(prefix, "__");
            }
            other => panic!("expected ReservedId, got {other:?}"),
        }
    }

    #[test]
    fn err_reserved_id_any_double_underscore_prefix() {
        // Any `__`-prefixed user id is reserved, not just the fan-root form.
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "__synthetic", prompt: "p")),
    ])],
)
"#;
        match parse_err(src) {
            WorkflowParseError::ReservedId { id, .. } => assert_eq!(id, "__synthetic"),
            other => panic!("expected ReservedId, got {other:?}"),
        }
    }

    #[test]
    fn single_agent_workflow_lowers_to_one_node() {
        let src = r#"Workflow(meta: (name: "solo"), phases: [Phase(steps: [Agent((id: "only", prompt: "go"))])])"#;
        let (g, meta) = parse_workflow(src).expect("should parse");
        assert_eq!(meta.name, "solo");
        assert_eq!(g.entry, "only");
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.is_empty());
        assert!(matches!(node(&g, "only"), Some(Node::AgentCall { .. })));
    }

    #[test]
    fn concat_join_uses_concat_outputs_strategy() {
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Parallel(id: "agg", branches: [
            (id: "a", prompt: "pa"),
            (id: "b", prompt: "pb"),
        ], join: Concat),
    ])],
)
"#;
        let (g, _) = parse_workflow(src).expect("should parse");
        assert!(matches!(
            node(&g, "agg"),
            Some(Node::Aggregator {
                strategy: AggregationStrategy::ConcatOutputs
            })
        ));
        // Concat does NOT install the Collect reducer (neither under the
        // aggregator id nor the legacy `"output"` key).
        assert!(!g.state_reducers.contains_key("agg"));
        assert!(!g.state_reducers.contains_key("output"));
    }

    #[test]
    fn collect_join_reducer_keyed_by_aggregator_id() {
        // FIX 2 regression: the Collect reducer must be registered under the
        // aggregator NODE id (the key `apply_aggregator` reads), not the
        // literal `"output"`, or the reducer is dead and downstream reads see
        // `Null`.
        let src = r#"
Workflow(
    meta: (name: "x"),
    phases: [Phase(title: "p", steps: [
        Parallel(id: "consensus", branches: [
            (id: "a", prompt: "pa"),
            (id: "b", prompt: "pb"),
        ], join: Collect),
    ])],
)
"#;
        let (g, _) = parse_workflow(src).expect("should parse");
        assert!(
            matches!(
                g.state_reducers.get("consensus"),
                Some(StateReducer::Collect)
            ),
            "Collect reducer must be keyed by the aggregator node id"
        );
        assert!(
            !g.state_reducers.contains_key("output"),
            "Collect reducer must not be keyed by the literal `output`"
        );
    }
}
