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

// Workspace-authority propagation/denial coverage (below) drives the REAL
// production spawner composition Bootstrap installs on the workflow/spawner
// path, so the same imports the durable child launch uses are pulled in here.
use common::bind_test_spawner;
use wcore_agent::spawner::{
    AgentSpawner, DurableSpawner, DurableSpawnerError, ForkOverrides, ResolvedChildLaunch,
    SubAgentConfig,
};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildRecoveryState,
    ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus, RequestedChildWorkspace,
};

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
) -> (
    AgentEngine,
    Arc<ToolApprovalManager>,
    Arc<CapturingEmitter>,
    tempfile::TempDir,
) {
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
    let session_root = common::bind_test_engine(&mut engine);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(emitter.clone());
    (engine, approval_manager, emitter, session_root)
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
    let (mut engine, manager, emitter, _session_root) = live_engine(provider);

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
    let (mut engine, manager, emitter, _session_root) = live_engine(provider);

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
    let (mut engine, manager, emitter, _session_root) = live_engine(provider);
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
    let (mut engine, manager, emitter, _session_root) = live_engine(provider);

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

// ===========================================================================
// Production-path workspace-authority propagation and denials (f20-04 Task 3)
//
// The live workflow gate approves a plan and then runs it through the SAME
// `AgentSpawner` composition Bootstrap installs for the Spawn / workflow /
// Crucible / Anvil paths. These tests exercise that REAL spawner — not a
// mock `Spawner` adapter — end to end through its public durable-launch
// surface (`resolve_durable_launch`, `spawn_one`, `validate_record`,
// `declare_resolved_child`), asserting that a delegated workflow child can
// never read/write the parent workspace, upgrade shared read-only authority
// to mutation, run against a substituted transaction opening / session
// generation / parent snapshot, or proceed when the parent-workspace
// authority is unavailable — and that every refusal happens BEFORE any
// provider/tool execution and leaves the durable journal untouched.
//
// The denials asserted here live in the resolution/allocation/declaration
// layer, so they are platform-agnostic and run on macOS and the Linux gate
// alike; the isolated-checkout filesystem/git plumbing is proven separately
// on the Linux remote gate.
// ===========================================================================

/// A quiet provider for spawner construction. Workspace-authority denials
/// resolve before any model turn, so the scripted stream is never consumed.
fn quiet_provider() -> Arc<dyn LlmProvider> {
    Arc::new(SequencedProvider::new(vec![]))
}

/// An unpinned sub-agent request (inherits the spawner's provider), matching
/// what the workflow runner hands the spawner for a single agent stage.
fn sub_config(name: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.to_string(),
        prompt: "do the delegated work".to_string(),
        max_turns: 2,
        max_tokens: 128,
        system_prompt: None,
        provider: None,
        model: None,
        temperature: None,
    }
}

/// A durable child record whose correlation identity faithfully mirrors a
/// resolved launch. Every hostile test tampers exactly one field of a clone so
/// the specific `EvidenceMismatch` the validator raises is unambiguous.
fn faithful_record(launch: &ResolvedChildLaunch, request_digest: String) -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: "declare-workspace-authority".to_string(),
        child_id: launch.child_id().clone(),
        parent: ChildParent {
            session_id: launch.authority().session_id().to_string(),
            turn_id: None,
            parent_child_id: None,
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: None,
        },
        origin: ChildOrigin::Workflow,
        request: ChildRequestEvidence::redacted(request_digest),
        policy_snapshot: launch.policy_snapshot().clone(),
        provider: Some(launch.provider_id().to_string()),
        model: Some(launch.model().to_string()),
        workspace: launch.workspace().clone(),
        status: DurableChildStatus::Prepared,
        desired_state: ChildDesiredState::Run,
        recovery: ChildRecoveryState::Clean,
        revision: 0,
        timestamps: ChildTimestamps {
            created_at_unix_ms: 1,
            updated_at_unix_ms: 1,
            queued_at_unix_ms: None,
            started_at_unix_ms: None,
            terminal_at_unix_ms: None,
        },
        result: None,
        delivery_target: None,
        delivery_state: ChildDeliveryState::NotRequired,
        attempt: 1,
        retry_of: None,
        applied_events: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// 6. Classification downgrade is impossible: an empty/read-only tool set is
//    shared read-only, but ANY write-capable or unknown tool is conservatively
//    isolated-mutation. A shared child can never be mislabelled to acquire an
//    isolated checkout, and a mutating child can never collapse into shared.
// ---------------------------------------------------------------------------
#[test]
fn fork_overrides_classification_never_downgrades_mutation_to_shared() {
    let shared = |tools: &[&str]| {
        ForkOverrides {
            allowed_tools: tools.iter().map(|t| t.to_string()).collect(),
            ..ForkOverrides::default()
        }
        .requested_workspace()
    };

    assert_eq!(shared(&[]), RequestedChildWorkspace::SharedReadOnly);
    assert_eq!(
        shared(&["Read", "Grep", "Glob"]),
        RequestedChildWorkspace::SharedReadOnly
    );
    // A single write-capable tool forces mutation authority.
    assert_eq!(
        shared(&["Read", "Write"]),
        RequestedChildWorkspace::IsolatedMutation
    );
    assert_eq!(shared(&["Edit"]), RequestedChildWorkspace::IsolatedMutation);
    assert_eq!(shared(&["Bash"]), RequestedChildWorkspace::IsolatedMutation);
    // An unknown/misspelled tool is conservatively mutation-capable so a new
    // tool can never silently be treated as shared read-only.
    assert_eq!(
        shared(&["totally-unknown-tool"]),
        RequestedChildWorkspace::IsolatedMutation
    );
}

// ---------------------------------------------------------------------------
// 7. A mutating request is NEVER resolved inline as a shared launch: the
//    synchronous `resolve_durable_launch` refuses it, preserving a
//    workspace-authority diagnostic instead of realizing a shared checkout
//    with write-capable tools. (Isolated mutation requires the asynchronous
//    standalone-checkout preparation path proven on the Linux gate.)
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolve_durable_launch_refuses_inline_mutation() {
    let dir = tempfile::tempdir().expect("parent workspace root");
    let spawner = AgentSpawner::new(quiet_provider(), test_config())
        .with_parent_workspace(dir.path())
        .expect("bind canonical parent workspace");
    let (spawner, _journal, _root) = bind_test_spawner(spawner);

    let mutating = ForkOverrides {
        allowed_tools: vec!["Write".to_string()],
        ..ForkOverrides::default()
    };
    assert_eq!(
        mutating.requested_workspace(),
        RequestedChildWorkspace::IsolatedMutation,
        "a Write tool must classify as mutation authority"
    );

    let err = match spawner.resolve_durable_launch(sub_config("mutant"), mutating) {
        Ok(_) => panic!("a mutating child must never resolve as a shared inline launch"),
        Err(error) => error,
    };
    let message = err.to_string();
    assert!(
        matches!(err, DurableSpawnerError::WorkspacePreparation(_)),
        "mutation refusal must be a workspace-preparation denial; got: {message}"
    );
    // The branch depends on the platform sandbox backend: with a non-enforcing
    // backend `pre_resolve` refuses for lack of containment; with an enforcing
    // backend the shared-resolver refuses because mutation needs asynchronous
    // isolated preparation. Either way the diagnostic is preserved.
    assert!(
        message.contains("isolated-workspace preparation")
            || message.contains("enforcing sandbox backend"),
        "mutating resolve must preserve a workspace-authority diagnostic; got: {message}"
    );
}

// ---------------------------------------------------------------------------
// 8. Synchronous resolution fails closed when the parent-workspace authority
//    is unavailable: a durable-bound spawner with NO bound parent workspace
//    refuses to resolve even a shared read-only child, with a preserved
//    diagnostic — never a silent fallback to a process-global cwd.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolve_durable_launch_without_parent_workspace_authority_is_refused() {
    // Durable session bound, but `with_parent_workspace` deliberately skipped.
    let spawner = AgentSpawner::new(quiet_provider(), test_config());
    let (spawner, _journal, _root) = bind_test_spawner(spawner);

    let err = match spawner.resolve_durable_launch(sub_config("orphan"), ForkOverrides::default()) {
        Ok(_) => panic!("resolution must fail closed without parent workspace authority"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &err,
            DurableSpawnerError::WorkspacePreparation(message)
                if message.contains("parent workspace authority is not bound")
        ),
        "unbound parent-workspace authority must preserve its diagnostic; got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 9. The async spawn path fails closed at workspace preparation when the
//    parent-workspace authority is unbound: `spawn_one` reaches
//    `prepare_child_workspace`, which refuses because BOTH shared and isolated
//    modes require a bound parent. No child engine/turn or tool ever runs.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn spawn_prepare_child_workspace_fails_closed_without_parent_authority() {
    let spawner = AgentSpawner::new(quiet_provider(), test_config());
    let (spawner, _journal, _root) = bind_test_spawner(spawner);

    let result = spawner.spawn_one(sub_config("orphan-spawn")).await;
    assert!(
        result.is_error,
        "a spawn with no bound parent workspace must be a terminal error"
    );
    assert_eq!(
        result.turns, 0,
        "no child engine/turn may run when workspace authority is unbound"
    );
    assert!(
        result
            .text
            .contains("parent workspace authority is not bound"),
        "the fail-closed diagnostic must be preserved on the result; got: {}",
        result.text
    );
}

// ---------------------------------------------------------------------------
// 10. Binding a parent workspace itself refuses a missing root or a
//     non-directory root, so a bogus parent identity can never enter the
//     spawner in the first place.
// ---------------------------------------------------------------------------
#[test]
fn with_parent_workspace_rejects_missing_and_non_directory_roots() {
    let dir = tempfile::tempdir().expect("scratch root");

    let missing = dir.path().join("does-not-exist");
    let err =
        match AgentSpawner::new(quiet_provider(), test_config()).with_parent_workspace(&missing) {
            Ok(_) => panic!("a nonexistent parent workspace must be refused"),
            Err(error) => error,
        };
    assert!(
        matches!(err, DurableSpawnerError::WorkspacePreparation(_)),
        "missing parent workspace must be a workspace-preparation denial; got: {err}"
    );

    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"x").expect("write scratch file");
    let err = match AgentSpawner::new(quiet_provider(), test_config()).with_parent_workspace(&file)
    {
        Ok(_) => panic!("a non-directory parent workspace must be refused"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &err,
            DurableSpawnerError::WorkspacePreparation(message)
                if message.contains("not a directory")
        ),
        "a file parent root must preserve its diagnostic; got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 11. Correlation identity threads through resolution intact, and any rebind
//     of the resolved launch to a foreign child identity, parent session
//     (generation), request opening, provider, model, policy, or workspace
//     (parent-snapshot substitution) is refused by `validate_record` with a
//     precise, preserved diagnostic — before any provider/tool execution.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn resolved_child_launch_validates_identity_and_refuses_rebind() {
    let dir = tempfile::tempdir().expect("parent workspace root");
    let spawner = AgentSpawner::new(quiet_provider(), test_config())
        .with_parent_workspace(dir.path())
        .expect("bind canonical parent workspace");
    let (spawner, _journal, _root) = bind_test_spawner(spawner);

    let config = sub_config("correlated");
    let overrides = ForkOverrides::default();
    let request_digest =
        DurableSpawner::request_digest(&config, &overrides).expect("request digest");
    let launch = spawner
        .resolve_durable_launch(config.clone(), overrides.clone())
        .expect("a shared read-only child resolves against bound authority");

    // Shared read-only stays shared: the realized workspace is the parent, not
    // an isolated checkout, and it never carries authority-deny roots.
    assert_eq!(
        launch.requested_workspace(),
        RequestedChildWorkspace::SharedReadOnly
    );
    assert_eq!(launch.workspace().mode, ChildWorkspaceMode::SharedReadOnly);

    // Positive: the faithfully-correlated record validates.
    let faithful = faithful_record(&launch, request_digest.clone());
    launch
        .validate_record(&faithful)
        .expect("a faithfully-correlated record must validate");

    // Foreign child identity (checkout bound to another child).
    let mut foreign_child = faithful_record(&launch, request_digest.clone());
    foreign_child.child_id = ChildId::new("child-attacker").unwrap();
    assert!(matches!(
        launch.validate_record(&foreign_child),
        Err(DurableSpawnerError::EvidenceMismatch("child identity"))
    ));

    // Foreign parent session / generation rebind.
    let mut foreign_session = faithful_record(&launch, request_digest.clone());
    foreign_session.parent.session_id = "attacker-session".to_string();
    assert!(matches!(
        launch.validate_record(&foreign_session),
        Err(DurableSpawnerError::EvidenceMismatch("parent session"))
    ));

    // Substituted transaction opening (request evidence).
    let mut foreign_request = faithful_record(&launch, request_digest.clone());
    foreign_request.request =
        ChildRequestEvidence::redacted(std::iter::repeat_n('9', 64).collect::<String>());
    assert!(matches!(
        launch.validate_record(&foreign_request),
        Err(DurableSpawnerError::EvidenceMismatch("request digest"))
    ));

    // Provider / model substitution.
    let mut foreign_provider = faithful_record(&launch, request_digest.clone());
    foreign_provider.provider = Some("evil-provider".to_string());
    assert!(matches!(
        launch.validate_record(&foreign_provider),
        Err(DurableSpawnerError::EvidenceMismatch("provider"))
    ));

    let mut foreign_model = faithful_record(&launch, request_digest.clone());
    foreign_model.model = Some("evil-model".to_string());
    assert!(matches!(
        launch.validate_record(&foreign_model),
        Err(DurableSpawnerError::EvidenceMismatch("model"))
    ));

    // Policy-snapshot tamper (posture downgrade).
    let mut foreign_policy = faithful_record(&launch, request_digest.clone());
    foreign_policy.policy_snapshot.posture = "dangerous".to_string();
    assert!(matches!(
        launch.validate_record(&foreign_policy),
        Err(DurableSpawnerError::EvidenceMismatch("policy snapshot"))
    ));

    // Parent-snapshot substitution: claim an isolated checkout the shared child
    // never realized.
    let mut foreign_workspace = faithful_record(&launch, request_digest);
    foreign_workspace.workspace = ChildWorkspace {
        mode: ChildWorkspaceMode::Isolated,
        workspace_id: "isolated-attacker".to_string(),
    };
    assert!(matches!(
        launch.validate_record(&foreign_workspace),
        Err(DurableSpawnerError::EvidenceMismatch("workspace"))
    ));
}

// ---------------------------------------------------------------------------
// 12. A declaration carrying substituted workspace evidence is refused at the
//     journal boundary: `declare_resolved_child` validates under the store lock
//     and returns an error WITHOUT advancing the durable journal, so a rebind
//     attempt leaves no persisted child and no parent-state mutation.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn declare_resolved_child_refuses_substituted_workspace_without_journal_write() {
    let dir = tempfile::tempdir().expect("parent workspace root");
    let spawner = AgentSpawner::new(quiet_provider(), test_config())
        .with_parent_workspace(dir.path())
        .expect("bind canonical parent workspace");
    let (spawner, journal, _root) = bind_test_spawner(spawner);

    let config = sub_config("declared");
    let overrides = ForkOverrides::default();
    let request_digest =
        DurableSpawner::request_digest(&config, &overrides).expect("request digest");
    let launch = spawner
        .resolve_durable_launch(config, overrides)
        .expect("a shared read-only child resolves against bound authority");

    let before = journal.state().expect("journal state").last_seq;

    let mut substituted = faithful_record(&launch, request_digest);
    substituted.workspace = ChildWorkspace {
        mode: ChildWorkspaceMode::Isolated,
        workspace_id: "isolated-attacker".to_string(),
    };
    assert!(
        spawner
            .declare_resolved_child(&launch, substituted)
            .is_err(),
        "a substituted-workspace declaration must be refused"
    );
    assert_eq!(
        journal.state().expect("journal state").last_seq,
        before,
        "a refused declaration must not advance the durable journal"
    );
}
