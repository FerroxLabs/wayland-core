use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use wcore_permissions::{Actor, CallActor, PolicyEngine};
use wcore_protocol::ToolApprovalManager;
use wcore_protocol::commands::ApprovalScope;
use wcore_protocol::events::{ProtocolEvent, ToolCategory};
use wcore_protocol::writer::ProtocolEmitter;
use wcore_tools::dispatcher::ClosureDispatcher;
use wcore_tools::effects::PreparedToolEffect;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::script::ScriptTool;
use wcore_tools::{Tool, ToolExecutionClass};
use wcore_types::message::ContentBlock;
use wcore_types::tool::ToolResult;

use super::graph::NodeExecutor;
use super::node_executor::{AgentExecutorConfig, AgentNodeExecutor, TurnCell};
use super::*;
use crate::confirm::ToolConfirmer;
use crate::journal_effects::{JournalEffectCoordinator, TurnEffectScope};
use crate::plugins::PluginToolAdapter;
use crate::policy_gate::PolicyGate;
use crate::session_journal::{
    SessionEvent, SessionJournal, ToolEffectState, ToolNotStartedReason, ToolState,
    ToolUnknownReason,
};
use crate::tool_budget::ToolBudgetTracker;

#[derive(Clone, Copy)]
enum ProbeBehavior {
    Succeed,
    Hang,
    Panic,
    WaitForCancellation,
}

struct ProbeTool {
    name: &'static str,
    calls: Arc<AtomicUsize>,
    behavior: ProbeBehavior,
    class: ToolExecutionClass,
    started: Option<Arc<Notify>>,
    prepare_started: Option<Arc<Notify>>,
    release_prepare: Option<Arc<Notify>>,
}

impl ProbeTool {
    fn new(
        name: &'static str,
        calls: Arc<AtomicUsize>,
        behavior: ProbeBehavior,
        class: ToolExecutionClass,
    ) -> Self {
        Self {
            name,
            calls,
            behavior,
            class,
            started: None,
            prepare_started: None,
            release_prepare: None,
        }
    }

    fn with_started(mut self, started: Arc<Notify>) -> Self {
        self.started = Some(started);
        self
    }

    fn with_prepare_barrier(mut self, started: Arc<Notify>, release: Arc<Notify>) -> Self {
        self.prepare_started = Some(started);
        self.release_prepare = Some(release);
        self
    }

    async fn run(&self, cancel: Option<&CancellationToken>) -> ToolResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(started) = &self.started {
            started.notify_one();
        }
        match self.behavior {
            ProbeBehavior::Succeed => ToolResult {
                content: "ok".into(),
                is_error: false,
            },
            ProbeBehavior::Hang => {
                std::future::pending::<()>().await;
                unreachable!("hanging probe only exits through dispatcher timeout")
            }
            ProbeBehavior::Panic => panic!("injected opaque tool panic"),
            ProbeBehavior::WaitForCancellation => {
                cancel
                    .expect("ctx-aware dispatch supplies cancellation")
                    .cancelled()
                    .await;
                ToolResult {
                    content: "cancel observed after durable start".into(),
                    is_error: true,
                }
            }
        }
    }
}

#[async_trait]
impl Tool for ProbeTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "F13 production-dispatch durability probe"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false})
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn execution_class_for(&self, _input: &Value) -> ToolExecutionClass {
        self.class
    }

    async fn prepare_effect(
        &self,
        _input: &Value,
        _ctx: &wcore_tools::context::ToolContext,
    ) -> Result<Option<PreparedToolEffect>, ToolResult> {
        if let Some(started) = &self.prepare_started {
            started.notify_one();
        }
        if let Some(release) = &self.release_prepare {
            release.notified().await;
        }
        Ok(None)
    }

    async fn execute(&self, _input: Value) -> ToolResult {
        self.run(None).await
    }

    async fn execute_with_ctx(
        &self,
        _input: Value,
        ctx: &wcore_tools::context::ToolContext,
    ) -> ToolResult {
        self.run(Some(&ctx.cancel)).await
    }
}

fn effect_fixture() -> (tempfile::TempDir, SessionJournal, TurnEffectScope) {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
    journal
        .append(SessionEvent::TurnStarted {
            turn_id: "turn".into(),
            user_message: "F13 durability boundary proof".into(),
        })
        .unwrap();
    let scope = JournalEffectCoordinator::new(journal.clone()).for_turn("turn");
    (dir, journal, scope)
}

fn tool_call(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
        extra: None,
    }
}

fn only_tool(journal: &SessionJournal) -> ToolState {
    let state = journal.state().unwrap();
    assert_eq!(state.tools.len(), 1, "exactly one durable tool record");
    state.tools.values().next().unwrap().clone()
}

fn assert_not_started(tool: &ToolState, expected: fn(&ToolNotStartedReason) -> bool) {
    assert!(matches!(tool.effect, ToolEffectState::NotStarted));
    let reason = tool
        .not_started_reason
        .as_ref()
        .expect("not-started must carry its typed denial reason");
    assert!(
        expected(reason),
        "unexpected not-started reason: {reason:?}"
    );
}

async fn execute_durable(
    registry: &ToolRegistry,
    call: &ContentBlock,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    scope: &TurnEffectScope,
) -> ContentBlock {
    execute_single_with_budget(
        registry,
        call,
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        budget,
        false,
        cancel,
        None,
        Some(scope),
        0,
    )
    .await
    .0
}

#[tokio::test]
async fn production_node_policy_denial_is_durable_not_started() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "PolicyProbe",
        Arc::clone(&calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let cell = Arc::new(tokio::sync::Mutex::new(TurnCell::new(
        vec![tool_call("policy-call", "PolicyProbe", json!({}))],
        None,
    )));
    let cfg = AgentExecutorConfig {
        tools: Arc::new(registry),
        confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
        compaction_level: wcore_compact::CompactionLevel::Off,
        toon_enabled: false,
        streaming: None,
        tool_budget: None,
        approval: None,
        allow_list: vec![],
        policy_gate: Some(PolicyGate::new(
            Arc::new(PolicyEngine::new()),
            Actor::User("default".into()),
        )),
        actor: CallActor::Root,
        learned_policy: None,
        cancel: CancellationToken::new(),
        file_write_notifier: None,
        dispatcher_crash_cut: None,
    };

    AgentNodeExecutor::new(cfg, Arc::clone(&cell))
        .with_effect_scope(Some(scope))
        .run_agent("main", &Value::Null)
        .await
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 0, "denied tool executed");
    assert_not_started(&only_tool(&journal), |reason| {
        matches!(reason, ToolNotStartedReason::PolicyDenied { .. })
    });
}

struct DenyingEmitter {
    manager: Arc<ToolApprovalManager>,
}

impl ProtocolEmitter for DenyingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        if let ProtocolEvent::ToolRequest { call_id, .. } = event {
            assert!(
                self.manager
                    .resolve_host(call_id, false, ApprovalScope::Once, None,)
            );
        }
        Ok(())
    }
}

#[tokio::test]
async fn host_approval_denial_is_durable_not_started() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "ApprovalProbe",
        Arc::clone(&calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let manager = Arc::new(ToolApprovalManager::new());
    let writer: Arc<dyn ProtocolEmitter> = Arc::new(DenyingEmitter {
        manager: Arc::clone(&manager),
    });
    let call = tool_call("approval-call", "ApprovalProbe", json!({}));

    let outcome = execute_tool_calls_with_approval_budget_and_effects(
        &registry,
        std::slice::from_ref(&call),
        &manager,
        &writer,
        "message",
        &[],
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        &CancellationToken::new(),
        None,
        Some(&scope),
        None,
    )
    .await
    .unwrap();

    assert!(block_is_error(&outcome.results[0]));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "denied tool executed");
    assert_not_started(&only_tool(&journal), |reason| {
        matches!(reason, ToolNotStartedReason::ApprovalDenied { .. })
    });
}

#[tokio::test]
async fn budget_denial_is_durable_not_started() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "BudgetProbe",
        Arc::clone(&calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::ProcessSpawning,
    )));
    let budget = crate::budget::ExecutionBudget {
        max_processes: Some(0),
        ..Default::default()
    }
    .start_root();
    let tracker = ToolBudgetTracker::with_execution_budget(budget);
    let (_dir, journal, scope) = effect_fixture();
    let result = execute_durable(
        &registry,
        &tool_call("budget-call", "BudgetProbe", json!({})),
        Some(&tracker),
        &CancellationToken::new(),
        &scope,
    )
    .await;

    assert!(block_is_error(&result));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "budget-denied tool executed"
    );
    assert_not_started(&only_tool(&journal), |reason| {
        matches!(reason, ToolNotStartedReason::BudgetDenied { .. })
    });
}

#[tokio::test]
async fn circuit_denial_is_durable_not_started() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "CircuitProbe",
        Arc::clone(&calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    for _ in 0..3 {
        registry.record_breaker_outcome("CircuitProbe", true);
    }
    assert!(registry.breaker_is_open("CircuitProbe"));
    let (_dir, journal, scope) = effect_fixture();
    let result = execute_durable(
        &registry,
        &tool_call("circuit-call", "CircuitProbe", json!({})),
        None,
        &CancellationToken::new(),
        &scope,
    )
    .await;

    assert!(block_is_error(&result));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "open-circuit tool executed"
    );
    assert_not_started(&only_tool(&journal), |reason| {
        matches!(reason, ToolNotStartedReason::CircuitOpen)
    });
}

#[tokio::test(start_paused = true)]
async fn dispatcher_timeout_is_durable_unknown() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "TimeoutProbe",
        Arc::clone(&calls),
        ProbeBehavior::Hang,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let result = execute_durable(
        &registry,
        &tool_call("timeout-call", "TimeoutProbe", json!({})),
        None,
        &CancellationToken::new(),
        &scope,
    )
    .await;

    assert!(block_is_error(&result));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        only_tool(&journal).effect,
        ToolEffectState::Unknown {
            reason: ToolUnknownReason::TimedOut { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn dispatcher_panic_is_durable_unknown() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "PanicProbe",
        Arc::clone(&calls),
        ProbeBehavior::Panic,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let result = execute_durable(
        &registry,
        &tool_call("panic-call", "PanicProbe", json!({})),
        None,
        &CancellationToken::new(),
        &scope,
    )
    .await;

    assert!(block_is_error(&result));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        only_tool(&journal).effect,
        ToolEffectState::Unknown {
            reason: ToolUnknownReason::Panicked { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn cooperative_cancellation_after_start_is_durable_unknown() {
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        ProbeTool::new(
            "CancelProbe",
            Arc::clone(&calls),
            ProbeBehavior::WaitForCancellation,
            ToolExecutionClass::InProcess,
        )
        .with_started(Arc::clone(&started)),
    ));
    let cancel = CancellationToken::new();
    let (_dir, journal, scope) = effect_fixture();
    let call = tool_call("cancel-call", "CancelProbe", json!({}));
    let dispatch = execute_durable(&registry, &call, None, &cancel, &scope);
    let cancel_after_start = async {
        started.notified().await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(dispatch, cancel_after_start);

    assert!(block_is_error(&result));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        only_tool(&journal).effect,
        ToolEffectState::Unknown {
            reason: ToolUnknownReason::Cancelled { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn cancellation_before_dispatch_is_durable_not_started() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "CancelProbe",
        Arc::clone(&calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (_dir, journal, scope) = effect_fixture();
    let result = execute_durable(
        &registry,
        &tool_call("cancel-before-start", "CancelProbe", json!({})),
        None,
        &cancel,
        &scope,
    )
    .await;

    assert!(block_is_error(&result));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_not_started(&only_tool(&journal), |reason| {
        matches!(reason, ToolNotStartedReason::Cancelled { .. })
    });
}

#[tokio::test]
async fn cancellation_during_preparation_is_durable_not_started() {
    let calls = Arc::new(AtomicUsize::new(0));
    let prepare_started = Arc::new(Notify::new());
    let release_prepare = Arc::new(Notify::new());
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        ProbeTool::new(
            "CancelProbe",
            Arc::clone(&calls),
            ProbeBehavior::Succeed,
            ToolExecutionClass::InProcess,
        )
        .with_prepare_barrier(Arc::clone(&prepare_started), Arc::clone(&release_prepare)),
    ));
    let cancel = CancellationToken::new();
    let (_dir, journal, scope) = effect_fixture();
    let call = tool_call("cancel-during-prepare", "CancelProbe", json!({}));
    let dispatch = execute_durable(&registry, &call, None, &cancel, &scope);
    let cancel_during_prepare = async {
        prepare_started.notified().await;
        cancel.cancel();
        release_prepare.notify_one();
    };
    let (result, ()) = tokio::join!(dispatch, cancel_during_prepare);

    assert!(block_is_error(&result));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_not_started(&only_tool(&journal), |reason| {
        matches!(reason, ToolNotStartedReason::Cancelled { .. })
    });
}

async fn assert_actual_adapter_error_is_unknown(registry: ToolRegistry, call: ContentBlock) {
    let (_dir, journal, scope) = effect_fixture();
    let result = execute_durable(&registry, &call, None, &CancellationToken::new(), &scope).await;
    assert!(block_is_error(&result));
    assert!(matches!(
        only_tool(&journal).effect,
        ToolEffectState::Unknown {
            reason: ToolUnknownReason::AmbiguousFailure { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn actual_mcp_adapter_error_is_durable_unknown() {
    let manager = Arc::new(wcore_mcp::manager::McpManager::new_for_test(vec![]));
    let proxy = wcore_mcp::tool_proxy::McpToolProxy::new(
        "McpFailure".into(),
        "remote_effect".into(),
        "missing-server".into(),
        "actual MCP adapter durability probe".into(),
        json!({"type": "object"}),
        manager,
        false,
    );
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(proxy));
    assert_actual_adapter_error_is_unknown(
        registry,
        tool_call("mcp-call", "McpFailure", json!({})),
    )
    .await;
}

#[tokio::test]
async fn actual_plugin_adapter_error_is_durable_unknown() {
    let plugin = wcore_plugin_api::tool::PluginTool {
        name: "PluginFailure".into(),
        description: "actual plugin adapter durability probe".into(),
        input_schema: json!({"type": "object"}),
        category: ToolCategory::Info,
        is_deferred: false,
        max_result_size: 1_000,
        execute: Arc::new(|_inv| {
            Box::pin(async {
                ToolResult {
                    content: "plugin reported an ambiguous external failure".into(),
                    is_error: true,
                }
            })
        }),
    };
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(PluginToolAdapter::new(plugin)));
    assert_actual_adapter_error_is_unknown(
        registry,
        tool_call("plugin-call", "PluginFailure", json!({})),
    )
    .await;
}

#[tokio::test]
async fn actual_script_adapter_error_is_durable_unknown() {
    let dispatcher = ClosureDispatcher::new(Box::new(|_tool, _input| {
        Box::pin(async {
            ToolResult {
                content: "nested Script step outcome is ambiguous".into(),
                is_error: true,
            }
        })
    }));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ScriptTool::new(Arc::new(dispatcher))));
    assert_actual_adapter_error_is_unknown(
        registry,
        tool_call(
            "script-call",
            "Script",
            json!({
                "steps": [{
                    "id": "read-step",
                    "tool": "Read",
                    "input": {}
                }]
            }),
        ),
    )
    .await;
}
