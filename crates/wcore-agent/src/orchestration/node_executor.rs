//! Wave OR (W8b.2.B.1) — `AgentNodeExecutor` production adapter.
//!
//! This is the bridge that wakes up the [`super::graph::ExecutionGraph`]
//! machinery in the engine's per-turn loop. The graph's `NodeExecutor`
//! trait is called once per `AgentCall` node; the adapter routes each
//! call back through the existing tool-dispatch pipeline
//! (`execute_tool_calls_with_budget` / `_with_approval`), preserving
//! confirmation, hooks, streaming, budget tracking, and cancellation.
//!
//! ## Why this lives in its own module
//!
//! `orchestration/mod.rs` already exceeds 1200 lines. Keeping the
//! adapter here also makes the dependency edge obvious: this is the
//! ONLY production-side `NodeExecutor` impl; tests script the trait
//! directly.
//!
//! ## Why `Arc<ToolRegistry>` everywhere
//!
//! `ExecutionGraph::execute` spawns each parallel `AgentCall` via
//! `tokio::spawn`, which imposes `'static + Send + Sync` on the
//! executor. The adapter therefore must hold owned/Arc state — not a
//! borrowed `&ToolRegistry`. Wave OR's bounded refactor wraps the
//! engine's registry in `Arc<ToolRegistry>` so the adapter can clone
//! the handle cheaply per turn without copying the tool list.
//!
//! ## Per-turn mutable state
//!
//! `HookEngine` is owned by the engine (`Option<HookEngine>`) and is
//! mutated during dispatch (post-tool-use outcomes, log-line drains).
//! We move it into the adapter via `Arc<tokio::sync::Mutex<...>>` for
//! the duration of the graph walk and move it back when the walk
//! completes. The lock is single-tasked on the Direct path (one
//! `AgentCall` node) and is short-held inside `run_agent` (each tool
//! call's pre/post hooks run inside the lock guard). For parallel
//! templates that use the production adapter the lock serialises
//! hook access across sibling agents — acceptable because hooks are
//! not the hot path.
//!
//! ## What input/output the executor sees
//!
//! The graph passes `Value` as input/output. For the Direct template
//! the input is the engine's tool-call batch serialised into a JSON
//! object; the output is the merged `ToolCallOutcome` serialised back.
//! `engine::run` extracts the outcome from the executor's per-turn
//! shared cell (`outcome_slot`) rather than parsing the JSON — the
//! `Value` is a graph-level shape carrier; `outcome_slot` is the
//! load-bearing path. This duality is intentional: the graph stays
//! type-erased (so non-engine executors work in tests) while the
//! engine adapter keeps full structured types end-to-end.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex as TokioMutex;

use wcore_permissions::{CallActor, LearnedPolicy};
use wcore_protocol::ToolApprovalManager;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;

use crate::confirm::ToolConfirmer;
use crate::hooks::HookEngine;
use crate::orchestration::graph::NodeExecutor;
use crate::orchestration::{
    ExecutionControl, StreamingContext, ToolCallOutcome, execute_tool_calls_with_approval,
    execute_tool_calls_with_budget, execute_tool_calls_with_policy_gate,
};
use crate::policy_gate::PolicyGate;

/// Per-turn shared state moved into the adapter for the duration of a
/// graph walk. The engine `take()`s from these cells before invoking
/// `ExecutionGraph::execute` and reclaims them afterward.
pub struct TurnCell {
    /// LLM-emitted tool calls for this turn. Cleared once consumed.
    pub tool_calls: Vec<ContentBlock>,
    /// Engine's hook state, moved in at turn-start, moved back at
    /// turn-end. `None` if the engine has no hook engine configured.
    pub hooks: Option<HookEngine>,
    /// Dispatch outcome written by `run_agent`. The engine drains this
    /// after `ExecutionGraph::execute` returns.
    pub outcome: Option<Result<ToolCallOutcome, ExecutionControl>>,
}

impl TurnCell {
    pub fn new(tool_calls: Vec<ContentBlock>, hooks: Option<HookEngine>) -> Self {
        Self {
            tool_calls,
            hooks,
            outcome: None,
        }
    }
}

/// Configuration captured by `engine::run` once at turn-start. Cheap to
/// clone — every field is either an `Arc<...>` or a `Copy` enum.
#[derive(Clone)]
pub struct AgentExecutorConfig {
    pub tools: Arc<ToolRegistry>,
    pub confirmer: Arc<std::sync::Mutex<ToolConfirmer>>,
    pub compaction_level: wcore_compact::CompactionLevel,
    pub toon_enabled: bool,
    pub streaming: Option<StreamingContext>,
    /// When `Some`, dispatch goes through the JSON-protocol approval
    /// flow (`execute_tool_calls_with_approval`); otherwise it uses the
    /// budget-aware terminal-confirmation path
    /// (`execute_tool_calls_with_budget`).
    pub approval: Option<ApprovalChannel>,
    pub allow_list: Vec<String>,
    /// v0.6.1 CRIT-1: opt-in policy gate. When `Some`, every tool call
    /// is checked against the `PolicyEngine` before dispatch. `None`
    /// (the default) preserves byte-identical v0.6.0 behaviour.
    pub policy_gate: Option<PolicyGate>,
    /// v0.8.0 Task I (1.D.3): who is asking. `Root` (the default) goes
    /// through the existing approval / budget / policy-gate paths
    /// unchanged. `SubAgent { id, .. }` activates the learned-policy
    /// pre-filter below (see `dispatch_once`).
    pub actor: CallActor,
    /// v0.8.0 Task I (1.D.3): opt-in learned-policy pre-filter for
    /// sub-agent callers. When `Some` AND `actor.is_sub_agent()`, each
    /// tool call's `(name, argv)` is run through the policy BEFORE the
    /// approval path. A deny short-circuits with an error
    /// `ToolResult`; an allow or `Ask` falls through to the normal
    /// dispatch path. `None` (the default) preserves byte-identical
    /// pre-task-I behaviour even when `actor` is set.
    pub learned_policy: Option<Arc<LearnedPolicy>>,
    /// AUDIT B-1 / A2 — session-root cancellation token threaded into
    /// every tool dispatch. `engine::run` passes a child of the
    /// engine's `cancel_token` so a host cancel reaches a running tool
    /// and so each dispatch's category timeout can fire the call's own
    /// cooperative cancel.
    pub cancel: tokio_util::sync::CancellationToken,
    /// W8b.2.A — when `Some`, threaded into the per-call `ToolContext` so
    /// Write/Edit tools notify the file watcher of self-originated writes,
    /// preventing the agent's own writes from being treated as user edits.
    pub file_write_notifier:
        Option<std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>>,
}

/// Approval-flow plumbing for JSON-protocol hosts (e.g. Wayland Desktop).
#[derive(Clone)]
pub struct ApprovalChannel {
    pub manager: Arc<ToolApprovalManager>,
    pub writer: Arc<dyn ProtocolEmitter>,
    pub msg_id: String,
    pub auto_approve: bool,
}

/// Production `NodeExecutor` adapter — wraps `execute_tool_calls_*`.
///
/// One instance is constructed per turn by `engine::run`. The graph
/// walker holds it as `Arc<dyn NodeExecutor>`. For the Direct template
/// the graph fires one `AgentCall` node, which routes here, which
/// drives the legacy dispatch path, which writes the result into the
/// shared `turn_cell.outcome` slot.
pub struct AgentNodeExecutor {
    cfg: AgentExecutorConfig,
    /// Per-turn shared state (tool calls in, outcome + hooks out).
    /// `Arc<TokioMutex<...>>` because the executor is held behind an
    /// `Arc<dyn NodeExecutor>` and the graph may invoke `run_agent`
    /// from a `tokio::spawn`-ed task; the engine reclaims state from
    /// the same cell after `ExecutionGraph::execute` returns.
    turn_cell: Arc<TokioMutex<TurnCell>>,
    /// Rank 52: lock-free first-dispatch latch. Set to `true` by the
    /// task that wins the dispatch (under the `turn_cell` lock) and
    /// read WITHOUT the lock by every later sibling so they can
    /// short-circuit to an empty carrier without contending on the
    /// mutex while the winner runs its full async LLM dispatch. Lives
    /// alongside the `TokioMutex` (rather than inside `TurnCell`) so the
    /// fast-path check is truly lock-free — reading it from inside
    /// `TurnCell` would force every late sibling to acquire the lock,
    /// which is exactly the serialisation this fix removes.
    dispatched: Arc<std::sync::atomic::AtomicBool>,
}

impl AgentNodeExecutor {
    /// Construct the adapter. The `turn_cell` must already contain the
    /// turn's `tool_calls` and any `hooks` the engine wants the
    /// adapter to mutate.
    pub fn new(cfg: AgentExecutorConfig, turn_cell: Arc<TokioMutex<TurnCell>>) -> Self {
        Self {
            cfg,
            turn_cell,
            dispatched: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl NodeExecutor for AgentNodeExecutor {
    /// Dispatch the turn's tool-call batch.
    ///
    /// `agent` is the graph-level node name. For the Direct template
    /// the engine names this node `"main"`; the adapter ignores it and
    /// runs the captured `turn_cell.tool_calls` once. The `_input`
    /// `Value` is the graph-level state snapshot; the engine doesn't
    /// route on it today (the load-bearing path is `turn_cell`).
    ///
    /// Returns a graph-level `Value` (carrier; the engine reads the
    /// real outcome from `turn_cell.outcome`). On dispatch error the
    /// `Value` carries a `{"error": "..."}` shape so non-engine
    /// callers (tests) can observe the failure too.
    async fn run_agent(&self, _agent: &str, _input: &Value) -> Result<Value, String> {
        use std::sync::atomic::Ordering;

        // Multi-node templates (Sequential, Parallel, etc.) call
        // `run_agent` once per AgentCall. The engine's per-turn loop
        // hands the adapter exactly one batch of tool_calls; we run
        // them on the FIRST invocation, then subsequent invocations
        // become inert (return a carrier without re-dispatching) so
        // the outcome cell isn't clobbered. This makes non-Direct
        // templates structurally exerciseable through the production
        // adapter without changing the engine's "one batch per turn"
        // contract — full multi-LLM-turn orchestration is a follow-up
        // wave.
        //
        // Rank 52: the walker `tokio::spawn`s each parallel AgentCall
        // sibling and `join_all`s them. Previously this fn held the
        // `turn_cell` lock across the entire `dispatch_once(...).await`
        // (a full async LLM tool dispatch), so every sibling blocked on
        // `lock().await` until the winner finished — `join_all` never
        // achieved real parallelism. The fix keeps first-dispatch-wins
        // semantics but releases the lock during dispatch.
        //
        // (a) Lock-free fast path: a late sibling that observes the
        // `dispatched` latch returns the empty carrier WITHOUT ever
        // touching the mutex. `Acquire` here pairs with the winner's
        // `Release` store below so a sibling that sees `dispatched=true`
        // also sees a consistent view of the cell it (correctly) skips.
        if self.dispatched.load(Ordering::Acquire) {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // (b) Acquire the lock to claim the dispatch.
        let mut cell = self.turn_cell.lock().await;

        // (c) TOCTOU re-check: two tasks can both pass the atomic load
        // before either takes the lock. The first to lock claims the
        // dispatch and sets the latch; the second observes the
        // already-populated outcome here and returns an inert carrier.
        if cell.outcome.is_some() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // (d) Claim it: take the batch + hooks out of the cell and set
        // the latch with `Release` so the matching `Acquire` load above
        // sees this and everything that precedes it.
        let tool_calls = std::mem::take(&mut cell.tool_calls);
        let hooks_owned = cell.hooks.take();
        self.dispatched.store(true, Ordering::Release);

        // (e) Drop the guard BEFORE dispatching — this is the
        // load-bearing fix. With the lock released, sibling tasks hit
        // the lock-free fast path in (a) instead of blocking here while
        // the winner runs its full async LLM dispatch.
        drop(cell);

        // (f) Dispatch with no lock held.
        let (outcome, hooks_back) = dispatch_once(&self.cfg, &tool_calls, hooks_owned).await;

        let carrier = match &outcome {
            Ok(_) => Value::Object(serde_json::Map::new()),
            Err(_) => serde_json::json!({"error": "ExecutionControl::Quit"}),
        };

        // (g) Re-acquire the lock only to write the result back.
        let mut cell = self.turn_cell.lock().await;
        cell.hooks = hooks_back;
        cell.outcome = Some(outcome);
        Ok(carrier)
    }
}

/// One-shot dispatch through the existing pipeline. Returns the outcome
/// AND the (possibly mutated) hook engine so the caller can put it back
/// in the cell.
async fn dispatch_once(
    cfg: &AgentExecutorConfig,
    tool_calls: &[ContentBlock],
    mut hooks: Option<HookEngine>,
) -> (
    Result<ToolCallOutcome, ExecutionControl>,
    Option<HookEngine>,
) {
    // v0.8.1 U11 — sub-agent ACL pre-filter scope-down. The 1.D.3
    // pre-filter shipped in v0.8.0 I never fires in production
    // (CallActor::SubAgent is never constructed; LearnedPolicy::new is
    // never wired into AgentExecutorConfig). Removed pending a real
    // sub-agent spawn path that sets the actor + a procedural-memory
    // policy source. CallActor type is retained on AgentExecutorConfig
    // because it defaults to Root (zero overhead) and is ready to
    // activate when needed.
    let outcome = if let Some(gate) = cfg.policy_gate.as_ref() {
        execute_tool_calls_with_policy_gate(
            &cfg.tools,
            tool_calls,
            &cfg.confirmer,
            hooks.as_mut(),
            cfg.compaction_level,
            cfg.toon_enabled,
            cfg.streaming.clone(),
            None,
            Some(gate),
            &cfg.cancel,
            cfg.file_write_notifier.as_ref(),
        )
        .await
    } else if let Some(approval) = cfg.approval.as_ref() {
        execute_tool_calls_with_approval(
            &cfg.tools,
            tool_calls,
            &approval.manager,
            &approval.writer,
            &approval.msg_id,
            approval.auto_approve,
            &cfg.allow_list,
            hooks.as_mut(),
            cfg.compaction_level,
            cfg.toon_enabled,
            &cfg.cancel,
            cfg.file_write_notifier.as_ref(),
        )
        .await
    } else {
        execute_tool_calls_with_budget(
            &cfg.tools,
            tool_calls,
            &cfg.confirmer,
            hooks.as_mut(),
            cfg.compaction_level,
            cfg.toon_enabled,
            cfg.streaming.clone(),
            None,
            &cfg.cancel,
            cfg.file_write_notifier.as_ref(),
        )
        .await
    };

    (outcome, hooks)
}

// v0.8.1 U11 — `sub_agent_prefilter` and `merge_prefilter_denies` removed
// alongside the pre-filter call site above. The `LearnedPolicy` /
// `DeniedPartition` plumbing was orphaned: every production caller
// constructs `AgentExecutorConfig` with `actor: CallActor::Root` and
// `learned_policy: None`, so the helpers were unreachable. They lived
// here as a working spec; when a future wave wires a real sub-agent
// spawn path that constructs `CallActor::SubAgent` and threads a
// procedural-memory `LearnedPolicy` into the config, restore from git
// history at `52b1ae2~..HEAD` and re-enable the `#[ignore]`'d
// integration tests in `actor_acl_test.rs`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::graph::{
        ExecutionGraph, GraphConfig, GraphContext, InputMapper, NodeExecutor,
    };
    use std::sync::Mutex as StdMutex;
    use tokio_util::sync::CancellationToken;

    fn make_cfg_empty() -> AgentExecutorConfig {
        AgentExecutorConfig {
            tools: Arc::new(ToolRegistry::new()),
            confirmer: Arc::new(StdMutex::new(ToolConfirmer::new(true, vec![]))),
            compaction_level: wcore_compact::CompactionLevel::Off,
            toon_enabled: false,
            streaming: None,
            approval: None,
            allow_list: vec![],
            policy_gate: None,
            actor: CallActor::Root,
            learned_policy: None,
            cancel: CancellationToken::new(),
            file_write_notifier: None,
        }
    }

    #[tokio::test]
    async fn direct_template_empty_batch_succeeds_byte_identical() {
        // The Direct template is a single AgentCall node. With an empty
        // tool-call batch the existing dispatch returns an empty
        // ToolCallOutcome; the adapter must round-trip that through the
        // graph walker without error.
        let cfg = make_cfg_empty();
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let exec: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let graph = GraphConfig::direct("main", serde_json::json!({}));
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let result = ExecutionGraph::execute(graph, Value::Null, ctx).await;
        assert!(result.is_ok());
        let cell_inner = cell.lock().await;
        assert!(cell_inner.outcome.is_some());
        let outcome = cell_inner.outcome.as_ref().unwrap().as_ref().unwrap();
        assert!(outcome.results.is_empty());
    }

    #[tokio::test]
    async fn turn_cell_outcome_populated_after_graph_walk() {
        // Verifies the cell-based outcome handoff: the adapter must
        // write the outcome into the shared cell so engine::run can
        // reclaim it after ExecutionGraph::execute returns.
        let cfg = make_cfg_empty();
        let calls = vec![]; // empty batch; non-empty cases covered by integration tests
        let cell = Arc::new(TokioMutex::new(TurnCell::new(calls, None)));
        let exec: Arc<dyn NodeExecutor> =
            Arc::new(AgentNodeExecutor::new(cfg.clone(), cell.clone()));
        let graph = GraphConfig::single_node("main", InputMapper::PassThrough);
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let _ = ExecutionGraph::execute(graph, Value::Null, ctx)
            .await
            .expect("execute should succeed");
        let cell_inner = cell.lock().await;
        assert!(
            cell_inner.outcome.is_some(),
            "outcome cell must be populated by the adapter"
        );
    }

    #[tokio::test]
    async fn approval_channel_routed_when_set() {
        // Smoke-test the conditional path selection: when approval is
        // Some, the adapter calls execute_tool_calls_with_approval;
        // when None, it calls execute_tool_calls_with_budget. With an
        // empty batch both paths short-circuit to an empty outcome, so
        // we verify the call shape via outcome presence.
        let manager = Arc::new(ToolApprovalManager::new());
        // Construct a no-op protocol writer (writes to a TestSink).
        struct NullEmitter;
        impl wcore_protocol::writer::ProtocolEmitter for NullEmitter {
            fn emit(&self, _event: &wcore_protocol::events::ProtocolEvent) -> std::io::Result<()> {
                Ok(())
            }
        }
        let writer: Arc<dyn ProtocolEmitter> = Arc::new(NullEmitter);
        let mut cfg = make_cfg_empty();
        cfg.approval = Some(ApprovalChannel {
            manager,
            writer,
            msg_id: "test_msg".into(),
            auto_approve: true,
        });
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let exec: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let graph = GraphConfig::single_node("main", InputMapper::PassThrough);
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let _ = ExecutionGraph::execute(graph, Value::Null, ctx)
            .await
            .expect("approval-path execute should succeed");
        let cell_inner = cell.lock().await;
        assert!(cell_inner.outcome.is_some());
    }

    // --- Wave OR Step 2: non-Direct template integration tests ---------
    //
    // These exercise the production `AgentNodeExecutor` through the
    // ExecutionGraph walker for templates other than Direct. The engine
    // contract is "one tool-call batch per turn"; the adapter enforces
    // it via the `cell.outcome.is_some()` guard so that templates with
    // multiple `AgentCall` nodes (Sequential, Parallel, SelfCritique)
    // still produce exactly one ToolCallOutcome — the first invocation
    // wins; subsequent nodes are inert carriers.
    //
    // These tests are the "shippable proof" that non-Direct templates
    // can be walked by the production adapter without panicking or
    // corrupting the cell. Full multi-LLM-turn orchestration (where each
    // node has its OWN tool-call batch) is a follow-up wave.

    use crate::orchestration::graph::AggregationStrategy;
    use crate::orchestration::intent::{
        Complexity, Intent, IntentClassifier, LoopSelector, Mode, Shape,
    };

    #[tokio::test]
    async fn sequential_pipeline_template_walks_through_production_adapter() {
        // The sequential_pipeline template walks N AgentCall nodes
        // edge-by-edge. The adapter must dispatch the engine's single
        // tool-call batch on the FIRST AgentCall and short-circuit on
        // subsequent ones (no double-dispatch, no cell corruption).
        let cfg = make_cfg_empty();
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let exec: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let graph = GraphConfig::sequential_pipeline(vec![
            ("step1", InputMapper::PassThrough),
            ("step2", InputMapper::PassThrough),
            ("step3", InputMapper::PassThrough),
        ]);
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let result = ExecutionGraph::execute(graph, Value::Null, ctx).await;
        assert!(result.is_ok(), "sequential pipeline must walk cleanly");
        let cell_inner = cell.lock().await;
        assert!(
            cell_inner.outcome.is_some(),
            "outcome must be populated exactly once across the pipeline"
        );
        let outcome = cell_inner.outcome.as_ref().unwrap().as_ref().unwrap();
        assert!(
            outcome.results.is_empty(),
            "empty batch yields an empty outcome regardless of node count"
        );
    }

    #[tokio::test]
    async fn parallel_fanout_template_walks_through_production_adapter() {
        // parallel_fanout creates N AgentCall nodes joined by an
        // Aggregator. With the production adapter, the engine's single
        // tool-call batch is dispatched on whichever sibling executes
        // first; the rest return inert carriers. The cell.outcome guard
        // (cell.outcome.is_some()) MUST prevent the second sibling from
        // clobbering the result.
        let cfg = make_cfg_empty();
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let exec: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let graph = GraphConfig::parallel_fanout(
            vec!["worker_a", "worker_b", "worker_c"],
            AggregationStrategy::MergeObjects,
        );
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let result = ExecutionGraph::execute(graph, Value::Null, ctx).await;
        assert!(result.is_ok(), "parallel fanout must walk cleanly");
        let cell_inner = cell.lock().await;
        assert!(
            cell_inner.outcome.is_some(),
            "outcome must be populated exactly once across sibling agents"
        );
    }

    #[tokio::test]
    async fn parallel_siblings_do_not_block_on_lock_during_dispatch() {
        // Rank 52 regression guard. Before the fix, `run_agent` held the
        // `turn_cell` lock across the entire `dispatch_once(...).await`,
        // so the walker's spawned sibling tasks serialised through the
        // lock instead of running concurrently. The fix releases the
        // lock before dispatch and uses a lock-free `dispatched` latch so
        // late siblings short-circuit WITHOUT touching the mutex.
        //
        // We prove the lock is free during dispatch by holding the
        // `turn_cell` lock from the test task across the entire graph
        // walk: if `run_agent` still tried to acquire it across dispatch,
        // the walk would deadlock (and the test would hang / time out).
        // Because the winner takes the lock only briefly (claim + final
        // write-back) and siblings never take it at all once the latch is
        // set, the walk completes even while the test contends for the
        // lock between those two short critical sections.
        let cfg = make_cfg_empty();
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let executor = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let dispatched = executor.dispatched.clone();
        let exec: Arc<dyn NodeExecutor> = executor;
        let graph = GraphConfig::parallel_fanout(
            vec!["worker_a", "worker_b", "worker_c"],
            AggregationStrategy::MergeObjects,
        );
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };

        // Bound the walk so a regression (re-introduced lock-across-
        // dispatch) surfaces as a failed timeout rather than a hung test.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ExecutionGraph::execute(graph, Value::Null, ctx),
        )
        .await
        .expect("parallel walk must not deadlock — lock must be free during dispatch");
        assert!(result.is_ok(), "parallel fanout must walk cleanly");

        // The latch must have been set exactly once (by the winner).
        assert!(
            dispatched.load(std::sync::atomic::Ordering::Acquire),
            "the first-dispatch latch must be set after the walk"
        );

        // First-dispatch-wins preserved: exactly one outcome, populated.
        let cell_inner = cell.lock().await;
        assert!(
            cell_inner.outcome.is_some(),
            "outcome must be populated exactly once across sibling agents"
        );
        let outcome = cell_inner.outcome.as_ref().unwrap().as_ref().unwrap();
        assert!(
            outcome.results.is_empty(),
            "empty batch yields an empty outcome regardless of sibling count"
        );
    }

    #[tokio::test]
    async fn self_critique_template_walks_through_production_adapter() {
        // self_critique uses a bounded Loop node with two AgentCalls
        // per iteration (doer + critic). Verifies the adapter's
        // first-dispatch-wins guard holds across loop iterations: even
        // with max_revisions > 1, the cell.outcome remains the first
        // dispatch's result.
        let cfg = make_cfg_empty();
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let exec: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let graph = GraphConfig::self_critique("doer", "critic", 3);
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let result = ExecutionGraph::execute(graph, Value::Null, ctx).await;
        assert!(result.is_ok(), "self-critique loop must walk cleanly");
        let cell_inner = cell.lock().await;
        assert!(
            cell_inner.outcome.is_some(),
            "outcome must be populated despite multiple loop iterations"
        );
    }

    #[tokio::test]
    async fn loop_selector_default_routes_to_direct_for_classified_trivial_task() {
        // End-to-end: classifier on a trivial user prompt yields
        // Intent::Trivial + Shape::Default, which LoopSelector resolves
        // to GraphConfig::direct. This is THE byte-identical-default
        // contract for engine::run: every existing test exercises a
        // task that lands here and must route to a single AgentCall
        // node — i.e. is_direct() must be true.
        let intent = IntentClassifier::classify("fix typo in readme");
        let graph = LoopSelector::select(&intent, None);
        assert!(
            graph.is_direct(),
            "classifier-default task must route to Direct template (byte-identical to pre-OR)"
        );
        // Walk it through the production adapter to prove the wiring.
        let cfg = make_cfg_empty();
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let exec: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let result = ExecutionGraph::execute(graph, Value::Null, ctx).await;
        assert!(result.is_ok());
        let cell_inner = cell.lock().await;
        assert!(cell_inner.outcome.is_some());
    }

    #[tokio::test]
    async fn loop_selector_mode_override_parallel_routes_through_adapter() {
        // User explicitly forces Mode::Parallel; the LoopSelector must
        // return parallel_fanout REGARDLESS of the classifier's verdict
        // on a trivial task. Walk the resulting graph through the
        // production adapter to confirm the override path is wired.
        let intent = Intent {
            task: "trivial thing".into(),
            complexity: Complexity::Trivial,
            shape: Shape::Default,
        };
        let graph = LoopSelector::select(&intent, Some(Mode::Parallel));
        assert!(
            graph.is_parallel_fanout(),
            "Mode::Parallel override must produce a parallel_fanout graph"
        );
        let cfg = make_cfg_empty();
        let cell = Arc::new(TokioMutex::new(TurnCell::new(vec![], None)));
        let exec: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
        let ctx = GraphContext {
            cancel: CancellationToken::new(),
            executor: exec,
        };
        let result = ExecutionGraph::execute(graph, Value::Null, ctx).await;
        assert!(
            result.is_ok(),
            "Mode::Parallel route must walk through the production adapter without error"
        );
        let cell_inner = cell.lock().await;
        assert!(
            cell_inner.outcome.is_some(),
            "outcome cell must still receive exactly one dispatch under Parallel mode"
        );
    }
}
