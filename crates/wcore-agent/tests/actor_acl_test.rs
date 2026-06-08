//! v0.8.0 Task I (1.D.3) — sub-agent ACL pre-filter integration tests.
//!
//! v0.8.1 U11 — all tests in this file are `#[ignore]`'d because the
//! pre-filter they exercise has been removed from
//! `node_executor::dispatch_once` (it never fired in production:
//! `CallActor::SubAgent` was never constructed and `LearnedPolicy` was
//! never threaded into `AgentExecutorConfig`). The test code remains as
//! a working spec — when a future wave wires a real sub-agent spawn
//! path that constructs `CallActor::SubAgent` and a procedural-memory
//! `LearnedPolicy`, restore the pre-filter from `52b1ae2~..HEAD` and
//! remove the `#[ignore]` annotations.
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
#[ignore = "v0.8.1 U11: pre-filter removed, will re-enable when sub-agent ACL wired"]
async fn root_actor_bypasses_deny_policy() {
    // Even with a deny-everything policy in place, the Root actor
    // bypasses the sub-agent pre-filter — the approval path applies
    // (auto-approve confirmer says yes) and the tool runs.
    let cfg = root_cfg(Some(deny_all_policy()));
    let results = run_dispatch(cfg, "t1").await;
    expect_executed(&results);
}

#[tokio::test]
#[ignore = "v0.8.1 U11: pre-filter removed, will re-enable when sub-agent ACL wired"]
async fn sub_agent_with_allow_policy_runs_tool() {
    // SubAgent + allow policy → pre-filter says Allow, falls through to
    // the normal approval path, tool runs.
    let cfg = sub_agent_cfg(Some(allow_all_policy()));
    let results = run_dispatch(cfg, "t2").await;
    expect_executed(&results);
}

#[tokio::test]
#[ignore = "v0.8.1 U11: pre-filter removed, will re-enable when sub-agent ACL wired"]
async fn sub_agent_with_deny_policy_short_circuits() {
    // The primary wiring proof: a SubAgent caller with a deny-everything
    // policy gets an error ToolResult before dispatch — the MockTool
    // payload must never appear.
    let cfg = sub_agent_cfg(Some(deny_all_policy()));
    let results = run_dispatch(cfg, "t3").await;
    expect_denied(&results);
}

#[tokio::test]
#[ignore = "v0.8.1 U11: pre-filter removed, will re-enable when sub-agent ACL wired"]
async fn sub_agent_ask_policy_falls_through_to_approval() {
    // Empty LearnedPolicy → every evaluate() returns Ask, which the
    // pre-filter treats as "fall through to the normal dispatch path".
    // With an auto-approve confirmer the tool runs.
    let cfg = sub_agent_cfg(Some(Arc::new(LearnedPolicy::new())));
    let results = run_dispatch(cfg, "t4").await;
    expect_executed(&results);
}

#[tokio::test]
#[ignore = "v0.8.1 U11: pre-filter removed, will re-enable when sub-agent ACL wired"]
async fn sub_agent_without_policy_runs_tool() {
    // SubAgent actor but no learned_policy configured → pre-filter is
    // skipped entirely; normal dispatch path applies and the tool runs.
    let cfg = sub_agent_cfg(None);
    let results = run_dispatch(cfg, "t5").await;
    expect_executed(&results);
}
