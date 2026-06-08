//! B5 — IR-walking cost estimator.
//!
//! Computes a workflow's agent/cost footprint **before any spawn** by
//! statically walking the lowered [`WorkflowPlan`] IR — **never** by trusting
//! the author's self-declared
//! [`crate::orchestration::workflow::meta::WorkflowMeta::est_agents`] hint
//! (cross-AI item 6: "drop `Meta.est_agents` as the source of truth").
//!
//! ## What gets counted
//!
//! The agent count is the number of sub-agent dispatches the run will make:
//!
//! 1. **Every `AgentCall` graph node.** A plain `Agent` step, each stage of a
//!    *classic* (`over: None`) pipeline, and every `Parallel` branch all lower
//!    to real `AgentCall` nodes in [`WorkflowPlan::graph`] (see `dsl::lower`),
//!    so a single pass over `graph.nodes` captures all of them — including the
//!    full Parallel fan-out (N branches → N nodes). Non-agent nodes
//!    (`Aggregator`, `PassThrough`, `Predicate`, `End`, `Loop`) contribute no
//!    sub-agent of their own.
//!
//! 2. **No-barrier pipeline stages, scaled by cardinality.** A
//!    `Pipeline(over: Some(ref))` step does **not** lower its stages to graph
//!    nodes — it lowers to a single `PassThrough` placeholder and parks its
//!    stages in [`WorkflowPlan::pipelines`]. At runtime every item of the
//!    `over` collection streams through all stages, so the real dispatch count
//!    is `stages.len() * cardinality(over)`. We resolve `over` against the
//!    provided `initial_state` (the host's starting object, e.g. an injected
//!    `changed_files` array). If it resolves to an array we use its length; if
//!    it is statically unresolvable (missing key, or not an array) we fall back
//!    to [`UNKNOWN_CARDINALITY`] and flag `cardinality_unknown`.
//!
//! ## Cost model (rough planning numbers)
//!
//! These are deliberately coarse constants for an *order-of-magnitude*
//! pre-spend estimate shown on the confirm card — not billing. Each is a
//! single documented constant so tuning is one edit:
//!
//! - [`AVG_INPUT_TOKENS_PER_AGENT`] / [`AVG_OUTPUT_TOKENS_PER_AGENT`] — average
//!   prompt / completion size for one workflow sub-agent turn.
//! - [`USD_PER_1K_INPUT_TOKENS`] / [`USD_PER_1K_OUTPUT_TOKENS`] — a blended
//!   mid-tier price; input and output priced separately as providers do.
//! - [`AVG_SECS_PER_AGENT`] — wall-clock per agent. Agents in a workflow run
//!   with bounded concurrency, but this estimate is intentionally a simple
//!   per-agent product (an upper-ish bound), not a critical-path schedule.

use serde_json::Value;

use super::super::graph::{InputMapper, Node};
use super::runner::WorkflowPlan;

/// Cardinality assumed for an `over` collection that cannot be resolved
/// statically from the initial state (missing key, or the ref is not an
/// array). One item is the minimum a pipeline can meaningfully process; the
/// estimate flags this assumption via [`CostEstimate::cardinality_unknown`].
pub const UNKNOWN_CARDINALITY: usize = 1;

/// Average input (prompt) tokens consumed by one workflow sub-agent turn.
pub const AVG_INPUT_TOKENS_PER_AGENT: u64 = 2_000;

/// Average output (completion) tokens produced by one workflow sub-agent turn.
pub const AVG_OUTPUT_TOKENS_PER_AGENT: u64 = 800;

/// Blended price per 1,000 input tokens (USD). Mid-tier model ballpark.
pub const USD_PER_1K_INPUT_TOKENS: f64 = 0.003;

/// Blended price per 1,000 output tokens (USD). Mid-tier model ballpark.
pub const USD_PER_1K_OUTPUT_TOKENS: f64 = 0.015;

/// Average wall-clock seconds attributed to one sub-agent.
pub const AVG_SECS_PER_AGENT: u64 = 20;

/// A pre-execution cost/footprint estimate for a workflow, derived purely by
/// walking the lowered IR.
#[derive(Debug, Clone, PartialEq)]
pub struct CostEstimate {
    /// Total sub-agent dispatches the run will make (IR-derived).
    pub agents: usize,
    /// Estimated total input (prompt) tokens across all agents.
    pub est_input_tokens: u64,
    /// Estimated total output (completion) tokens across all agents.
    pub est_output_tokens: u64,
    /// Estimated total spend in USD.
    pub est_usd: f64,
    /// Estimated wall-clock seconds (per-agent product, not critical path).
    pub est_secs: u64,
    /// `true` when at least one `over:` collection could not be resolved
    /// against the initial state and fell back to [`UNKNOWN_CARDINALITY`], so
    /// the agent count (and everything derived from it) is a floor, not exact.
    pub cardinality_unknown: bool,
}

/// Statically estimate a workflow's agent/cost footprint by walking `plan`'s
/// lowered IR, resolving each no-barrier pipeline's `over` collection against
/// `initial_state`.
///
/// This **ignores** `plan.meta.est_agents` entirely — the agent count is
/// derived only from the graph + pipeline side-tables.
pub fn estimate(plan: &WorkflowPlan, initial_state: &Value) -> CostEstimate {
    // 1. Every AgentCall graph node is one dispatch. This already includes
    //    plain agents, classic-pipeline stages, and every Parallel branch.
    let mut agents: usize = plan
        .graph
        .nodes
        .iter()
        .filter(|(_, node)| matches!(node, Node::AgentCall { .. }))
        .count();

    // 1b. FIX B — `Loop` nodes. A `Loop` is NOT an `AgentCall`, so the pass
    //     above counts it as 0 — but at runtime the runner's `run_loop`
    //     dispatches every one of the loop's inner `agents` once per iteration,
    //     up to `min(max_iters, LOOP_ITER_CAP)` iterations (the exact cap
    //     `run_loop` enforces). A loop of 10 agents × 100 declared iters
    //     therefore really dispatches `10 * min(100, 16) = 160` sub-agents, not
    //     0. The loop's inner agents are NOT separate graph nodes, so this never
    //     double-counts the AgentCall pass above.
    for (_, node) in &plan.graph.nodes {
        if let Node::Loop {
            agents: loop_agents,
            max_iters,
            ..
        } = node
        {
            let iters = (*max_iters).min(crate::orchestration::workflow::runner::LOOP_ITER_CAP);
            agents = agents.saturating_add(loop_agents.len().saturating_mul(iters));
        }
    }

    // 2. No-barrier pipelines: stages * resolved cardinality of `over`.
    let mut cardinality_unknown = false;
    for def in plan.pipelines.values() {
        let cardinality = match resolve_cardinality(&def.over, initial_state) {
            Some(n) => n,
            None => {
                cardinality_unknown = true;
                UNKNOWN_CARDINALITY
            }
        };
        agents = agents.saturating_add(def.stages.len().saturating_mul(cardinality));
    }

    let agents_u64 = agents as u64;
    let est_input_tokens = agents_u64.saturating_mul(AVG_INPUT_TOKENS_PER_AGENT);
    let est_output_tokens = agents_u64.saturating_mul(AVG_OUTPUT_TOKENS_PER_AGENT);
    let est_usd = (est_input_tokens as f64 / 1_000.0) * USD_PER_1K_INPUT_TOKENS
        + (est_output_tokens as f64 / 1_000.0) * USD_PER_1K_OUTPUT_TOKENS;
    let est_secs = agents_u64.saturating_mul(AVG_SECS_PER_AGENT);

    CostEstimate {
        agents,
        est_input_tokens,
        est_output_tokens,
        est_usd,
        est_secs,
        cardinality_unknown,
    }
}

/// Resolve an `over` ref against the initial state and return the array length,
/// or `None` when it is missing or not an array (the caller treats `None` as
/// the `cardinality_unknown` fallback).
///
/// Uses the same [`InputMapper::Select`] resolution the runner uses for `over`
/// at execution time, so the estimate matches what the run will actually see.
fn resolve_cardinality(over: &str, initial_state: &Value) -> Option<usize> {
    let resolved = InputMapper::Select {
        path: over.to_string(),
    }
    .apply(initial_state);
    match resolved {
        Value::Array(items) => Some(items.len()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::workflow::runner::WorkflowPlan;
    use serde_json::json;

    /// Build a plan from RON, panicking with the parse error on failure.
    fn plan(src: &str) -> WorkflowPlan {
        match WorkflowPlan::parse(src) {
            Ok(p) => p,
            Err(e) => panic!("workflow should parse: {e:?}"),
        }
    }

    /// 12 agent steps → `agents == 12`, derived from the IR. The RON declares a
    /// deliberately WRONG `est_agents: 999` to prove the estimate ignores it.
    #[test]
    fn counts_twelve_agent_steps_ignoring_wrong_meta() {
        let mut steps = String::new();
        for i in 0..12 {
            steps.push_str(&format!("Agent((id: \"a{i}\", prompt: \"do {i}\")),\n"));
        }
        let src = format!(
            r#"
Workflow(
    meta: (name: "twelve", est_agents: 999),
    phases: [Phase(title: "p", steps: [
{steps}    ])],
)
"#
        );
        let p = plan(&src);
        // The author lied: meta says 999. The IR walk must say 12.
        assert_eq!(p.meta.est_agents, 999, "precondition: meta hint is wrong");
        let est = estimate(&p, &Value::Null);
        assert_eq!(est.agents, 12, "agent count must come from IR, not meta");
        assert_ne!(
            est.agents, p.meta.est_agents,
            "estimate must not echo the meta hint"
        );
        assert!(!est.cardinality_unknown);
    }

    /// A no-barrier pipeline `over` a 5-item collection (in initial_state) with
    /// 3 stages counts 5 * 3 = 15 agents.
    #[test]
    fn pipeline_over_collection_multiplies_stages_by_cardinality() {
        let src = r#"
Workflow(
    meta: (name: "fanpipe"),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("files"), stages: [
            (id: "s1", prompt: "stage 1"),
            (id: "s2", prompt: "stage 2"),
            (id: "s3", prompt: "stage 3"),
        ]),
    ])],
)
"#;
        let p = plan(src);
        let state = json!({ "files": ["a", "b", "c", "d", "e"] });
        let est = estimate(&p, &state);
        assert_eq!(est.agents, 15, "5 items * 3 stages");
        assert!(!est.cardinality_unknown);
    }

    /// When the `over` ref is unresolvable against the initial state, fall back
    /// to UNKNOWN_CARDINALITY (1) and flag `cardinality_unknown`.
    #[test]
    fn pipeline_over_unresolvable_falls_back_and_flags() {
        let src = r#"
Workflow(
    meta: (name: "fanpipe"),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("missing"), stages: [
            (id: "s1", prompt: "stage 1"),
            (id: "s2", prompt: "stage 2"),
        ]),
    ])],
)
"#;
        let p = plan(src);
        // Empty state: `missing` does not resolve to an array.
        let est = estimate(&p, &json!({}));
        assert!(est.cardinality_unknown, "must flag the unknown collection");
        assert_eq!(
            est.agents,
            2 * UNKNOWN_CARDINALITY,
            "2 stages * fallback cardinality of 1"
        );
    }

    /// A parallel fan-out of N branches counts exactly N agents (each branch is
    /// a real AgentCall node; the synthetic fan root + aggregator are not).
    #[test]
    fn parallel_fanout_counts_branches() {
        let n = 4;
        let mut branches = String::new();
        for i in 0..n {
            branches.push_str(&format!("(id: \"b{i}\", prompt: \"branch {i}\"),\n"));
        }
        let src = format!(
            r#"
Workflow(
    meta: (name: "fan"),
    phases: [Phase(title: "p", steps: [
        Parallel(id: "vote", branches: [
{branches}        ], join: Collect),
    ])],
)
"#
        );
        let p = plan(&src);
        let est = estimate(&p, &Value::Null);
        assert_eq!(est.agents, n, "exactly N branches, no aggregator/root");
        assert!(!est.cardinality_unknown);
    }

    /// The derived cost figures are bounded, non-zero, and scale off the agent
    /// count via the documented constants.
    #[test]
    fn cost_figures_derive_from_agent_count() {
        let src = r#"
Workflow(
    meta: (name: "one"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "only", prompt: "go")),
    ])],
)
"#;
        let p = plan(src);
        let est = estimate(&p, &Value::Null);
        assert_eq!(est.agents, 1);
        assert_eq!(est.est_input_tokens, AVG_INPUT_TOKENS_PER_AGENT);
        assert_eq!(est.est_output_tokens, AVG_OUTPUT_TOKENS_PER_AGENT);
        assert_eq!(est.est_secs, AVG_SECS_PER_AGENT);
        // Non-zero, finite, and matches the documented per-token formula.
        let expected_usd = (AVG_INPUT_TOKENS_PER_AGENT as f64 / 1_000.0) * USD_PER_1K_INPUT_TOKENS
            + (AVG_OUTPUT_TOKENS_PER_AGENT as f64 / 1_000.0) * USD_PER_1K_OUTPUT_TOKENS;
        assert!((est.est_usd - expected_usd).abs() < 1e-12);
        assert!(est.est_usd > 0.0 && est.est_usd.is_finite());

        // Scaling check: more agents => proportionally more cost.
        let src2 = r#"
Workflow(
    meta: (name: "two"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "a", prompt: "x")),
        Agent((id: "b", prompt: "y", input: Some("a"))),
    ])],
)
"#;
        let est2 = estimate(&plan(src2), &Value::Null);
        assert_eq!(est2.agents, 2);
        assert_eq!(est2.est_input_tokens, 2 * AVG_INPUT_TOKENS_PER_AGENT);
        assert!(est2.est_usd > est.est_usd);
    }

    /// FIX B — a `Loop` node contributes `agents.len() * min(max_iters, cap)`
    /// to the estimate, using the SAME `LOOP_ITER_CAP` the runner's `run_loop`
    /// enforces. The RON front-end has no `Loop` step, so build the plan with a
    /// `Loop` node directly. Declared `max_iters` (100) exceeds the cap (16), so
    /// the count must clamp: `3 agents * min(100, 16) = 48`, never 0.
    #[test]
    fn loop_node_counts_agents_times_capped_iters() {
        use crate::orchestration::graph::{GraphConfig, InputMapper, Predicate};
        use crate::orchestration::workflow::meta::WorkflowMeta;
        use crate::orchestration::workflow::runner::LOOP_ITER_CAP;
        use std::collections::HashMap;

        let mut graph = GraphConfig::empty("refine");
        // 3 inner agents, 100 declared iters — clamps to the cap.
        let loop_agents: Vec<(String, InputMapper)> = vec![
            ("draft".to_string(), InputMapper::PassThrough),
            ("critique".to_string(), InputMapper::PassThrough),
            ("revise".to_string(), InputMapper::PassThrough),
        ];
        // `Never` so the loop never short-circuits — the static estimate is the
        // worst-case capped iteration count.
        graph.add_loop("refine", loop_agents, Predicate::Never, 100);

        let plan = WorkflowPlan {
            graph,
            prompts: HashMap::new(),
            schemas: HashMap::new(),
            schema_defs: HashMap::new(),
            pipelines: HashMap::new(),
            meta: WorkflowMeta {
                name: "loopy".to_string(),
                description: String::new(),
                est_agents: 0,
            },
        };

        let est = estimate(&plan, &Value::Null);
        assert_eq!(
            est.agents,
            3 * LOOP_ITER_CAP,
            "loop must count 3 agents * min(100, {LOOP_ITER_CAP}) iters, not 0"
        );
        assert_ne!(
            est.agents, 0,
            "loop must not be silently under-reported as 0"
        );
        // Sub-cap case: declared iters BELOW the cap use the declared count.
        let mut graph2 = GraphConfig::empty("refine2");
        graph2.add_loop(
            "refine2",
            vec![("solo".to_string(), InputMapper::PassThrough)],
            Predicate::Never,
            4,
        );
        let plan2 = WorkflowPlan {
            graph: graph2,
            prompts: HashMap::new(),
            schemas: HashMap::new(),
            schema_defs: HashMap::new(),
            pipelines: HashMap::new(),
            meta: WorkflowMeta {
                name: "loopy2".to_string(),
                description: String::new(),
                est_agents: 0,
            },
        };
        let est2 = estimate(&plan2, &Value::Null);
        assert_eq!(est2.agents, 4, "1 agent * min(4, cap) = 4 (declared < cap)");
    }

    /// Classic (`over: None`) pipeline stages ARE graph nodes, so they are
    /// counted once each by the node walk — not double-counted via the
    /// pipeline side-table (which only holds `over: Some` pipelines).
    #[test]
    fn classic_pipeline_stages_counted_once() {
        let src = r#"
Workflow(
    meta: (name: "chain"),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", stages: [
            (id: "s1", prompt: "one"),
            (id: "s2", prompt: "two", input: Some("s1")),
        ]),
    ])],
)
"#;
        let p = plan(src);
        // No `over` => not in the pipeline side-table.
        assert!(p.pipelines.is_empty());
        let est = estimate(&p, &Value::Null);
        assert_eq!(est.agents, 2);
        assert!(!est.cardinality_unknown);
    }
}
