//! v0.8.0 Task I (1.D.3) — sub-agent ACL pre-filter integration tests.
//!
//! Phase 22 (22-02 Task 3) — RE-ENABLED. The v0.8.1 U11 note said these
//! would be un-`#[ignore]`d "when a future wave wires a real sub-agent spawn
//! path that constructs `CallActor::SubAgent` and a procedural-memory
//! `LearnedPolicy`". That wave is Phase 22: `AgentSpawner` now stamps
//! `CallActor::SubAgent` on every child engine it builds and hands it the
//! parent's `LearnedPolicy`, and `dispatch_once` consults both again. The
//! pre-filter itself was NOT restored from `52b1ae2~..HEAD` — that revision
//! does not exist, this repository's history begins at a squashed root — it
//! was folded into `filter_tool_calls_by_policy`, which now runs the policy
//! gate first and offers only its survivors to the learned policy.
//!
//! These prove that `AgentExecutorConfig::{actor, learned_policy}` are
//! actually consulted by `dispatch_once` via `AgentNodeExecutor`. The
//! contract:
//!
//! 1. Root actor + deny-everything policy → tool runs (Root bypasses the
//!    pre-filter; policy is sub-agent-only).
//! 2. SubAgent actor + allow-everything policy → tool runs.
//! 3. SubAgent actor + deny-everything policy → tool denied BEFORE
//!    dispatch; result is a policy-deny error, not the MockTool payload.
//! 4. SubAgent actor + Ask policy (empty rules) → falls through to the
//!    normal approval path (auto-approve confirmer → tool runs).
//! 5. SubAgent actor without learned_policy → no pre-filter; tool runs.
//!
//! The "no payload reached" assertions are the load-bearing ones: if
//! `dispatch_once` ignored the new fields the MockTool would execute and
//! the deny assertions would fail.

mod common;

use std::sync::Arc;

use common::{MockTool, auto_approve_confirmer};
use serde_json::json;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;
use wcore_agent::orchestration::graph::{ExecutionGraph, GraphConfig, GraphContext, NodeExecutor};
use wcore_agent::orchestration::node_executor::{AgentExecutorConfig, AgentNodeExecutor, TurnCell};
use wcore_compact::CompactionLevel;
use wcore_permissions::{CallActor, LearnedDecision, LearnedPolicy};
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;

fn tool_use(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input: json!({}),
        extra: None,
    }
}

fn root_cfg(learned_policy: Option<Arc<LearnedPolicy>>) -> AgentExecutorConfig {
    cfg(CallActor::Root, learned_policy)
}

fn sub_agent_cfg(learned_policy: Option<Arc<LearnedPolicy>>) -> AgentExecutorConfig {
    cfg(
        CallActor::SubAgent {
            id: "worker-1".into(),
            parent_id: Some("main".into()),
        },
        learned_policy,
    )
}

fn cfg(actor: CallActor, learned_policy: Option<Arc<LearnedPolicy>>) -> AgentExecutorConfig {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("guarded", "tool-executed", false)));
    AgentExecutorConfig {
        tools: Arc::new(registry),
        confirmer: auto_approve_confirmer(),
        compaction_level: CompactionLevel::Off,
        toon_enabled: false,
        streaming: None,
        tool_budget: None,
        approval: None,
        allow_list: vec![],
        policy_gate: None,
        actor,
        learned_policy,
        cancel: tokio_util::sync::CancellationToken::new(),
        file_write_notifier: None,
    }
}

fn deny_all_policy() -> Arc<LearnedPolicy> {
    let mut p = LearnedPolicy::new();
    p.record(
        "guarded",
        Some("*".to_string()),
        LearnedDecision::DenyAlways,
    );
    Arc::new(p)
}

fn allow_all_policy() -> Arc<LearnedPolicy> {
    let mut p = LearnedPolicy::new();
    p.record(
        "guarded",
        Some("*".to_string()),
        LearnedDecision::AllowAlways,
    );
    Arc::new(p)
}

async fn run_dispatch(cfg: AgentExecutorConfig, call_id: &str) -> Vec<ContentBlock> {
    let calls = vec![tool_use(call_id, "guarded")];
    let cell = Arc::new(TokioMutex::new(TurnCell::new(calls, None)));
    let executor: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
    let graph = GraphConfig::direct("main", serde_json::json!({}));
    let ctx = GraphContext {
        cancel: CancellationToken::new(),
        executor,
    };
    ExecutionGraph::execute(graph, serde_json::Value::Null, ctx)
        .await
        .expect("graph walk must succeed");
    let cell_guard = cell.lock().await;
    cell_guard
        .outcome
        .as_ref()
        .expect("outcome must be populated")
        .as_ref()
        .expect("outcome must be Ok")
        .results
        .clone()
}

fn expect_executed(results: &[ContentBlock]) {
    assert_eq!(results.len(), 1, "expected exactly one result");
    match &results[0] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(
                !*is_error,
                "expected tool to run; got error result: {content}"
            );
            assert_eq!(content, "tool-executed", "MockTool payload must reach LLM");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

fn expect_denied(results: &[ContentBlock]) {
    assert_eq!(results.len(), 1, "expected exactly one result");
    match &results[0] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(
                *is_error,
                "expected deny error result; got success: {content}"
            );
            assert!(
                content.contains("Denied by sub-agent learned policy"),
                "result must carry the sub-agent deny message; got: {content}"
            );
            assert!(
                !content.contains("tool-executed"),
                "MockTool payload must NOT have been produced; got: {content}"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[tokio::test]
async fn root_actor_bypasses_deny_policy() {
    // Even with a deny-everything policy in place, the Root actor
    // bypasses the sub-agent pre-filter — the approval path applies
    // (auto-approve confirmer says yes) and the tool runs.
    let cfg = root_cfg(Some(deny_all_policy()));
    let results = run_dispatch(cfg, "t1").await;
    expect_executed(&results);
}

#[tokio::test]
async fn sub_agent_with_allow_policy_runs_tool() {
    // SubAgent + allow policy → pre-filter says Allow, falls through to
    // the normal approval path, tool runs.
    let cfg = sub_agent_cfg(Some(allow_all_policy()));
    let results = run_dispatch(cfg, "t2").await;
    expect_executed(&results);
}

#[tokio::test]
async fn sub_agent_with_deny_policy_short_circuits() {
    // The primary wiring proof: a SubAgent caller with a deny-everything
    // policy gets an error ToolResult before dispatch — the MockTool
    // payload must never appear.
    let cfg = sub_agent_cfg(Some(deny_all_policy()));
    let results = run_dispatch(cfg, "t3").await;
    expect_denied(&results);
}

#[tokio::test]
async fn sub_agent_ask_policy_falls_through_to_approval() {
    // Empty LearnedPolicy → every evaluate() returns Ask, which the
    // pre-filter treats as "fall through to the normal dispatch path".
    // With an auto-approve confirmer the tool runs.
    let cfg = sub_agent_cfg(Some(Arc::new(LearnedPolicy::new())));
    let results = run_dispatch(cfg, "t4").await;
    expect_executed(&results);
}

#[tokio::test]
async fn sub_agent_without_policy_runs_tool() {
    // SubAgent actor but no learned_policy configured → pre-filter is
    // skipped entirely; normal dispatch path applies and the tool runs.
    let cfg = sub_agent_cfg(None);
    let results = run_dispatch(cfg, "t5").await;
    expect_executed(&results);
}

/// The narrowing-only guarantee, and the only case here that can catch the
/// dangerous direction of this feature.
///
/// The 2026-07-13 frontier gap audit §4 is explicit: learned policy may be
/// wired "only as a narrowing/preapproval aid; it must never override hard
/// denial or managed policy." Cases 1-5 above all check that a DENY denies or
/// that a non-deny lets the tool run — none of them can fail if an
/// `AllowAlways` rule were wired to bypass the policy gate, which is the
/// escalation this feature could plausibly introduce.
///
/// So: a policy gate that denies `guarded` outright, PLUS an allow-everything
/// learned policy on a sub-agent caller. The call must still be denied, and
/// denied BY THE GATE — the learned policy must not be able to resurrect it.
#[tokio::test]
async fn allow_always_cannot_override_the_policy_gate() {
    let mut cfg = sub_agent_cfg(Some(allow_all_policy()));
    // A gate whose parent authority contains no tools at all denies everything.
    cfg.policy_gate = Some(wcore_agent::policy_gate::PolicyGate::from_parent_tools(
        std::iter::empty::<&str>(),
    ));
    let results = run_dispatch(cfg, "t6").await;
    assert_eq!(results.len(), 1, "expected exactly one result");
    match &results[0] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(*is_error, "AllowAlways must not resurrect a gate denial");
            assert!(
                content.contains("Denied by policy:"),
                "the surviving denial must be the GATE's, not the learned policy's; got: {content}"
            );
            assert!(
                !content.contains("tool-executed"),
                "MockTool payload must NOT have been produced; got: {content}"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

/// Ordering control for the case above: with the SAME allow-everything policy
/// and NO gate, the tool runs. One variable — the gate — so the assertion
/// above is shown to be measuring the gate rather than passing for any reason.
#[tokio::test]
async fn the_gate_is_the_variable_in_the_override_test() {
    let cfg = sub_agent_cfg(Some(allow_all_policy()));
    assert!(cfg.policy_gate.is_none());
    let results = run_dispatch(cfg, "t7").await;
    expect_executed(&results);
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// This binary's five cases were `#[ignore]`d from v0.8.1 U11 until Phase 22,
/// so `cargo test --test actor_acl_test` executed 0 of 5 and still exited 0
/// printing `test result: ok`. They are no longer ignored, but the guard is
/// kept, inverted, and made stronger: it now FAILS if any test in this binary
/// is ever `#[ignore]`d back into inertness while a caller declares intent to
/// run the suite.
///
/// It always runs, so this binary can never report success on zero executed
/// tests.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    let ignored = std::process::Command::new(std::env::current_exe().expect("test binary path"))
        .args(["--list", "--ignored"])
        .output();
    if let Ok(output) = ignored {
        let listed = String::from_utf8_lossy(&output.stdout);
        let count = listed
            .lines()
            .filter(|line| line.contains(": test"))
            .count();
        assert_eq!(
            count, 0,
            "this suite's cases were un-#[ignore]d in Phase 22 when the sub-agent \
             ACL pre-filter was wired; {count} are ignored again, which would let \
             the binary exit 0 having proven nothing about the pre-filter"
        );
    }
}
