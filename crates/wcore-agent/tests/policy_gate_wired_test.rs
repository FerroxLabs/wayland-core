//! v0.6.1 CRIT-1 — integration test proving `PolicyGate` is wired into
//! the production `dispatch_once` path via `AgentNodeExecutor`.
//!
//! `policy_gate_test.rs` already verifies that
//! `execute_tool_calls_with_policy_gate` itself consults the
//! `PolicyEngine`. This test proves the one level above: that
//! `AgentExecutorConfig::policy_gate` is actually *consulted by
//! `dispatch_once`*. Before the wiring fix, a deny-all gate set on
//! `AgentExecutorConfig` had zero effect — `dispatch_once` ignored the
//! field entirely and called the budget path directly.

mod common;

use std::sync::Arc;

use common::{MockTool, auto_approve_confirmer};
use serde_json::json;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;
use wcore_agent::orchestration::graph::{ExecutionGraph, GraphConfig, GraphContext, NodeExecutor};
use wcore_agent::orchestration::node_executor::{AgentExecutorConfig, AgentNodeExecutor, TurnCell};
use wcore_agent::policy_gate::PolicyGate;
use wcore_compact::CompactionLevel;
use wcore_permissions::{Actor, CallActor, PolicyEngine};
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

/// Build a deny-all gate (zero grants → every tool denied).
fn deny_all_gate() -> PolicyGate {
    PolicyGate::new(Arc::new(PolicyEngine::new()), Actor::User("default".into()))
}

/// Build an `AgentExecutorConfig` with a registered `MockTool` called
/// "guarded" and the provided gate.
fn cfg_with_gate(gate: Option<PolicyGate>) -> AgentExecutorConfig {
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
        policy_gate: gate,
        actor: CallActor::Root,
        learned_policy: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        file_write_notifier: None,
    }
}

/// Gate is `None` → tool runs, result is the MockTool's payload.
///
/// This is the backwards-compat proof: removing the gate must restore
/// the pre-wiring behaviour (tool executes, returns "tool-executed").
#[tokio::test]
async fn no_gate_tool_executes_via_dispatch_once() {
    let cfg = cfg_with_gate(None);
    let calls = vec![tool_use("t1", "guarded")];
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
    let outcome = cell_guard
        .outcome
        .as_ref()
        .expect("outcome must be populated")
        .as_ref()
        .expect("outcome must be Ok");

    assert_eq!(outcome.results.len(), 1);
    match &outcome.results[0] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(!is_error, "without a gate the tool must succeed");
            assert_eq!(
                content, "tool-executed",
                "MockTool payload must reach the result"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

/// Gate is `Some(deny-all)` → tool is denied before dispatch; result is
/// a policy-deny error, NOT the MockTool's "tool-executed" payload.
///
/// This is the primary wiring proof: if `dispatch_once` ignored the
/// `policy_gate` field the result would be "tool-executed" and this
/// assertion would fail.
#[tokio::test]
async fn deny_all_gate_blocks_tool_via_dispatch_once() {
    let cfg = cfg_with_gate(Some(deny_all_gate()));
    let calls = vec![tool_use("t1", "guarded")];
    let cell = Arc::new(TokioMutex::new(TurnCell::new(calls, None)));
    let executor: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
    let graph = GraphConfig::direct("main", serde_json::json!({}));
    let ctx = GraphContext {
        cancel: CancellationToken::new(),
        executor,
    };

    ExecutionGraph::execute(graph, serde_json::Value::Null, ctx)
        .await
        .expect("graph walk must succeed even when gate denies");

    let cell_guard = cell.lock().await;
    let outcome = cell_guard
        .outcome
        .as_ref()
        .expect("outcome must be populated")
        .as_ref()
        .expect("outcome must be Ok");

    assert_eq!(outcome.results.len(), 1);
    match &outcome.results[0] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(*is_error, "gate must produce an error result");
            assert!(
                content.starts_with("Denied by policy"),
                "result must carry policy-deny message; got: {content}"
            );
            assert!(
                !content.contains("tool-executed"),
                "MockTool must NOT have executed; got: {content}"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}
