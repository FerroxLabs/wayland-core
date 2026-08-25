//! F13, reachable half: an interrupted Write or Edit must reconcile itself.
//!
//! The durable sequence a crash cuts into is exactly the one the dispatcher
//! runs: `Tool::prepare_effect` -> durable prepare (with the receipt, when the
//! tool produced one) -> `start` -> the physical write -> a terminal append.
//! These tests stop after the physical boundary and never make the terminal
//! append, which is what a `kill -9` between those two points leaves behind,
//! and then run the SAME production recovery path the engine runs at startup.
//!
//! Three arms, and the third is the one that keeps the other two honest:
//!
//! * the write definitely landed   -> resolved `Succeeded`, no operator
//! * the write definitely did not  -> resolved `NotStarted`, no operator
//! * neither                       -> stays `Unknown`, the operator is asked
//!
//! A reconciler that answers the third case is worse than no reconciler at
//! all, so it is asserted with the same weight as the two it can answer.

mod common;

use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use wcore_agent::engine::AgentEngine;
use wcore_agent::journal_effects::JournalEffectCoordinator;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_agent::session::SessionManager;
use wcore_agent::session_journal::{SessionEvent, ToolEffectState, ToolUnknownReason};
use wcore_tools::context::ToolContext;
use wcore_tools::edit::EditTool;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::unsaved_work::UnsavedWorkGuard;
use wcore_tools::vfs::RealFs;
use wcore_tools::write::WriteTool;
use wcore_tools::{NullToolOutputSink, Tool};

use common::{MockLlmProvider, configure_persisted_test_session, test_config};

const TURN: &str = "interrupted-turn";

fn tool_context() -> ToolContext {
    ToolContext::new(
        "f889-call",
        CancellationToken::new(),
        Arc::new(RealFs),
        None,
        Arc::new(NullToolOutputSink),
    )
}

fn write_tool() -> WriteTool {
    WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()))
}

fn edit_tool() -> EditTool {
    EditTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()))
}

/// What the crash cut into: whether the physical write got to run before the
/// process died, and whether anyone else touched the file afterwards.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Interruption {
    /// Killed after the bytes landed but before the terminal journal append.
    AfterThePhysicalWrite,
    /// Killed after `start` but before the write reached the filesystem.
    BeforeThePhysicalWrite,
    /// Killed before the write, and a third party then rewrote the target.
    BeforeTheWriteAndThenAThirdPartyWrote,
}

struct Interrupted {
    _root: TempDir,
    engine: AgentEngine,
    tool_execution_id: String,
    target: std::path::PathBuf,
    prepared_a_receipt: bool,
}

/// Drive one tool call through the dispatcher's durable sequence and stop
/// where a crash would, leaving the execution nonterminal on disk.
async fn interrupt(tool: &dyn Tool, input: Value, cut: Interruption) -> Interrupted {
    let root = tempfile::tempdir().expect("f889 test root");
    let target = std::path::PathBuf::from(
        input["file_path"]
            .as_str()
            .expect("every arm targets a file"),
    );

    let manager = SessionManager::new(root.path().join("sessions"), 10);
    let active = manager
        .create_for_run("test-provider", "test-model", "/tmp", None)
        .expect("durable session");
    active
        .journal
        .append(SessionEvent::TurnStarted {
            turn_id: TURN.into(),
            user_message: "F13 interrupted file effect".into(),
        })
        .expect("turn start");

    let ctx = tool_context();
    let contract = tool.effect_contract(&input);

    // Exactly the dispatcher's order: prepare the runtime effect, encode its
    // durable receipt, checkpoint the preimage, then take durable authority.
    let prepared_runtime = tool
        .prepare_effect(&input, &ctx)
        .await
        .expect("preparation must not refuse a well-formed call");
    let durable_receipt = prepared_runtime
        .as_ref()
        .map(|prepared| prepared.durable_receipt().expect("encodable receipt"));
    let scope = JournalEffectCoordinator::new(active.journal.clone()).for_turn(TURN);
    if let Some(prepared) = prepared_runtime.as_ref()
        && let Some(preimage) = prepared.preimage_bytes()
    {
        let identity = prepared
            .filesystem_receipt()
            .checkpoint_identity()
            .expect("a present precondition carries its checkpoint identity");
        scope
            .store_effect_checkpoint(&identity.sha256, preimage)
            .expect("checkpoint the preimage");
    }

    let prepared_a_receipt = durable_receipt.is_some();
    let lease = match durable_receipt {
        Some(receipt) => scope.prepare_tool_with_effect_receipt(
            "provider-call",
            0,
            tool.name(),
            input.clone(),
            input.clone(),
            contract,
            receipt,
        ),
        None => scope.prepare_tool_with_contract(
            "provider-call",
            0,
            tool.name(),
            input.clone(),
            input.clone(),
            contract,
        ),
    }
    .expect("durable tool intent");
    let running = lease.start().expect("durable running boundary");
    let tool_execution_id = running.id().to_owned();

    if cut == Interruption::AfterThePhysicalWrite {
        let result = match prepared_runtime {
            Some(prepared) => tool.execute_prepared_effect(prepared, &ctx).await.result,
            None => tool.execute_with_ctx(input, &ctx).await,
        };
        assert!(
            !result.is_error,
            "physical write failed: {}",
            result.content
        );
    }
    if cut == Interruption::BeforeTheWriteAndThenAThirdPartyWrote {
        std::fs::write(&target, b"bytes from somebody else entirely\n").expect("third-party write");
    }
    // The crash: the started lease is dropped with no terminal append.
    drop(running);

    let mut config = test_config();
    configure_persisted_test_session(&mut config, root.path());
    config.session.directory = root.path().join("sessions").to_string_lossy().into_owned();
    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));
    let engine = AgentEngine::resume_active_with_provider(
        Arc::new(MockLlmProvider::with_text_response("unused")),
        config,
        ToolRegistry::new(),
        output,
        active,
    );

    Interrupted {
        _root: root,
        engine,
        tool_execution_id,
        target,
        prepared_a_receipt,
    }
}

/// Run the production startup reconciliation over the interrupted session.
async fn reconcile(state: &mut Interrupted) -> Result<(), String> {
    let plan = state.engine.recovery_plan().expect("recovery plan");
    let cursor = plan.cursor();
    state
        .engine
        .reconcile_interrupted_turn(TURN, &cursor)
        .await
        .map_err(|error| error.to_string())
}

fn effect(state: &Interrupted) -> ToolEffectState {
    state
        .engine
        .session_journal()
        .expect("journal")
        .state()
        .expect("reduced state")
        .tools[&state.tool_execution_id]
        .effect
        .clone()
}

fn write_input(target: &std::path::Path, content: &str) -> Value {
    json!({ "file_path": target.to_string_lossy(), "content": content })
}

// ---------------------------------------------------------------------------
// Arm 1 — the write definitely landed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_interrupted_write_that_landed_reconciles_to_succeeded_without_an_operator() {
    let staging = tempfile::tempdir().unwrap();
    let target = staging.path().join("landed.txt");
    std::fs::write(&target, b"before\n").unwrap();

    let mut state = interrupt(
        &write_tool(),
        write_input(&target, "before\nafter\n"),
        Interruption::AfterThePhysicalWrite,
    )
    .await;
    assert!(
        state.prepared_a_receipt,
        "Write must prepare a durable filesystem receipt; without one recovery \
         has nothing to reconcile against and every crash becomes a question \
         for a human"
    );
    assert_eq!(std::fs::read(&state.target).unwrap(), b"before\nafter\n");

    reconcile(&mut state)
        .await
        .expect("a landed write must reconcile itself");
    assert!(
        matches!(effect(&state), ToolEffectState::Succeeded),
        "expected Succeeded, got {:?}",
        effect(&state)
    );
    assert_eq!(std::fs::read(&state.target).unwrap(), b"before\nafter\n");
}

#[tokio::test]
async fn an_interrupted_edit_that_landed_reconciles_to_succeeded_without_an_operator() {
    let staging = tempfile::tempdir().unwrap();
    let target = staging.path().join("landed-edit.txt");
    std::fs::write(&target, b"hello world\n").unwrap();

    let input = json!({
        "file_path": target.to_string_lossy(),
        "old_string": "hello",
        "new_string": "goodbye",
    });
    let mut state = interrupt(&edit_tool(), input, Interruption::AfterThePhysicalWrite).await;
    assert!(
        state.prepared_a_receipt,
        "Edit must prepare a durable filesystem receipt"
    );
    assert_eq!(std::fs::read(&state.target).unwrap(), b"goodbye world\n");

    reconcile(&mut state)
        .await
        .expect("a landed edit must reconcile itself");
    assert!(
        matches!(effect(&state), ToolEffectState::Succeeded),
        "expected Succeeded, got {:?}",
        effect(&state)
    );
}

// ---------------------------------------------------------------------------
// Arm 2 — the write definitely did not land.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_interrupted_write_that_never_landed_reconciles_to_not_started() {
    let staging = tempfile::tempdir().unwrap();
    let target = staging.path().join("never-landed.txt");
    std::fs::write(&target, b"before\n").unwrap();

    let mut state = interrupt(
        &write_tool(),
        write_input(&target, "before\nafter\n"),
        Interruption::BeforeThePhysicalWrite,
    )
    .await;
    assert!(state.prepared_a_receipt);
    assert_eq!(std::fs::read(&state.target).unwrap(), b"before\n");

    reconcile(&mut state)
        .await
        .expect("an untouched target must reconcile itself");
    assert!(
        matches!(effect(&state), ToolEffectState::NotStarted),
        "expected NotStarted, got {:?}",
        effect(&state)
    );
    assert_eq!(
        std::fs::read(&state.target).unwrap(),
        b"before\n",
        "reconciliation is read-only and must never write to the target"
    );
}

#[tokio::test]
async fn an_interrupted_write_that_never_created_its_file_reconciles_to_not_started() {
    let staging = tempfile::tempdir().unwrap();
    let target = staging.path().join("never-created.txt");

    let mut state = interrupt(
        &write_tool(),
        write_input(&target, "brand new\n"),
        Interruption::BeforeThePhysicalWrite,
    )
    .await;
    assert!(state.prepared_a_receipt);
    assert!(!state.target.exists());

    reconcile(&mut state)
        .await
        .expect("an absent target must reconcile itself");
    assert!(
        matches!(effect(&state), ToolEffectState::NotStarted),
        "expected NotStarted, got {:?}",
        effect(&state)
    );
    assert!(!state.target.exists());
}

// ---------------------------------------------------------------------------
// Arm 3 — cannot tell. This one must stay a question for a human.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_third_party_write_between_prepare_and_recovery_stays_unknown() {
    let staging = tempfile::tempdir().unwrap();
    let target = staging.path().join("contended.txt");
    std::fs::write(&target, b"before\n").unwrap();

    let mut state = interrupt(
        &write_tool(),
        write_input(&target, "before\nafter\n"),
        Interruption::BeforeTheWriteAndThenAThirdPartyWrote,
    )
    .await;
    assert!(state.prepared_a_receipt);

    let error = reconcile(&mut state)
        .await
        .expect_err("a target matching neither identity must not be guessed at");
    assert!(
        error.contains("reconciliation"),
        "recovery must say the effect still needs reconciling: {error}"
    );
    assert!(
        matches!(
            effect(&state),
            ToolEffectState::Unknown {
                reason: ToolUnknownReason::Interrupted,
                ..
            }
        ),
        "expected Unknown, got {:?}",
        effect(&state)
    );
    assert_eq!(
        std::fs::read(&state.target).unwrap(),
        b"bytes from somebody else entirely\n",
        "reconciliation must not touch a contended target"
    );
}
