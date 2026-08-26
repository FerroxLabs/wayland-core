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
use crate::hooks::{Hook, HookAction, HookEngine};
use crate::journal_effects::{JournalEffectCoordinator, TurnEffectScope};
use crate::plugins::PluginToolAdapter;
use crate::policy_gate::PolicyGate;
use crate::session_journal::{
    ApprovalDecision, ApprovalResolution, HookPhaseState, SessionEvent, SessionJournal,
    ToolEffectState, ToolNotStartedReason, ToolResolutionSource, ToolState, ToolUnknownReason,
    state_payload_digest,
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

struct DurableLifecycleHook;

#[async_trait]
impl Hook for DurableLifecycleHook {
    fn name(&self) -> &str {
        "durable-lifecycle"
    }

    async fn pre_tool_use(&self, _tool: &str, _input: &Value) -> HookAction {
        HookAction::Continue
    }

    async fn post_tool_use(
        &self,
        _tool: &str,
        _call_id: &str,
        _input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> HookAction {
        HookAction::Continue
    }
}

struct CountingLifecycleHook {
    pre_calls: Arc<AtomicUsize>,
    post_calls: Arc<AtomicUsize>,
}

struct InputModifyingHook(Arc<AtomicUsize>);

#[async_trait]
impl Hook for InputModifyingHook {
    fn name(&self) -> &str {
        "input-modifying"
    }

    async fn pre_tool_use(&self, _tool: &str, _input: &Value) -> HookAction {
        self.0.fetch_add(1, Ordering::SeqCst);
        HookAction::ModifyInput(json!({"modified": true}))
    }
}

#[async_trait]
impl Hook for CountingLifecycleHook {
    fn name(&self) -> &str {
        "counting-lifecycle"
    }

    async fn pre_tool_use(&self, _tool: &str, _input: &Value) -> HookAction {
        self.pre_calls.fetch_add(1, Ordering::SeqCst);
        HookAction::Continue
    }

    async fn post_tool_use(
        &self,
        _tool: &str,
        _call_id: &str,
        _input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> HookAction {
        self.post_calls.fetch_add(1, Ordering::SeqCst);
        HookAction::Continue
    }
}

#[tokio::test]
async fn production_dispatch_orders_and_atomically_consumes_durable_hook_phases() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "HookedProbe",
        Arc::clone(&calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let mut hooks = HookEngine::new(wcore_config::hooks::HooksConfig::default());
    hooks.register_rust_hook(Box::new(DurableLifecycleHook));

    let (result, _, outcome, was_cancelled) = execute_single_with_budget(
        &registry,
        &tool_call("hooked-call", "HookedProbe", json!({})),
        Some(&hooks),
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        false,
        &CancellationToken::new(),
        None,
        Some(&scope),
        0,
    )
    .await;

    assert!(!was_cancelled);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        result,
        ContentBlock::ToolResult {
            is_error: false,
            ..
        }
    ));
    assert_eq!(outcome.durable_hook_phases.len(), 2);

    let mut pre_hook_phase_id = None;
    let mut post_hook_phase_id = None;
    let event_order = journal
        .committed_entries()
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry.event {
            SessionEvent::HookPhasePrepared {
                hook_phase_id,
                phase,
                ..
            } => Some(match phase {
                ToolHookPhase::PreToolUse => {
                    pre_hook_phase_id = Some(hook_phase_id);
                    "pre-prepared"
                }
                ToolHookPhase::PostToolUse => {
                    post_hook_phase_id = Some(hook_phase_id);
                    "post-prepared"
                }
            }),
            SessionEvent::HookPhaseStarted { hook_phase_id, .. } => Some(
                if pre_hook_phase_id.as_deref() == Some(hook_phase_id.as_str()) {
                    "pre-started"
                } else {
                    assert_eq!(post_hook_phase_id.as_deref(), Some(hook_phase_id.as_str()));
                    "post-started"
                },
            ),
            SessionEvent::HookPhaseFinished { hook_phase_id, .. } => Some(
                if pre_hook_phase_id.as_deref() == Some(hook_phase_id.as_str()) {
                    "pre-finished"
                } else {
                    assert_eq!(post_hook_phase_id.as_deref(), Some(hook_phase_id.as_str()));
                    "post-finished"
                },
            ),
            SessionEvent::ToolIntentRecordedV2 { .. } => Some("tool-prepared"),
            SessionEvent::ToolExecutionStarted { .. } => Some("tool-started"),
            SessionEvent::ToolExecutionFinished { .. } => Some("tool-finished"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        event_order,
        vec![
            "pre-prepared",
            "pre-started",
            "pre-finished",
            "tool-prepared",
            "post-prepared",
            "tool-started",
            "tool-finished",
            "post-started",
            "post-finished",
        ]
    );

    let messages = vec![json!({"role": "assistant", "content": "tool round complete"})];
    let messages_digest = state_payload_digest(&Value::Array(messages.clone())).unwrap();
    let checkpoint = json!({"kind": "f14-hook-consumption-proof"});
    let checkpoint_state_digest = state_payload_digest(&checkpoint).unwrap();
    journal
        .append(SessionEvent::ConversationRecoveryCheckpointCommittedV2 {
            turn_id: "turn".into(),
            messages,
            messages_digest,
            checkpoint_id: "hook-checkpoint".into(),
            checkpoint_state_digest,
            checkpoint,
            consumed_hook_phases: outcome.durable_hook_phases,
        })
        .unwrap();

    let state = journal.state().unwrap();
    assert_eq!(state.hook_phases.len(), 2);
    assert!(state.hook_phases.values().all(|phase| matches!(
        phase.state,
        HookPhaseState::Consumed { ref checkpoint_id, .. } if checkpoint_id == "hook-checkpoint"
    )));
}

#[tokio::test]
async fn recovered_retry_reuses_pre_hook_and_runs_post_once_for_new_attempt() {
    let physical_calls = Arc::new(AtomicUsize::new(0));
    let pre_calls = Arc::new(AtomicUsize::new(0));
    let post_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "RecoveredHookProbe",
        Arc::clone(&physical_calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let mut hooks = HookEngine::new(wcore_config::hooks::HooksConfig::default());
    hooks.register_rust_hook(Box::new(CountingLifecycleHook {
        pre_calls: Arc::clone(&pre_calls),
        post_calls: Arc::clone(&post_calls),
    }));
    let call = tool_call("recovered-hook-call", "RecoveredHookProbe", json!({}));

    let crashed = scope_dispatcher_crash_cut(
        DispatcherCrashCut::BeforeRunning,
        AssertUnwindSafe(execute_single_with_budget(
            &registry,
            &call,
            Some(&hooks),
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            false,
            &CancellationToken::new(),
            None,
            Some(&scope),
            0,
        ))
        .catch_unwind(),
    )
    .await;
    assert!(crashed.is_err());
    assert_eq!(pre_calls.load(Ordering::SeqCst), 1);
    assert_eq!(post_calls.load(Ordering::SeqCst), 0);
    assert_eq!(physical_calls.load(Ordering::SeqCst), 0);

    let state = journal.state().unwrap();
    let (prior_tool_execution_id, prior) = state.tools.iter().next().unwrap();
    let prior_tool_execution_id = prior_tool_execution_id.clone();
    let prior = prior.clone();
    let pre_hook_phase_id = prior.pre_hook_phase_id.clone().unwrap();
    let pre_outcome_digest = match &state.hook_phases[&pre_hook_phase_id].state {
        HookPhaseState::Finished { outcome_digest, .. } => outcome_digest.clone(),
        other => panic!("pre-hook was not durably finished before crash: {other:?}"),
    };
    let original_post_hook_phase_id = state
        .hook_phases
        .iter()
        .find(|(_, phase)| {
            phase.phase == ToolHookPhase::PostToolUse
                && phase.tool_execution_id.as_deref() == Some(&prior_tool_execution_id)
        })
        .map(|(id, _)| id.clone())
        .unwrap();
    drop(state);

    journal
        .append(SessionEvent::ToolExecutionNotStarted {
            tool_execution_id: prior_tool_execution_id.clone(),
            reason: ToolNotStartedReason::Cancelled {
                reason: "injected crash before durable tool start".into(),
            },
        })
        .unwrap();
    journal
        .append(SessionEvent::HookPhaseNotApplicable {
            hook_phase_id: original_post_hook_phase_id.clone(),
        })
        .unwrap();

    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer: Arc<dyn ProtocolEmitter> = Arc::new(NoopEmitter);
    let outcome = execute_recovered_retry_tool_call_with_effects(
        &registry,
        &call,
        &approval_manager,
        &writer,
        "msg",
        &["RecoveredHookProbe".into()],
        Some(&mut hooks),
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        &CancellationToken::new(),
        None,
        &scope,
        0,
        None,
        "recovered-hook-call",
        &prior_tool_execution_id,
        &prior.tool,
        prior.ordinal,
        &prior.effect_contract,
        prior.effect_receipt.as_ref(),
        &prior.requested_input_digest,
        &prior.effective_input_digest,
        Some(&pre_hook_phase_id),
        Some(crate::session_journal::HookPhaseConsumption {
            hook_phase_id: pre_hook_phase_id.clone(),
            outcome_digest: pre_outcome_digest,
        }),
    )
    .await
    .unwrap();

    assert_eq!(physical_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        pre_calls.load(Ordering::SeqCst),
        1,
        "recovery must not repeat the pre-hook"
    );
    assert_eq!(
        post_calls.load(Ordering::SeqCst),
        1,
        "the retry attempt must run its own post-hook exactly once"
    );
    assert_eq!(
        outcome
            .hook_outcomes
            .iter()
            .map(|outcome| outcome.durable_hook_phases.len())
            .sum::<usize>(),
        2
    );
    let state = journal.state().unwrap();
    assert!(matches!(
        state.hook_phases[&original_post_hook_phase_id].state,
        HookPhaseState::NotApplicable
    ));
    let (retry_id, retry) = state
        .tools
        .iter()
        .find(|(_, tool)| tool.retry_of.as_deref() == Some(&prior_tool_execution_id))
        .unwrap();
    assert!(matches!(retry.effect, ToolEffectState::Succeeded));
    assert!(state.hook_phases.values().any(|phase| {
        phase.phase == ToolHookPhase::PostToolUse
            && phase.tool_execution_id.as_deref() == Some(retry_id.as_str())
            && matches!(phase.state, HookPhaseState::Finished { .. })
    }));
}

#[tokio::test]
async fn recovered_retry_with_redacted_hook_modified_input_fails_before_dispatch() {
    let physical_calls = Arc::new(AtomicUsize::new(0));
    let pre_calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "ModifiedRecoveryProbe",
        Arc::clone(&physical_calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let mut hooks = HookEngine::new(wcore_config::hooks::HooksConfig::default());
    hooks.register_rust_hook(Box::new(InputModifyingHook(Arc::clone(&pre_calls))));
    let call = tool_call("modified-recovery-call", "ModifiedRecoveryProbe", json!({}));

    let crashed = scope_dispatcher_crash_cut(
        DispatcherCrashCut::BeforeRunning,
        AssertUnwindSafe(execute_single_with_budget(
            &registry,
            &call,
            Some(&hooks),
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            false,
            &CancellationToken::new(),
            None,
            Some(&scope),
            0,
        ))
        .catch_unwind(),
    )
    .await;
    assert!(crashed.is_err());
    assert_eq!(pre_calls.load(Ordering::SeqCst), 1);

    let state = journal.state().unwrap();
    let (prior_tool_execution_id, prior) = state.tools.iter().next().unwrap();
    let prior_tool_execution_id = prior_tool_execution_id.clone();
    let prior = prior.clone();
    assert_ne!(
        prior.requested_input_digest, prior.effective_input_digest,
        "fixture must prove a hook-modified effective input"
    );
    let pre_hook_phase_id = prior.pre_hook_phase_id.clone().unwrap();
    let pre_outcome_digest = match &state.hook_phases[&pre_hook_phase_id].state {
        HookPhaseState::Finished { outcome_digest, .. } => outcome_digest.clone(),
        other => panic!("pre-hook was not durably finished before crash: {other:?}"),
    };
    let post_hook_phase_id = state
        .hook_phases
        .iter()
        .find(|(_, phase)| phase.phase == ToolHookPhase::PostToolUse)
        .map(|(id, _)| id.clone())
        .unwrap();
    drop(state);
    journal
        .append(SessionEvent::ToolExecutionNotStarted {
            tool_execution_id: prior_tool_execution_id.clone(),
            reason: ToolNotStartedReason::Cancelled {
                reason: "injected crash before durable tool start".into(),
            },
        })
        .unwrap();
    journal
        .append(SessionEvent::HookPhaseNotApplicable {
            hook_phase_id: post_hook_phase_id,
        })
        .unwrap();

    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer: Arc<dyn ProtocolEmitter> = Arc::new(NoopEmitter);
    let outcome = execute_recovered_retry_tool_call_with_effects(
        &registry,
        &call,
        &approval_manager,
        &writer,
        "msg",
        &["ModifiedRecoveryProbe".into()],
        Some(&mut hooks),
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        &CancellationToken::new(),
        None,
        &scope,
        0,
        None,
        "modified-recovery-call",
        &prior_tool_execution_id,
        &prior.tool,
        prior.ordinal,
        &prior.effect_contract,
        prior.effect_receipt.as_ref(),
        &prior.requested_input_digest,
        &prior.effective_input_digest,
        Some(&pre_hook_phase_id),
        Some(crate::session_journal::HookPhaseConsumption {
            hook_phase_id: pre_hook_phase_id.clone(),
            outcome_digest: pre_outcome_digest,
        }),
    )
    .await
    .unwrap();

    assert_eq!(physical_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        pre_calls.load(Ordering::SeqCst),
        1,
        "recovery must neither rerun nor guess the modifying hook output"
    );
    assert!(matches!(
        outcome.results.as_slice(),
        [ContentBlock::ToolResult { is_error: true, .. }]
    ));
    assert_eq!(
        journal
            .state()
            .unwrap()
            .tools
            .values()
            .filter(|tool| tool.retry_of.as_deref() == Some(&prior_tool_execution_id))
            .count(),
        0,
        "a retry without reconstructable effective input must not mint an attempt"
    );
}

#[test]
fn recovered_early_denial_is_a_linked_not_started_retry() {
    let (_dir, journal, scope) = effect_fixture();
    let original = scope
        .prepare_tool("early-denial-call", 0, "Opaque", json!({}), json!({}))
        .unwrap();
    let original_id = original.id().to_owned();
    original
        .not_started(ToolNotStartedReason::Cancelled {
            reason: "crash before physical dispatch".into(),
        })
        .unwrap();
    let state = journal.state().unwrap();
    let prior = state.tools[&original_id].clone();
    drop(state);
    let retry = RecoveredToolRetry {
        call_id: "early-denial-call",
        prior_tool_execution_id: &original_id,
        tool: &prior.tool,
        ordinal: prior.ordinal,
        effect_contract: &prior.effect_contract,
        effect_receipt: prior.effect_receipt.as_ref(),
        requested_input_digest: &prior.requested_input_digest,
        effective_input_digest: &prior.effective_input_digest,
        pre_hook_phase_id: None,
        pre_hook_consumption: None,
    };

    record_tool_attempt_not_started(
        Some(&scope),
        "early-denial-call",
        0,
        "Opaque",
        &json!({}),
        &json!({}),
        prior.effect_contract.clone(),
        ToolNotStartedReason::Cancelled {
            reason: "session cancellation was requested before tool dispatch".into(),
        },
        None,
        Some(&retry),
    )
    .unwrap();

    let state = journal.state().unwrap();
    assert_eq!(state.tools.len(), 2);
    assert!(matches!(
        state.tools[&original_id].effect,
        ToolEffectState::NotStarted
    ));
    let linked = state
        .tools
        .values()
        .find(|tool| tool.retry_of.as_deref() == Some(&original_id))
        .expect("early denial must retain a linked retry receipt");
    assert!(matches!(linked.effect, ToolEffectState::NotStarted));
    assert!(matches!(
        linked.not_started_reason,
        Some(ToolNotStartedReason::Cancelled { ref reason })
            if reason == "session cancellation was requested before tool dispatch"
    ));
}

#[tokio::test]
async fn unknown_tool_outcome_terminalizes_post_hook_and_requires_tool_reconciliation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "PanickingHookedProbe",
        Arc::clone(&calls),
        ProbeBehavior::Panic,
        ToolExecutionClass::InProcess,
    )));
    let (_dir, journal, scope) = effect_fixture();
    let mut hooks = HookEngine::new(wcore_config::hooks::HooksConfig::default());
    hooks.register_rust_hook(Box::new(DurableLifecycleHook));

    let (_, _, outcome, _) = execute_single_with_budget(
        &registry,
        &tool_call("panicking-call", "PanickingHookedProbe", json!({})),
        Some(&hooks),
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        false,
        &CancellationToken::new(),
        None,
        Some(&scope),
        0,
    )
    .await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(outcome.durable_hook_phases.len(), 1);
    let state = journal.state().unwrap();
    assert!(matches!(
        only_tool(&journal).effect,
        ToolEffectState::Unknown { .. }
    ));
    assert_eq!(state.hook_phases.len(), 2);
    assert!(state.hook_phases.values().any(|phase| matches!(
        phase.state,
        HookPhaseState::NotStarted {
            reason: crate::session_journal::HookPhaseNotStartedReason::ToolOutcomeUnknown
        }
    )));
    let checkpoint = json!({"kind": "f14-unknown-tool-recovery-proof"});
    journal
        .append(SessionEvent::ConversationRecoveryCheckpointCommittedV2 {
            turn_id: "turn".into(),
            messages: Vec::new(),
            messages_digest: state_payload_digest(&Value::Array(Vec::new())).unwrap(),
            checkpoint_id: "unknown-tool-checkpoint".into(),
            checkpoint_state_digest: state_payload_digest(&checkpoint).unwrap(),
            checkpoint,
            consumed_hook_phases: outcome.durable_hook_phases,
        })
        .unwrap();
    let plan = crate::recovery::RecoveryPlan::from_journal(&journal).unwrap();
    assert!(matches!(
        plan.disposition,
        crate::recovery::RecoveryDisposition::ReconciliationRequired {
            ref tool_execution_ids,
            ..
        } if tool_execution_ids.len() == 1
    ));
}

struct CountingPostHook(Arc<AtomicUsize>);

#[async_trait]
impl Hook for CountingPostHook {
    fn name(&self) -> &str {
        "counting-post"
    }

    async fn post_tool_use(
        &self,
        _tool: &str,
        _call_id: &str,
        _input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> HookAction {
        self.0.fetch_add(1, Ordering::SeqCst);
        HookAction::Continue
    }
}

#[tokio::test]
async fn unknown_registry_tool_does_not_run_post_hooks_outside_durable_authority() {
    let post_calls = Arc::new(AtomicUsize::new(0));
    let registry = ToolRegistry::new();
    let (_dir, journal, scope) = effect_fixture();
    let mut hooks = HookEngine::new(wcore_config::hooks::HooksConfig::default());
    hooks.register_rust_hook(Box::new(CountingPostHook(Arc::clone(&post_calls))));

    let (result, _, outcome, _) = execute_single_with_budget(
        &registry,
        &tool_call("missing-call", "MissingTool", json!({})),
        Some(&hooks),
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        false,
        &CancellationToken::new(),
        None,
        Some(&scope),
        0,
    )
    .await;

    assert!(matches!(
        result,
        ContentBlock::ToolResult { is_error: true, .. }
    ));
    assert_eq!(post_calls.load(Ordering::SeqCst), 0);
    assert_eq!(outcome.durable_hook_phases.len(), 1);
    let state = journal.state().unwrap();
    assert_eq!(state.hook_phases.len(), 1);
    assert!(
        state
            .hook_phases
            .values()
            .all(|phase| matches!(phase.state, HookPhaseState::Finished { .. }))
    );
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
    journal: SessionJournal,
}

impl ProtocolEmitter for DenyingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        if let ProtocolEvent::ToolRequest { call_id, .. } = event {
            let state = self.journal.state().unwrap();
            let approval = state
                .approvals
                .get(call_id)
                .expect("approval must be durable before host emission");
            assert!(approval.resolution.is_none());
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
        journal: journal.clone(),
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
    assert!(matches!(
        journal.state().unwrap().approvals["approval-call"]
            .resolution
            .as_ref(),
        Some(ApprovalResolution::Decided {
            decision: ApprovalDecision::Deny
        })
    ));
    assert_not_started(&only_tool(&journal), |reason| {
        matches!(reason, ToolNotStartedReason::ApprovalDenied { .. })
    });
}

struct ApprovingEmitter {
    manager: Arc<ToolApprovalManager>,
    journal: SessionJournal,
}

impl ProtocolEmitter for ApprovingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        if let ProtocolEvent::ToolRequest { call_id, .. } = event {
            let state = self.journal.state().unwrap();
            let approval = state
                .approvals
                .get(call_id)
                .expect("approval must be durable before host emission");
            assert!(approval.resolution.is_none());
            assert!(
                self.manager
                    .resolve_host(call_id, true, ApprovalScope::Once, None)
            );
        }
        Ok(())
    }
}

#[tokio::test]
async fn approval_crash_cuts_reopen_at_the_exact_durable_boundary() {
    for cut in [
        ApprovalCrashCut::AfterRequested,
        ApprovalCrashCut::BeforeResolved,
        ApprovalCrashCut::AfterResolved,
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ProbeTool::new(
            "ApprovalProbe",
            Arc::clone(&calls),
            ProbeBehavior::Succeed,
            ToolExecutionClass::InProcess,
        )));
        let (dir, journal, scope) = effect_fixture();
        let path = dir.path().join("session.journal");
        let manager = Arc::new(ToolApprovalManager::new());
        let writer: Arc<dyn ProtocolEmitter> = Arc::new(ApprovingEmitter {
            manager: Arc::clone(&manager),
            journal: journal.clone(),
        });
        let call = tool_call("approval-call", "ApprovalProbe", json!({}));

        let crashed = APPROVAL_CRASH_CUT
            .scope(
                std::cell::Cell::new(Some(cut)),
                AssertUnwindSafe(execute_tool_calls_with_approval_budget_and_effects(
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
                ))
                .catch_unwind(),
            )
            .await;
        assert!(
            crashed.is_err(),
            "{cut:?} did not cut the live approval path"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "{cut:?} reached tool dispatch"
        );

        drop(writer);
        drop(scope);
        drop(journal);
        let reopened = SessionJournal::open(&path, "session").unwrap();
        let state = reopened.state().unwrap();
        let approval = &state.approvals["approval-call"];
        if cut == ApprovalCrashCut::AfterResolved {
            assert!(matches!(
                approval.resolution.as_ref(),
                Some(ApprovalResolution::Decided {
                    decision: ApprovalDecision::AllowOnce
                })
            ));
        } else {
            assert!(approval.resolution.is_none(), "{cut:?}");
        }
        assert!(state.tools.is_empty(), "{cut:?} invented a physical start");
    }
}

struct ReapingEmitter {
    manager: Arc<ToolApprovalManager>,
    journal: SessionJournal,
}

struct NoopEmitter;

impl ProtocolEmitter for NoopEmitter {
    fn emit(&self, _event: &ProtocolEvent) -> std::io::Result<()> {
        Ok(())
    }
}

impl ProtocolEmitter for ReapingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        if let ProtocolEvent::ToolRequest { call_id, .. } = event {
            assert!(
                self.journal
                    .state()
                    .unwrap()
                    .approvals
                    .contains_key(call_id),
                "approval must be durable before timeout"
            );
            assert_eq!(self.manager.reap_now(), 1);
        }
        Ok(())
    }
}

#[tokio::test]
async fn approval_cancel_and_timeout_are_terminal_before_return() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ProbeTool::new(
        "ApprovalProbe",
        Arc::clone(&calls),
        ProbeBehavior::Succeed,
        ToolExecutionClass::InProcess,
    )));
    let call = tool_call("approval-call", "ApprovalProbe", json!({}));

    let (_cancel_dir, cancel_journal, cancel_scope) = effect_fixture();
    let cancel_manager = Arc::new(ToolApprovalManager::new());
    let cancel_writer: Arc<dyn ProtocolEmitter> = Arc::new(NoopEmitter);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let cancelled = execute_tool_calls_with_approval_budget_and_effects(
        &registry,
        std::slice::from_ref(&call),
        &cancel_manager,
        &cancel_writer,
        "message",
        &[],
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        &cancel,
        None,
        Some(&cancel_scope),
        None,
    )
    .await;
    assert!(matches!(cancelled, Err(ExecutionControl::Quit)));
    assert!(matches!(
        cancel_journal.state().unwrap().approvals["approval-call"]
            .resolution
            .as_ref(),
        Some(ApprovalResolution::Cancelled)
    ));

    let (_timeout_dir, timeout_journal, timeout_scope) = effect_fixture();
    let timeout_manager = Arc::new(ToolApprovalManager::with_ttl(std::time::Duration::ZERO));
    let timeout_writer: Arc<dyn ProtocolEmitter> = Arc::new(ReapingEmitter {
        manager: Arc::clone(&timeout_manager),
        journal: timeout_journal.clone(),
    });
    let timed_out = execute_tool_calls_with_approval_budget_and_effects(
        &registry,
        std::slice::from_ref(&call),
        &timeout_manager,
        &timeout_writer,
        "message",
        &[],
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        &CancellationToken::new(),
        None,
        Some(&timeout_scope),
        None,
    )
    .await
    .unwrap();
    assert!(block_is_error(&timed_out.results[0]));
    assert!(matches!(
        timeout_journal.state().unwrap().approvals["approval-call"]
            .resolution
            .as_ref(),
        Some(ApprovalResolution::TimedOut)
    ));
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

/// An adapter error from a genuinely opaque tool must record the ambiguity —
/// it may have reached the far side before failing — and must still leave the
/// execution terminal so the turn can complete.
///
/// This helper previously asserted only that the record ended `Unknown`. That
/// is the nonterminal state which made every MCP/plugin/script adapter error
/// take the whole session down with it (live UAT defect D1); the assertions
/// below are strictly stronger. `resolution_source` proves an
/// `ToolExecutionUnknown` was recorded first, because the reducer refuses
/// `ToolExecutionResolved` for any tool that is not `Unknown`.
async fn assert_actual_adapter_error_is_recorded_ambiguous_and_terminal(
    registry: ToolRegistry,
    call: ContentBlock,
) {
    let (_dir, journal, scope) = effect_fixture();
    let result = execute_durable(&registry, &call, None, &CancellationToken::new(), &scope).await;
    assert!(block_is_error(&result));
    let tool = only_tool(&journal);
    assert!(
        matches!(tool.effect, ToolEffectState::Failed { .. }),
        "a completed adapter dispatch must end terminal, got {:?}",
        tool.effect
    );
    assert!(
        matches!(
            tool.resolution_source,
            Some(ToolResolutionSource::Reconciler { .. })
        ),
        "the ambiguity must remain on the record as a resolved unknown, got {:?}",
        tool.resolution_source
    );
    journal
        .append(SessionEvent::TurnCommitted {
            turn_id: "turn".into(),
            assistant_message: "turn survives an adapter error".into(),
        })
        .expect("the turn must remain committable after an adapter error");
}

#[tokio::test]
async fn actual_mcp_adapter_error_is_ambiguous_and_terminal() {
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
    assert_actual_adapter_error_is_recorded_ambiguous_and_terminal(
        registry,
        tool_call("mcp-call", "McpFailure", json!({})),
    )
    .await;
}

#[tokio::test]
async fn actual_plugin_adapter_error_is_ambiguous_and_terminal() {
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
    assert_actual_adapter_error_is_recorded_ambiguous_and_terminal(
        registry,
        tool_call("plugin-call", "PluginFailure", json!({})),
    )
    .await;
}

#[tokio::test]
async fn actual_script_adapter_error_is_ambiguous_and_terminal() {
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
    assert_actual_adapter_error_is_recorded_ambiguous_and_terminal(
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

// ---------------------------------------------------------------------------
// F13, reachable half (#889): the production dispatcher must carry a real
// Write/Edit call's filesystem receipt onto the durable record, and must
// narrow the contract back to opaque when the tool could not produce one.
// ---------------------------------------------------------------------------

fn file_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        wcore_tools::write::WriteTool::new(None).with_unsaved_guard(std::sync::Arc::new(
            wcore_tools::unsaved_work::UnsavedWorkGuard::new_isolated(),
        )),
    ));
    registry.register(Box::new(
        wcore_tools::edit::EditTool::new(None).with_unsaved_guard(std::sync::Arc::new(
            wcore_tools::unsaved_work::UnsavedWorkGuard::new_isolated(),
        )),
    ));
    registry
}

#[tokio::test]
async fn a_dispatched_write_records_its_filesystem_receipt_on_the_durable_tool() {
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("dispatched.txt");
    std::fs::write(&target, b"one\n").unwrap();
    let (_dir, journal, scope) = effect_fixture();
    let registry = file_tool_registry();
    let call = tool_call(
        "provider-call",
        "Write",
        json!({"file_path": target.to_string_lossy(), "content": "one\ntwo\n"}),
    );

    let block = execute_durable(&registry, &call, None, &CancellationToken::new(), &scope).await;
    assert!(
        matches!(&block, ContentBlock::ToolResult { is_error, .. } if !is_error),
        "unexpected dispatch outcome: {block:?}"
    );

    let tool = only_tool(&journal);
    assert!(matches!(tool.effect, ToolEffectState::Succeeded));
    assert_eq!(
        tool.effect_contract.kind,
        wcore_types::tool::ToolEffectKind::FilesystemTransactional
    );
    assert_eq!(
        tool.effect_contract.reconciler.as_deref(),
        Some(wcore_tools::effects::FILESYSTEM_EFFECT_RECONCILER)
    );
    let receipt = tool
        .effect_receipt
        .clone()
        .expect("the dispatcher must persist the prepared receipt");
    let receipt =
        serde_json::from_value::<wcore_tools::effects::FilesystemEffectReceiptV1>(receipt)
            .expect("a receipt the reducer already accepted must decode");
    receipt.validate().expect("and must still validate");
    assert_eq!(receipt.path(), target.as_path());
    assert_eq!(std::fs::read(&target).unwrap(), b"one\ntwo\n");
}

/// The reducer refuses to START a filesystem-transactional execution with no
/// receipt, and it is right to. A target the tool cannot identify beforehand
/// must therefore be recorded as the opaque effect it actually is — and must
/// still run.
#[cfg(unix)]
#[tokio::test]
async fn a_write_that_could_not_be_prepared_is_recorded_opaque_and_still_runs() {
    let workspace = tempfile::tempdir().unwrap();
    let real = workspace.path().join("real.txt");
    let link = workspace.path().join("link.txt");
    std::fs::write(&real, b"one\n").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (_dir, journal, scope) = effect_fixture();
    let registry = file_tool_registry();
    let call = tool_call(
        "provider-call",
        "Write",
        json!({"file_path": link.to_string_lossy(), "content": "one\ntwo\n"}),
    );

    let block = execute_durable(&registry, &call, None, &CancellationToken::new(), &scope).await;
    assert!(
        matches!(&block, ContentBlock::ToolResult { is_error, .. } if !is_error),
        "a declined preparation must not refuse the call: {block:?}"
    );

    let tool = only_tool(&journal);
    assert!(matches!(tool.effect, ToolEffectState::Succeeded));
    assert_eq!(
        tool.effect_contract.kind,
        wcore_types::tool::ToolEffectKind::Opaque,
        "a receiptless call must be recorded as the opaque effect it is"
    );
    assert!(tool.effect_contract.reconciler.is_none());
    assert!(tool.effect_receipt.is_none());
    assert_eq!(std::fs::read(&link).unwrap(), b"one\ntwo\n");
}

/// An ordinary tool error must stay a terminal failure. Recording it as a
/// reconcilable unknown would block the whole session on a human for every
/// `old_string` that did not match.
#[tokio::test]
async fn an_edit_that_cannot_match_fails_terminally_rather_than_blocking_the_session() {
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("nomatch.txt");
    std::fs::write(&target, b"one\n").unwrap();
    let (_dir, journal, scope) = effect_fixture();
    let registry = file_tool_registry();
    let call = tool_call(
        "provider-call",
        "Edit",
        json!({
            "file_path": target.to_string_lossy(),
            "old_string": "not present anywhere",
            "new_string": "x",
        }),
    );

    let block = execute_durable(&registry, &call, None, &CancellationToken::new(), &scope).await;
    assert!(
        matches!(&block, ContentBlock::ToolResult { is_error, .. } if *is_error),
        "unexpected dispatch outcome: {block:?}"
    );

    let tool = only_tool(&journal);
    assert!(
        matches!(tool.effect, ToolEffectState::Failed { .. }),
        "expected a terminal failure, got {:?}",
        tool.effect
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"one\n");
}

/// The session's private checkpoint store is quota'd, and a receipt whose
/// preimage cannot be checkpointed is one recovery declines anyway. Refusing
/// the call there would mean an agent that had spent the quota could no
/// longer write a file at all; falling back to the opaque path costs only the
/// automatic reconciliation of an interrupted write.
#[tokio::test]
async fn a_checkpoint_the_session_cannot_store_degrades_to_opaque_instead_of_refusing() {
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("unbacked.txt");
    std::fs::write(&target, b"one\n").unwrap();
    let (dir, journal, scope) = effect_fixture();
    // Put an ordinary file exactly where the private checkpoint directory
    // has to be, so nothing can ever be stored under it.
    std::fs::write(
        dir.path().join(".session.journal.effects"),
        b"not a directory",
    )
    .unwrap();

    let registry = file_tool_registry();
    let call = tool_call(
        "provider-call",
        "Write",
        json!({"file_path": target.to_string_lossy(), "content": "one\ntwo\n"}),
    );
    let block = execute_durable(&registry, &call, None, &CancellationToken::new(), &scope).await;
    assert!(
        matches!(&block, ContentBlock::ToolResult { is_error, .. } if !is_error),
        "an unstorable checkpoint must not refuse the write: {block:?}"
    );

    let tool = only_tool(&journal);
    assert!(matches!(tool.effect, ToolEffectState::Succeeded));
    assert_eq!(
        tool.effect_contract.kind,
        wcore_types::tool::ToolEffectKind::Opaque
    );
    assert!(tool.effect_receipt.is_none());
    assert_eq!(std::fs::read(&target).unwrap(), b"one\ntwo\n");
}

/// Upgrade path. A journal written before Write declared a
/// filesystem-transactional effect holds an opaque, receiptless record, and a
/// crash between durable prepare and durable start leaves that record to be
/// RETRIED by the new binary. The retry inherits the old record verbatim, so
/// the fresh declaration must be accepted as compatible with it and the
/// attempt must run the opaque path rather than the prepared one — otherwise
/// the physical dispatch and the durable description of it disagree, or the
/// call is refused outright for having changed shape.
#[tokio::test]
async fn a_pre_upgrade_opaque_write_record_can_still_be_retried() {
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join("upgraded.txt");
    std::fs::write(&target, b"one\n").unwrap();
    let (_dir, journal, scope) = effect_fixture();
    let registry = file_tool_registry();
    let input = json!({"file_path": target.to_string_lossy(), "content": "one\ntwo\n"});
    let call = tool_call("provider-call", "Write", input.clone());

    // The record an older binary would have left behind.
    let prior = scope
        .prepare_tool_with_contract(
            "provider-call",
            0,
            "Write",
            input.clone(),
            input,
            wcore_types::tool::ToolEffectContract::default(),
        )
        .unwrap();
    let prior_tool_execution_id = prior.id().to_owned();
    prior
        .not_started(ToolNotStartedReason::Cancelled {
            reason: "interrupted before durable tool start".into(),
        })
        .unwrap();
    let prior = journal.state().unwrap().tools[&prior_tool_execution_id].clone();
    assert_eq!(
        prior.effect_contract.kind,
        wcore_types::tool::ToolEffectKind::Opaque
    );
    assert!(prior.effect_receipt.is_none());

    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer: Arc<dyn ProtocolEmitter> = Arc::new(NoopEmitter);
    let outcome = execute_recovered_retry_tool_call_with_effects(
        &registry,
        &call,
        &approval_manager,
        &writer,
        "msg",
        &["Write".into()],
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        &CancellationToken::new(),
        None,
        &scope,
        0,
        None,
        "provider-call",
        &prior_tool_execution_id,
        &prior.tool,
        prior.ordinal,
        &prior.effect_contract,
        prior.effect_receipt.as_ref(),
        &prior.requested_input_digest,
        &prior.effective_input_digest,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        matches!(
            outcome.results.first(),
            Some(ContentBlock::ToolResult { is_error, .. }) if !is_error
        ),
        "the retry must run, not be refused: {:?}",
        outcome.results
    );

    let state = journal.state().unwrap();
    let (_, retry) = state
        .tools
        .iter()
        .find(|(_, tool)| tool.retry_of.as_deref() == Some(&prior_tool_execution_id))
        .expect("the retry must be linked to the pre-upgrade record");
    assert!(matches!(retry.effect, ToolEffectState::Succeeded));
    assert_eq!(
        retry.effect_contract.kind,
        wcore_types::tool::ToolEffectKind::Opaque,
        "a retry inherits its record; it may not silently upgrade to a receipt \
         the record does not carry"
    );
    assert!(retry.effect_receipt.is_none());
    assert_eq!(std::fs::read(&target).unwrap(), b"one\ntwo\n");
}
