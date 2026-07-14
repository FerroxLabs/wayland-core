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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{MockTool, auto_approve_confirmer};
use serde_json::json;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;
use wcore_agent::confirm::ToolConfirmer;
use wcore_agent::hooks::{Hook, HookAction, HookEngine};
use wcore_agent::orchestration::graph::{ExecutionGraph, GraphConfig, GraphContext, NodeExecutor};
use wcore_agent::orchestration::node_executor::{AgentExecutorConfig, AgentNodeExecutor, TurnCell};
use wcore_agent::orchestration::{ExecutionControl, execute_tool_calls_with_approval};
use wcore_agent::policy_gate::PolicyGate;
use wcore_compact::CompactionLevel;
use wcore_config::hooks::HooksConfig;
use wcore_permissions::{Action, Actor, CallActor, Permission, PolicyEngine, Resource};
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::events::ToolCategory;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_protocol::{
    ToolApprovalManager,
    commands::{ApprovalScope, SessionMode},
};
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;
use wcore_types::tool::ToolResult;

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

fn allow_guarded_gate() -> PolicyGate {
    gate_allowing("guarded")
}

fn gate_allowing(name: &str) -> PolicyGate {
    let actor = Actor::User("default".into());
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: actor.clone(),
        resource: Resource::Tool(name.into()),
        action: Action::Invoke,
    });
    PolicyGate::new(Arc::new(engine), actor)
}

struct CountingTool {
    name: &'static str,
    executions: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Counts executions"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> ToolResult {
        self.executions.fetch_add(1, Ordering::SeqCst);
        ToolResult {
            content: format!("{}-executed", self.name),
            is_error: false,
        }
    }
}

#[derive(Default)]
struct CapturingEmitter {
    events: Mutex<Vec<ProtocolEvent>>,
}

impl ProtocolEmitter for CapturingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        self.events
            .lock()
            .expect("event capture lock")
            .push(event.clone());
        Ok(())
    }
}

struct ResolvingEmitter {
    events: Mutex<Vec<ProtocolEvent>>,
    manager: Arc<ToolApprovalManager>,
    approve: bool,
    resolved_synchronously: std::sync::atomic::AtomicBool,
}

impl ProtocolEmitter for ResolvingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        self.events
            .lock()
            .expect("event capture lock")
            .push(event.clone());
        if let ProtocolEvent::ToolRequest { call_id, .. } = event {
            self.resolved_synchronously.store(
                self.manager
                    .resolve_host(call_id, self.approve, ApprovalScope::Once, None),
                Ordering::SeqCst,
            );
        }
        Ok(())
    }
}

struct FailingEmitter;

impl ProtocolEmitter for FailingEmitter {
    fn emit(&self, _event: &ProtocolEvent) -> std::io::Result<()> {
        Err(std::io::Error::other("host disconnected"))
    }
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

async fn run_guarded_call(cfg: AgentExecutorConfig) -> ContentBlock {
    run_guarded_call_with_hooks(cfg, None).await
}

async fn run_guarded_call_with_hooks(
    cfg: AgentExecutorConfig,
    hooks: Option<HookEngine>,
) -> ContentBlock {
    run_calls_with_hooks(cfg, vec![tool_use("t1", "guarded")], hooks)
        .await
        .into_iter()
        .next()
        .expect("one tool result")
}

async fn run_calls_with_hooks(
    cfg: AgentExecutorConfig,
    calls: Vec<ContentBlock>,
    hooks: Option<HookEngine>,
) -> Vec<ContentBlock> {
    let cell = Arc::new(TokioMutex::new(TurnCell::new(calls, hooks)));
    let executor: Arc<dyn NodeExecutor> = Arc::new(AgentNodeExecutor::new(cfg, cell.clone()));
    let ctx = GraphContext {
        cancel: CancellationToken::new(),
        executor,
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ExecutionGraph::execute(
            GraphConfig::direct("main", serde_json::json!({})),
            serde_json::Value::Null,
            ctx,
        ),
    )
    .await
    .expect("graph walk timed out")
    .expect("graph walk must succeed");

    let cell = cell.lock().await;
    cell.outcome
        .as_ref()
        .expect("outcome must be populated")
        .as_ref()
        .expect("outcome must be Ok")
        .results
        .clone()
}

struct MutatingHook;

#[async_trait::async_trait]
impl Hook for MutatingHook {
    fn name(&self) -> &str {
        "mutating-hook"
    }

    async fn pre_tool_use(&self, _tool: &str, _input: &serde_json::Value) -> HookAction {
        HookAction::ModifyInput(json!({"changed_after_approval": true}))
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

#[tokio::test]
async fn policy_allow_still_honors_protocol_denial() {
    let manager = Arc::new(ToolApprovalManager::new());
    let emitter = Arc::new(ResolvingEmitter {
        events: Mutex::new(Vec::new()),
        manager: manager.clone(),
        approve: false,
        resolved_synchronously: std::sync::atomic::AtomicBool::new(false),
    });
    let mut cfg = cfg_with_gate(Some(allow_guarded_gate()));
    cfg.approval = Some(wcore_agent::orchestration::node_executor::ApprovalChannel {
        manager: manager.clone(),
        writer: emitter.clone(),
        msg_id: "policy-and-approval".into(),
    });

    let result = run_guarded_call(cfg).await;
    assert!(
        emitter.resolved_synchronously.load(Ordering::SeqCst),
        "pending approval must exist before ToolRequest emission"
    );

    match &result {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(*is_error, "host denial must remain terminal");
            assert_eq!(content, "Tool denied: denied by host");
            assert!(!content.contains("tool-executed"));
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }

    let events = emitter.events.lock().expect("event capture lock");
    assert!(
        events.iter().any(
            |event| matches!(event, ProtocolEvent::ToolRequest { call_id, .. } if call_id == "t1")
        ),
        "policy allow must continue through protocol approval"
    );
}

#[tokio::test]
async fn live_mode_deescalation_revokes_boot_bypass() {
    let manager = Arc::new(ToolApprovalManager::new());
    manager.set_mode(SessionMode::Force);
    let emitter = Arc::new(ResolvingEmitter {
        events: Mutex::new(Vec::new()),
        manager: manager.clone(),
        approve: false,
        resolved_synchronously: std::sync::atomic::AtomicBool::new(false),
    });
    let mut cfg = cfg_with_gate(Some(allow_guarded_gate()));
    // Model the launch-time Bypass snapshot that previously stayed ORed into
    // the protocol path after the host visibly tightened its mode.
    cfg.confirmer = auto_approve_confirmer();
    cfg.approval = Some(wcore_agent::orchestration::node_executor::ApprovalChannel {
        manager: manager.clone(),
        writer: emitter.clone(),
        msg_id: "live-deescalation".into(),
    });

    manager.set_mode(SessionMode::Default);
    let result = run_guarded_call(cfg).await;

    assert!(emitter.resolved_synchronously.load(Ordering::SeqCst));
    match result {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(is_error);
            assert_eq!(content, "Tool denied: denied by host");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[tokio::test]
async fn protocol_approval_is_bound_to_original_tool_input() {
    let manager = Arc::new(ToolApprovalManager::new());
    let emitter = Arc::new(ResolvingEmitter {
        events: Mutex::new(Vec::new()),
        manager: manager.clone(),
        approve: true,
        resolved_synchronously: std::sync::atomic::AtomicBool::new(false),
    });
    let mut cfg = cfg_with_gate(Some(allow_guarded_gate()));
    cfg.approval = Some(wcore_agent::orchestration::node_executor::ApprovalChannel {
        manager: manager.clone(),
        writer: emitter.clone(),
        msg_id: "approval-input-binding".into(),
    });

    let mut hooks = HookEngine::new(HooksConfig::default());
    hooks.register_rust_hook(Box::new(MutatingHook));
    let result = run_guarded_call_with_hooks(cfg, Some(hooks)).await;
    assert!(emitter.resolved_synchronously.load(Ordering::SeqCst));

    match &result {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(*is_error, "changed arguments require a fresh approval");
            assert_eq!(
                content,
                "Blocked by hook: modified tool input requires fresh approval"
            );
            assert!(!content.contains("tool-executed"));
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[tokio::test]
async fn scoped_auto_approval_is_bound_to_original_tool_input() {
    let manager = Arc::new(ToolApprovalManager::new());
    manager.add_auto_approve("info");
    let emitter = Arc::new(CapturingEmitter::default());
    let mut cfg = cfg_with_gate(Some(allow_guarded_gate()));
    cfg.approval = Some(wcore_agent::orchestration::node_executor::ApprovalChannel {
        manager,
        writer: emitter.clone(),
        msg_id: "scoped-approval-input-binding".into(),
    });

    let mut hooks = HookEngine::new(HooksConfig::default());
    hooks.register_rust_hook(Box::new(MutatingHook));
    let result = run_guarded_call_with_hooks(cfg, Some(hooks)).await;

    match &result {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(*is_error, "scoped grants bind the matching arguments");
            assert_eq!(
                content,
                "Blocked by hook: modified tool input requires fresh approval"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    assert!(
        !emitter
            .events
            .lock()
            .expect("event capture lock")
            .iter()
            .any(|event| matches!(event, ProtocolEvent::ToolRequest { .. })),
        "the scoped grant should avoid a prompt without permitting mutation"
    );
}

#[tokio::test]
async fn failed_tool_request_emission_drops_pending_approval() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("guarded", "must-not-run", false)));
    let manager = Arc::new(ToolApprovalManager::new());
    let writer: Arc<dyn ProtocolEmitter> = Arc::new(FailingEmitter);
    let calls = vec![tool_use("t1", "guarded")];

    let result = execute_tool_calls_with_approval(
        &registry,
        &calls,
        &manager,
        &writer,
        "failed-emission",
        &[],
        None,
        CompactionLevel::Off,
        false,
        &CancellationToken::new(),
        None,
    )
    .await;

    assert!(matches!(result, Err(ExecutionControl::Quit)));
    assert!(
        !manager.resolve_host("t1", true, ApprovalScope::Once, None),
        "failed emission must not leave a pending approval behind"
    );
}

#[tokio::test]
async fn policy_allow_still_honors_protocol_force() {
    let manager = Arc::new(ToolApprovalManager::new());
    manager.set_allow_wire_force(true);
    assert!(manager.set_mode_from_wire(SessionMode::Force));

    let emitter = Arc::new(CapturingEmitter::default());
    let mut cfg = cfg_with_gate(Some(allow_guarded_gate()));
    cfg.confirmer = Arc::new(Mutex::new(ToolConfirmer::new(false, vec![])));
    cfg.approval = Some(wcore_agent::orchestration::node_executor::ApprovalChannel {
        manager,
        writer: emitter.clone(),
        msg_id: "policy-and-force".into(),
    });

    let mut hooks = HookEngine::new(HooksConfig::default());
    hooks.register_rust_hook(Box::new(MutatingHook));
    let result = run_guarded_call_with_hooks(cfg, Some(hooks)).await;
    match &result {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(!is_error, "wire Force may approve a policy-allowed call");
            assert_eq!(content, "tool-executed");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }

    let events = emitter.events.lock().expect("event capture lock");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProtocolEvent::ToolRequest { .. })),
        "Force must not prompt after the ACL gate allows the call"
    );
}

#[tokio::test]
async fn policy_deny_beats_protocol_force() {
    let manager = Arc::new(ToolApprovalManager::new());
    manager.set_allow_wire_force(true);
    assert!(manager.set_mode_from_wire(SessionMode::Force));

    let emitter = Arc::new(CapturingEmitter::default());
    let mut cfg = cfg_with_gate(Some(deny_all_gate()));
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool {
        name: "guarded",
        executions: executions.clone(),
    }));
    cfg.tools = Arc::new(registry);
    cfg.approval = Some(wcore_agent::orchestration::node_executor::ApprovalChannel {
        manager,
        writer: emitter.clone(),
        msg_id: "deny-beats-force".into(),
    });

    let result = run_guarded_call(cfg).await;
    match &result {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(*is_error);
            assert!(content.starts_with("Denied by policy"));
            assert!(!content.contains("tool-executed"));
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "policy denial must short-circuit the underlying tool"
    );

    let events = emitter.events.lock().expect("event capture lock");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProtocolEvent::ToolRequest { .. })),
        "a policy-denied call must never reach the approval rail"
    );
}

#[tokio::test]
async fn mixed_policy_batch_preserves_order_and_deny_beats_force() {
    let manager = Arc::new(ToolApprovalManager::new());
    manager.set_allow_wire_force(true);
    assert!(manager.set_mode_from_wire(SessionMode::Force));

    let allowed_executions = Arc::new(AtomicUsize::new(0));
    let denied_executions = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool {
        name: "denied",
        executions: denied_executions.clone(),
    }));
    registry.register(Box::new(CountingTool {
        name: "allowed",
        executions: allowed_executions.clone(),
    }));

    let emitter = Arc::new(CapturingEmitter::default());
    let mut cfg = cfg_with_gate(Some(gate_allowing("allowed")));
    cfg.tools = Arc::new(registry);
    cfg.approval = Some(wcore_agent::orchestration::node_executor::ApprovalChannel {
        manager,
        writer: emitter.clone(),
        msg_id: "mixed-deny-and-force".into(),
    });

    let results = run_calls_with_hooks(
        cfg,
        vec![
            tool_use("d1", "denied"),
            tool_use("a1", "allowed"),
            tool_use("d2", "denied"),
        ],
        None,
    )
    .await;

    let ids: Vec<_> = results
        .iter()
        .map(|result| match result {
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.as_str(),
            other => panic!("expected ToolResult, got {other:?}"),
        })
        .collect();
    assert_eq!(ids, ["d1", "a1", "d2"]);
    assert_eq!(allowed_executions.load(Ordering::SeqCst), 1);
    assert_eq!(denied_executions.load(Ordering::SeqCst), 0);
    assert!(matches!(
        &results[0],
        ContentBlock::ToolResult { is_error: true, .. }
    ));
    assert!(matches!(
        &results[1],
        ContentBlock::ToolResult {
            content,
            is_error: false,
            ..
        } if content == "allowed-executed"
    ));
    assert!(matches!(
        &results[2],
        ContentBlock::ToolResult { is_error: true, .. }
    ));
    assert!(
        !emitter
            .events
            .lock()
            .expect("event capture lock")
            .iter()
            .any(|event| matches!(event, ProtocolEvent::ToolRequest { .. })),
        "policy denials and Force approvals must not prompt"
    );
}
