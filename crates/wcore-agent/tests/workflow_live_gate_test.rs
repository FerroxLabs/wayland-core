//! B6 — integration tests for the LIVE workflow confirm gate.
//!
//! The gate is a PRE-LLM intercept in `AgentEngine::run`: it fires ONCE, after
//! `push_user_turn`/WAL setup and BEFORE the turn loop runs any model turn. When
//! `observability.workflow_live_mode` is on AND the user's input looks like a
//! workflow candidate AND both an approval manager and protocol writer are
//! wired, the engine:
//!   1. synthesises a `WorkflowPlan` (one sub-agent LLM call),
//!   2. emits `ToolRequest { tool.name == "Workflow" }` then `ApprovalRequired`,
//!   3. awaits approval (racing a session-root cancel),
//!   4. on `Approved` runs the workflow and RETURNS its result as the run output
//!      WITHOUT ever running a model turn,
//!   5. on Denied / cancel / synthesis-failure falls through to a normal turn.
//!
//! Placement note: because the gate runs BEFORE any model turn, the mock no
//! longer needs to emit a `tool_use` to reach it. On the approved path the
//! FIRST mock call is synthesis (RON) and the normal-turn mock response is never
//! consumed (proving pre-LLM interception). On every fall-through path the first
//! relevant mock call after synthesis is the normal turn the loop then runs.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::test_config;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_protocol::ToolApprovalManager;
use wcore_protocol::commands::ApprovalScope;
use wcore_protocol::events::{ProtocolEvent, ToolStatus};
use wcore_protocol::writer::ProtocolEmitter;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// A valid RON workflow with a single agent stage — enough to estimate, emit,
/// and run end-to-end through the runner.
const VALID_RON: &str = r#"Workflow(
    meta: (name: "audit-flow", description: "audit the repo", est_agents: 1),
    phases: [Phase(title: "scan", steps: [
        Agent((id: "scan", prompt: "scan the codebase")),
    ])],
)"#;

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
    }
}

/// A turn that emits `text` then ends.
fn text_turn(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: usage(),
        },
    ]
}

/// Returns a pre-configured event sequence per `stream` call, in order. Past
/// the configured list it falls back to an empty `EndTurn` (matching the shared
/// `MockLlmProvider` tail) so workflow-execution sub-agents resolve cleanly with
/// empty stage output. Shared across the engine's main stream AND every
/// sub-agent spawn because it is held behind `Arc`.
struct SequencedProvider {
    turns: Mutex<Vec<Vec<LlmEvent>>>,
    cursor: Mutex<usize>,
}

impl SequencedProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(turns),
            cursor: Mutex::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for SequencedProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let events = {
            let n = {
                let mut c = self.cursor.lock().unwrap();
                let v = *c;
                *c += 1;
                v
            };
            self.turns
                .lock()
                .unwrap()
                .get(n)
                .cloned()
                .unwrap_or_else(|| {
                    vec![LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                        usage: TokenUsage::default(),
                    }]
                })
        };
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in events {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A `ProtocolEmitter` that records every emitted event so a test can assert
/// emission order and pull the `call_id` out of the `ApprovalRequired` event.
#[derive(Default)]
struct CapturingEmitter {
    events: Mutex<Vec<ProtocolEvent>>,
}

impl ProtocolEmitter for CapturingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

impl CapturingEmitter {
    /// The first `ApprovalRequired`'s `call_id`, or `None` if none was emitted.
    fn approval_call_id(&self) -> Option<String> {
        self.events.lock().unwrap().iter().find_map(|e| match e {
            ProtocolEvent::ApprovalRequired { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
    }

    /// Index of the first `ToolRequest` whose tool name is "Workflow".
    fn workflow_tool_request_index(&self) -> Option<usize> {
        self.events.lock().unwrap().iter().position(
            |e| matches!(e, ProtocolEvent::ToolRequest { tool, .. } if tool.name == "Workflow"),
        )
    }

    /// Index of the first `ApprovalRequired`.
    fn approval_required_index(&self) -> Option<usize> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .position(|e| matches!(e, ProtocolEvent::ApprovalRequired { .. }))
    }

    /// The args `Value` from the first "Workflow" `ToolRequest`.
    fn workflow_args(&self) -> Option<serde_json::Value> {
        self.events.lock().unwrap().iter().find_map(|e| match e {
            ProtocolEvent::ToolRequest { tool, .. } if tool.name == "Workflow" => {
                Some(tool.args.clone())
            }
            _ => None,
        })
    }

    /// The `(call_id, is_error)` of the terminal `ToolResult` closing the
    /// Workflow card, if one was emitted. Without it the TUI card is stuck in
    /// `AwaitingApproval` and json-stream hosts never see the call resolve.
    fn workflow_tool_result(&self) -> Option<(String, bool)> {
        self.events.lock().unwrap().iter().find_map(|e| match e {
            ProtocolEvent::ToolResult {
                call_id,
                tool_name,
                status,
                ..
            } if tool_name == "Workflow" => {
                Some((call_id.clone(), matches!(status, ToolStatus::Error)))
            }
            _ => None,
        })
    }

    /// The `call_id` of the first `ToolCancelled` event, if any.
    fn tool_cancelled_call_id(&self) -> Option<String> {
        self.events.lock().unwrap().iter().find_map(|e| match e {
            ProtocolEvent::ToolCancelled { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
    }

    /// Every `Info` event's message, in emission order (GAP-5/7 progress +
    /// fall-through notices).
    fn info_messages(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ProtocolEvent::Info { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }
}

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// Build an engine wired with the live gate ON, the given provider, an approval
/// manager, and a capturing emitter. Returns the engine plus the shared manager
/// and emitter handles.
fn live_engine(
    provider: Arc<dyn LlmProvider>,
) -> (AgentEngine, Arc<ToolApprovalManager>, Arc<CapturingEmitter>) {
    // `auto_approve = true` so a fall-through turn-0 tool call (deny / off /
    // synthesis-fail paths) dispatches without parking on its own approval —
    // the gate's OWN approval round-trip is independent of this flag.
    let mut config = test_config();
    config.tools.auto_approve = true;
    config.observability.workflow_live_mode = true;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::ExecMockTool::new("noop", "tool output")));
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let emitter = Arc::new(CapturingEmitter::default());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output());
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(emitter.clone());
    (engine, approval_manager, emitter)
}

/// Spawn a task that approves whatever call_id the gate registered, by polling
/// the capturing emitter for the `ApprovalRequired` event (the gate mints a
/// fresh uuid the test cannot predict).
fn approve_when_pending(manager: Arc<ToolApprovalManager>, emitter: Arc<CapturingEmitter>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            if let Some(call_id) = emitter.approval_call_id() {
                manager.approve(&call_id, ApprovalScope::Once, None);
                break;
            }
        }
    });
}

/// Same as `approve_when_pending` but DENIES the request.
fn deny_when_pending(manager: Arc<ToolApprovalManager>, emitter: Arc<CapturingEmitter>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            if let Some(call_id) = emitter.approval_call_id() {
                manager.resolve(
                    &call_id,
                    wcore_protocol::ToolApprovalResult::Denied {
                        reason: "user declined".into(),
                    },
                );
                break;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 1. Live-mode + workflow prompt + valid RON + background-APPROVE:
//    ToolRequest(Workflow) then ApprovalRequired emitted in order; the runner
//    runs and the turn yields the workflow result.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn live_gate_approved_runs_workflow_and_yields_result() {
    // PRE-LLM intercept: call 0 = synthesis (RON); calls 1.. = workflow
    // execution sub-agents (empty-EndTurn tail). Nothing else is queued: if the
    // gate failed to intercept and the parent turn loop ran a model turn, that
    // turn would have to pull from the empty-EndTurn fallback and the run output
    // would NOT be the workflow completion summary.
    let provider = Arc::new(SequencedProvider::new(vec![text_turn(VALID_RON)]));
    let (mut engine, manager, emitter) = live_engine(provider);

    approve_when_pending(manager, emitter.clone());

    let result = engine
        .run("audit the entire codebase comprehensively", "msg-1")
        .await
        .expect("run should succeed");

    // The run output is the workflow result, not any model turn text.
    assert!(
        result.text.contains("audit-flow"),
        "run output should surface the workflow result; got: {}",
        result.text
    );
    assert!(
        result.text.contains("completed"),
        "approved workflow should render a completion summary; got: {}",
        result.text
    );
    // Pre-LLM interception proof: the gate returned the workflow result as a
    // SINGLE logical turn, before the turn loop ran ANY model turn. A model turn
    // (had the gate not intercepted) would increment past 1, and its output —
    // not the workflow summary — would be the run text.
    assert_eq!(
        result.turns, 1,
        "approved gate returns the workflow as a single turn with no model turn; got {}",
        result.turns
    );

    // Emission order: ToolRequest(Workflow) strictly before ApprovalRequired.
    let tr = emitter
        .workflow_tool_request_index()
        .expect("a Workflow ToolRequest must be emitted");
    let ar = emitter
        .approval_required_index()
        .expect("an ApprovalRequired must be emitted");
    assert!(
        tr < ar,
        "ToolRequest(Workflow) must precede ApprovalRequired (got {tr} then {ar})"
    );

    // Args contract the TUI card reads.
    let args = emitter.workflow_args().expect("Workflow args present");
    assert_eq!(args["name"], "audit-flow");
    assert_eq!(args["steps"], 1);
    assert!(
        args["summary"]
            .as_str()
            .is_some_and(|s| s.starts_with("~1 agents / ~$")),
        "summary must be the '~N agents / ~$X' string; got: {:?}",
        args["summary"]
    );

    // The card MUST be closed: a terminal `ToolResult` for the SAME call_id as
    // the approval, with success status. Without this the proposal card is
    // stuck in `AwaitingApproval` forever (the 2026-05-31 stuck-pill bug).
    let (result_call_id, is_error) = emitter
        .workflow_tool_result()
        .expect("approved run must emit a terminal ToolResult to close the card");
    assert!(
        !is_error,
        "a successful run must report ToolStatus::Success"
    );
    assert_eq!(
        Some(result_call_id),
        emitter.approval_call_id(),
        "the closing ToolResult must carry the same call_id as the proposal/approval"
    );
}

// ---------------------------------------------------------------------------
// 2. Background-DENY: the workflow does NOT run; the turn falls through to a
//    normal single-agent response.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn live_gate_denied_falls_through_to_normal_turn() {
    // PRE-LLM intercept: call 0 = synthesis RON; on deny the gate returns
    // `None`, the run falls through to the normal turn loop, and call 1 = the
    // model's normal answer ends the (first) model turn.
    let provider = Arc::new(SequencedProvider::new(vec![
        text_turn(VALID_RON),
        text_turn("normal answer after deny"),
    ]));
    let (mut engine, manager, emitter) = live_engine(provider);

    deny_when_pending(manager, emitter.clone());

    let result = engine
        .run("audit the entire codebase comprehensively", "msg-2")
        .await
        .expect("run should succeed");

    // The confirm round-trip still fired (gate proposed the workflow)...
    assert!(
        emitter.workflow_tool_request_index().is_some(),
        "the gate should still propose the workflow before the deny"
    );
    // ...but the workflow did NOT run: the turn output is the normal single-
    // agent response, not the workflow completion summary.
    assert!(
        !result.text.contains("audit-flow"),
        "denied workflow must not surface a workflow result; got: {}",
        result.text
    );
    assert_eq!(result.text, "normal answer after deny");

    // The proposal card MUST be resolved as cancelled — a `ToolCancelled` for
    // the approval call_id — so it does not linger in `AwaitingApproval` and
    // json-stream hosts see the declined call close out.
    assert_eq!(
        emitter.tool_cancelled_call_id(),
        emitter.approval_call_id(),
        "a declined gate must emit ToolCancelled for the proposal call_id"
    );
}

// ---------------------------------------------------------------------------
// 3. Cancel-race: cancelling the session-root token before approval resolves
//    `drop_pending` and falls through — no hang.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn live_gate_cancel_race_drops_pending_and_falls_through() {
    let provider = Arc::new(SequencedProvider::new(vec![
        text_turn(VALID_RON),
        text_turn("normal answer after cancel"),
    ]));
    let (mut engine, manager, emitter) = live_engine(provider);
    let cancel = engine.cancel_token();

    // Cancel as soon as the gate parks on the approval await (i.e. once the
    // ApprovalRequired event has been emitted). Never approve.
    let emitter_for_cancel = emitter.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            if emitter_for_cancel.approval_call_id().is_some() {
                cancel.cancel();
                break;
            }
        }
    });

    // The run still completes (no hang). After the cancel the gate returns
    // `None` and falls through; the turn loop's first between-turn cancel check
    // sees the cancelled token and returns `UserAborted`, OR (if the token is
    // cleared elsewhere) the normal turn completes first — either way it must
    // not hang and must not surface a workflow result.
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.run("audit the entire codebase comprehensively", "msg-3"),
    )
    .await
    .expect("run must not hang on a cancel-race");

    match outcome {
        Ok(result) => assert!(
            !result.text.contains("audit-flow"),
            "cancelled gate must not surface a workflow result; got: {}",
            result.text
        ),
        Err(_) => { /* UserAborted from the between-turn cancel check is fine */ }
    }

    // The pending approval entry was dropped (no leak): a fresh reap finds
    // nothing to collect.
    assert_eq!(
        manager.reap_now(),
        0,
        "drop_pending should have removed the entry; nothing left to reap"
    );
}

// ---------------------------------------------------------------------------
// 4. Live-mode OFF (default): the gate never fires — behaviour identical to
//    today. No Workflow ToolRequest, no ApprovalRequired, normal turn output.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn live_gate_off_by_default_is_no_op() {
    // With the gate OFF the pre-LLM intercept is a no-op: the run goes straight
    // to the turn loop and call 0 = the model's normal answer ends the turn.
    let provider = Arc::new(SequencedProvider::new(vec![text_turn(
        "normal answer after tool",
    )]));

    let mut config = test_config();
    config.tools.auto_approve = true;
    // workflow_live_mode defaults to false — do NOT enable it.
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::ExecMockTool::new("noop", "tool output")));
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let emitter = Arc::new(CapturingEmitter::default());
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output());
    engine.set_approval_manager(approval_manager);
    engine.set_protocol_writer(emitter.clone());

    let result = engine
        .run("audit the entire codebase comprehensively", "msg-4")
        .await
        .expect("run should succeed");

    assert_eq!(result.text, "normal answer after tool");
    assert!(
        emitter.workflow_tool_request_index().is_none(),
        "live gate OFF must not emit a Workflow ToolRequest"
    );
    assert!(
        emitter.approval_required_index().is_none(),
        "live gate OFF must not emit ApprovalRequired"
    );
}

// ---------------------------------------------------------------------------
// 5. Synthesis failure (model returns junk on every attempt): the gate falls
//    through to a normal turn with no panic, no workflow result.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn live_gate_synthesis_failure_falls_through() {
    // PRE-LLM intercept: synthesis retries up to MAX_SYNTH_ATTEMPTS (3) on a
    // missing/unparseable `Workflow(` block, so calls 0-2 are junk with no RON
    // block — synthesis exhausts its budget and returns `NoRonBlock`. The gate
    // then returns `None` and falls through to the turn loop, where call 3 = the
    // normal answer ends the turn.
    let provider = Arc::new(SequencedProvider::new(vec![
        text_turn("this is not RON at all"),
        text_turn("still just prose, no workflow block"),
        text_turn("nope, no Workflow( document here either"),
        text_turn("normal answer after synth-fail"),
    ]));
    let (mut engine, manager, emitter) = live_engine(provider);

    // Approve eagerly IF an approval is ever requested — it must NOT be, because
    // synthesis fails before the confirm round-trip is emitted.
    approve_when_pending(manager, emitter.clone());

    let result = engine
        .run("audit the entire codebase comprehensively", "msg-5")
        .await
        .expect("run should not panic on synthesis failure");

    assert_eq!(result.text, "normal answer after synth-fail");
    assert!(
        emitter.workflow_tool_request_index().is_none(),
        "failed synthesis must not emit a Workflow ToolRequest"
    );
    assert!(
        emitter.approval_required_index().is_none(),
        "failed synthesis must not emit ApprovalRequired"
    );
    // GAP-5/7: synthesis must not be silent. The user (in opt-in live mode) gets
    // a progress indicator while the up-to-3-round-trip synthesis runs, and a
    // one-line note when it fails so the plain answer that follows isn't an
    // unexplained surprise.
    let infos = emitter.info_messages();
    assert!(
        infos.iter().any(|m| m.contains("Designing a workflow")),
        "synthesis must emit a progress indicator; got {infos:?}"
    );
    assert!(
        infos
            .iter()
            .any(|m| m.contains("Couldn't design a workflow")),
        "a failed synthesis must leave a fall-through note; got {infos:?}"
    );
}
