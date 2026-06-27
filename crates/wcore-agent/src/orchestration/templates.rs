//! W8b.2.B Task C.2 — graph template factories.
//!
//! Eight constructors that build a [`GraphConfig`] for a recurring
//! coordination shape. Tests live in
//! `crates/wcore-agent/tests/orchestration_templates_test.rs`.
//!
//! Templates are stateless factory functions on [`GraphConfig`]; the
//! cycle-bearing ones (`iterative_loop`, `self_critique`) use
//! [`Node::Loop`] to keep the rest of the walker acyclic.

// Wave RB STABILITY — `parking_lot::Mutex` (no poisoning). The
// critical section here is a single `.take()` on an Option, which
// cannot leave the cell in an invalid state on unwind.
use parking_lot::Mutex;

use serde_json::Value;

use super::graph::{
    AggregationStrategy, GraphConfig, GraphResult, InputMapper, Predicate, ProviderPin,
};

/// Boxed replan closure for [`AdaptiveConfig`]. Returns `Some(new_cfg)`
/// to trigger a replacement graph after the initial run, or `None` to
/// keep the initial result.
pub type ReplanFn = Box<dyn Fn(&GraphResult) -> Option<GraphConfig> + Send + Sync>;

impl GraphConfig {
    /// Direct: a single agent called with the supplied input. Used as
    /// the default template when intent classification picks
    /// `Intent::Direct` (C.3).
    pub fn direct(agent: &str, input: Value) -> Self {
        let mut g = Self::empty(agent);
        g.add_agent(agent, InputMapper::Literal { value: input });
        g
    }

    /// Sequential pipeline: each step's output state feeds the next.
    /// `steps` is `(agent_name, input_mapper)`; the mapper governs how
    /// each step reads from the running state (typically
    /// `PassThrough`).
    pub fn sequential_pipeline(steps: Vec<(&str, InputMapper)>) -> Self {
        assert!(!steps.is_empty(), "sequential_pipeline requires ≥1 step");
        let first = steps[0].0.to_string();
        let mut g = Self::empty(first.clone());
        for (name, mapper) in &steps {
            g.add_agent(*name, mapper.clone());
        }
        for window in steps.windows(2) {
            g.add_edge(window[0].0, window[1].0, None);
        }
        g
    }

    /// Parallel fanout: all `agents` run concurrently from a synthetic
    /// passthrough root, then funnel into an `Aggregator` whose
    /// `strategy` produces the merged state.
    pub fn parallel_fanout(agents: Vec<&str>, joiner: AggregationStrategy) -> Self {
        assert!(!agents.is_empty(), "parallel_fanout requires ≥1 agent");
        let root = "__fan_root__".to_string();
        let join = "__fan_join__".to_string();
        let mut g = Self::empty(root.clone());
        g.add_passthrough(&root);
        for name in &agents {
            g.add_agent(*name, InputMapper::PassThrough);
            g.add_edge(&root, *name, None);
        }
        g.add_aggregator(&join, joiner);
        for name in &agents {
            g.add_edge(*name, &join, None);
        }
        g
    }

    /// Bounded iterative loop: invokes `agent` repeatedly, sharing
    /// state across iterations, stopping on `done_check` or
    /// `max_iters` (whichever fires first).
    pub fn iterative_loop(agent: &str, done_check: Predicate, max_iters: usize) -> Self {
        let id = "__loop__".to_string();
        let mut g = Self::empty(id.clone());
        g.add_loop(
            id,
            vec![(agent.to_string(), InputMapper::PassThrough)],
            done_check,
            max_iters,
        );
        g
    }

    /// Hierarchical delegation: a planner emits a plan, a single
    /// worker fans out across the planned subtasks (the worker
    /// receives the running state, so it can consume `state["plan"]`),
    /// and an integrator merges outcomes. For now this is a
    /// 3-stage linear chain (planner → worker → integrator). True
    /// dynamic per-subtask fan-out lives in a follow-up wave once the
    /// agent runtime can express it.
    pub fn hierarchical_delegation(planner: &str, worker_agent: &str, integrator: &str) -> Self {
        Self::sequential_pipeline(vec![
            (planner, InputMapper::PassThrough),
            (worker_agent, InputMapper::PassThrough),
            (integrator, InputMapper::PassThrough),
        ])
    }

    /// Multi-agent consensus: every proposer runs concurrently and
    /// emits a `vote` field; the `Collect` state reducer accumulates
    /// every proposer's `vote` into an array on `state["vote"]`. The
    /// judge then sees that array and emits the winner.
    pub fn multi_agent_consensus(proposers: Vec<&str>, judge: &str) -> Self {
        assert!(!proposers.is_empty(), "consensus requires ≥1 proposer");
        let root = "__cons_root__".to_string();
        let join = "__cons_join__".to_string();
        let mut g = Self::empty(root.clone());
        g.add_passthrough(&root);
        for name in &proposers {
            g.add_agent(*name, InputMapper::PassThrough);
            g.add_edge(&root, *name, None);
        }
        g.add_aggregator(&join, AggregationStrategy::MergeObjects);
        for name in &proposers {
            g.add_edge(*name, &join, None);
        }
        g.add_agent(judge, InputMapper::PassThrough);
        g.add_edge(&join, judge, None);
        // Reducer: each proposer's `vote` becomes one element of an
        // array on state["vote"]. Judge reads that array.
        g.state_reducers
            .insert("vote".to_string(), super::graph::StateReducer::Collect);
        g
    }

    /// Crucible Mixture-of-Providers topology: PassThrough root → one pinned
    /// `AgentCall` per proposer (provider/model written into `node_providers`)
    /// → a marker `aggregator_id` `AgentCall` node the runner intercepts for
    /// runner-phase synthesis. Built on `parallel_fanout`'s STRUCTURE (NOT
    /// `multi_agent_consensus`'s `vote` semantics): each proposer is pinned to
    /// its own provider, and the aggregator is a sink the runner fuses.
    ///
    /// `proposers` is `(node_id, ProviderPin, prompt)`. The prompt currently
    /// rides through state as `PassThrough`; the runner layer is responsible
    /// for routing the council task to each proposer.
    pub fn mixture_of_providers(
        proposers: &[(String, ProviderPin, String)],
        aggregator_id: &str,
    ) -> Self {
        assert!(
            !proposers.is_empty(),
            "mixture_of_providers requires ≥1 proposer"
        );
        let root = "__mop_root__".to_string();
        let mut g = Self::empty(root.clone());
        g.add_passthrough(&root);
        for (node_id, pin, _prompt) in proposers {
            g.add_agent(node_id.clone(), InputMapper::PassThrough);
            g.set_node_provider(node_id, pin.clone());
            g.add_edge(&root, node_id.clone(), None);
        }
        // The aggregator is a marker `AgentCall` node the runner intercepts to
        // perform runner-phase synthesis over the collected proposals.
        g.add_agent(aggregator_id, InputMapper::PassThrough);
        for (node_id, _pin, _prompt) in proposers {
            g.add_edge(node_id.clone(), aggregator_id, None);
        }
        g
    }

    /// Self-critique loop: each iteration runs `doer` then `critic`
    /// in sequence; stops when `state["good_enough"] == true` or
    /// `max_revisions` iterations have passed.
    pub fn self_critique(doer: &str, critic: &str, max_revisions: usize) -> Self {
        let id = "__crit__".to_string();
        let mut g = Self::empty(id.clone());
        g.add_loop(
            id,
            vec![
                (doer.to_string(), InputMapper::PassThrough),
                (critic.to_string(), InputMapper::PassThrough),
            ],
            Predicate::StateEquals {
                path: "good_enough".to_string(),
                value: Value::Bool(true),
            },
            max_revisions,
        );
        g
    }

    /// Adaptive: run `initial`; if the post-run `GraphResult` satisfies
    /// `replan_on_failure` (returning `Some(new_config)`), execute the
    /// replacement and use its result. Otherwise return the initial
    /// result as-is.
    ///
    /// The dispatch is encoded by wrapping the two configs inside a
    /// synthetic adapter node sequence. To keep the walker simple we
    /// stash the replan closure in a thread-local-friendly
    /// `AdaptiveBox` accessible via [`GraphConfig::take_adaptive`]
    /// (used by `ExecutionGraph::execute_adaptive`).
    ///
    /// In this sub-wave we keep adaptive minimal: the caller is
    /// `execute_adaptive`, which inspects the `is_adaptive()` flag
    /// (set via `state["__adaptive__"]`).
    pub fn adaptive(initial: GraphConfig, replan_on_failure: ReplanFn) -> AdaptiveConfig {
        AdaptiveConfig {
            initial,
            replan: Mutex::new(Some(replan_on_failure)),
        }
    }
}

/// Wrapper produced by [`GraphConfig::adaptive`]. Run with
/// [`AdaptiveConfig::execute`] so the replan callback can fire after
/// the initial walk. Kept distinct from the simple [`GraphConfig`]
/// surface to make the contract obvious at the call site.
pub struct AdaptiveConfig {
    pub initial: GraphConfig,
    /// `take()`-on-use so we don't require Clone on the boxed closure.
    pub(crate) replan: Mutex<Option<ReplanFn>>,
}

impl AdaptiveConfig {
    /// Adaptive runner: run `initial`, then — if the replan closure
    /// returns `Some(new_cfg)` — construct a fresh `GraphContext`
    /// via `mk_ctx` and run the replacement. The factory exists
    /// because `GraphContext` is not `Clone` (its `executor` is an
    /// `Arc<dyn NodeExecutor>` but the surrounding context holds
    /// other unique state).
    pub async fn execute_with_factory<F>(
        self,
        initial_state: Value,
        ctx: super::graph::GraphContext,
        mk_ctx: F,
    ) -> Result<GraphResult, super::graph::GraphError>
    where
        F: FnOnce() -> super::graph::GraphContext,
    {
        use super::graph::ExecutionGraph;

        let replan = self.replan.lock().take();
        let first = ExecutionGraph::execute(self.initial, initial_state.clone(), ctx).await?;
        if let Some(closure) = replan
            && let Some(new_cfg) = closure(&first)
        {
            let second = ExecutionGraph::execute(new_cfg, initial_state, mk_ctx()).await?;
            return Ok(second);
        }
        Ok(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixture_of_providers_pins_each_proposer() {
        let g = GraphConfig::mixture_of_providers(
            &[
                (
                    "p_openai".into(),
                    ProviderPin {
                        provider: Some("openai".into()),
                        model: None,
                    },
                    "task".into(),
                ),
                (
                    "p_anthropic".into(),
                    ProviderPin {
                        provider: Some("anthropic".into()),
                        model: None,
                    },
                    "task".into(),
                ),
            ],
            "synth",
        );
        assert_eq!(
            g.node_providers["p_openai"].provider.as_deref(),
            Some("openai")
        );
        assert_eq!(
            g.node_providers["p_anthropic"].provider.as_deref(),
            Some("anthropic")
        );
        // The aggregator marker node exists in the graph.
        assert!(g.nodes.iter().any(|(id, _)| id == "synth"));
    }
}
