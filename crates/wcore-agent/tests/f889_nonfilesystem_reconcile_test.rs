//! wayland#889, the non-filesystem half: shell, network and MCP.
//!
//! The filesystem reconciler answers "did this write land?" by re-reading the
//! target against a prepared receipt. Nothing equivalent exists for a shell
//! command, an HTTP request or a remote MCP call — there is no target to
//! re-read — so every one of them, interrupted by a crash, became a question
//! for a human.
//!
//! There is a second answer available for exactly one class, and it needs no
//! evidence about the world at all: an invocation that COULD NOT have changed
//! anything has nothing for an operator to have an opinion about. These tests
//! drive one tool call to the durable `Running` boundary, stop where a
//! `kill -9` would, and then run the SAME production recovery the engine runs
//! at startup.
//!
//! Every certified arm is paired with its refusal, because a reconciler that
//! answers the case it cannot prove is worse than no reconciler at all:
//!
//! * `ls -la`                     settles | `rm -rf …` still asks
//! * `WebFetch`                   settles | the web tool's `crawl` still asks
//! * MCP `readOnlyHint: true`     settles | an MCP tool with no hint still asks
//!
//! And the arm that keeps all six honest: a contract that declares the
//! repeat-safe KIND while naming a reconciler this build does not register
//! settles NOTHING. Recovery dispatches on the name.

mod common;

use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::TempDir;
use wcore_agent::engine::AgentEngine;
use wcore_agent::journal_effects::JournalEffectCoordinator;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_agent::session::SessionManager;
use wcore_agent::session_journal::{SessionEvent, ToolEffectState, ToolResolutionSource};
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::tool::{ToolEffectContract, ToolEffectKind};

use common::{MockLlmProvider, configure_persisted_test_session, test_config};

const TURN: &str = "interrupted-turn";

struct Interrupted {
    _root: TempDir,
    engine: AgentEngine,
    tool_execution_id: String,
}

/// Take one tool call through the dispatcher's durable sequence as far as a
/// crash would let it get — prepared, then `Running` — and stop there. No
/// physical dispatch happens: none of these tools produces a receipt, so what
/// recovery has to work with is the contract and nothing else, which is
/// exactly what a process killed mid-call leaves behind.
async fn interrupt(tool_name: &str, input: Value, contract: ToolEffectContract) -> Interrupted {
    let root = tempfile::tempdir().expect("f889 test root");
    let manager = SessionManager::new(root.path().join("sessions"), 10);
    let active = manager
        .create_for_run("test-provider", "test-model", "/tmp", None)
        .expect("durable session");
    active
        .journal
        .append(SessionEvent::TurnStarted {
            turn_id: TURN.into(),
            user_message: "f889 interrupted non-filesystem effect".into(),
        })
        .expect("turn start");

    let scope = JournalEffectCoordinator::new(active.journal.clone()).for_turn(TURN);
    let lease = scope
        .prepare_tool_with_contract(
            "provider-call",
            0,
            tool_name,
            input.clone(),
            input,
            contract,
        )
        .expect("durable tool intent");
    let running = lease.start().expect("durable running boundary");
    let tool_execution_id = running.id().to_owned();
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
    }
}

/// Interrupt a REAL tool, using the contract that tool declares for that
/// exact input. Nothing about the contract is invented by the test.
async fn interrupt_tool(tool: &dyn Tool, input: Value) -> Interrupted {
    let contract = tool.effect_contract(&input);
    interrupt(tool.name(), input, contract).await
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

fn resolution_source(state: &Interrupted) -> Option<ToolResolutionSource> {
    state
        .engine
        .session_journal()
        .expect("journal")
        .state()
        .expect("reduced state")
        .tools[&state.tool_execution_id]
        .resolution_source
        .clone()
}

/// The certified shape: recovery completes, the effect is terminal, and the
/// receipt names the reconciler that certified it rather than a human.
async fn assert_settles_without_an_operator(state: &mut Interrupted, reconciler: &str) {
    reconcile(state)
        .await
        .unwrap_or_else(|error| panic!("recovery must not need an operator here: {error}"));
    assert!(
        matches!(effect(state), ToolEffectState::NotStarted),
        "a certified repeat-safe effect settles as not-started; got {:?}",
        effect(state)
    );
    assert_eq!(
        resolution_source(state),
        Some(ToolResolutionSource::Reconciler {
            reconciler: reconciler.to_owned()
        }),
        "the receipt must attribute the decision to the reconciler that made it"
    );
}

/// The refusal shape: recovery REFUSES to continue and the effect is still an
/// unanswered unknown waiting for a person.
async fn assert_still_needs_an_operator(state: &mut Interrupted) {
    let error = reconcile(state)
        .await
        .expect_err("an unprovable effect must block, not be guessed at");
    assert!(
        error.contains("reconciliation"),
        "recovery must say the effect still needs reconciling; got: {error}"
    );
    assert!(
        matches!(effect(state), ToolEffectState::Unknown { .. }),
        "an unprovable effect stays unknown; got {:?}",
        effect(state)
    );
    assert_eq!(
        resolution_source(state),
        None,
        "nothing may write a receipt for an effect nobody can vouch for"
    );
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_interrupted_read_only_shell_command_settles_without_an_operator() {
    let mut state = interrupt_tool(
        &wcore_tools::bash::BashTool,
        json!({ "command": "ls -la src" }),
    )
    .await;
    assert_settles_without_an_operator(&mut state, wcore_types::tool::READ_ONLY_SHELL_RECONCILER)
        .await;
}

#[tokio::test]
async fn an_interrupted_mutating_shell_command_still_needs_an_operator() {
    let mut state = interrupt_tool(
        &wcore_tools::bash::BashTool,
        json!({ "command": "rm -rf build" }),
    )
    .await;
    assert_still_needs_an_operator(&mut state).await;
}

/// The one that would be easiest to get wrong: the command STARTS with a
/// program on the read-only list and then redirects into a file.
#[tokio::test]
async fn an_interrupted_redirecting_shell_command_still_needs_an_operator() {
    let mut state = interrupt_tool(
        &wcore_tools::bash::BashTool,
        json!({ "command": "cat src/lib.rs > /tmp/copy" }),
    )
    .await;
    assert_still_needs_an_operator(&mut state).await;
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_interrupted_web_fetch_settles_without_an_operator() {
    let mut state = interrupt_tool(
        &wcore_tools::web_fetch::WebFetchTool::default(),
        json!({ "url": "https://example.com" }),
    )
    .await;
    assert_settles_without_an_operator(&mut state, wcore_types::tool::READ_ONLY_NETWORK_RECONCILER)
        .await;
}

#[tokio::test]
async fn an_interrupted_web_search_settles_without_an_operator() {
    let mut state = interrupt_tool(
        &wcore_tools::web_tools::WebTool::default(),
        json!({ "operation": "search", "query": "rust" }),
    )
    .await;
    assert_settles_without_an_operator(&mut state, wcore_types::tool::READ_ONLY_NETWORK_RECONCILER)
        .await;
}

/// A crawl creates a job at the backend. That is a state change and it is not
/// classified, so it keeps the operator question it always had.
#[tokio::test]
async fn an_interrupted_web_crawl_still_needs_an_operator() {
    let mut state = interrupt_tool(
        &wcore_tools::web_tools::WebTool::default(),
        json!({ "operation": "crawl", "url": "https://example.com" }),
    )
    .await;
    assert_still_needs_an_operator(&mut state).await;
}

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------

fn mcp_proxy(annotations: Option<wcore_mcp::protocol::McpToolAnnotations>) -> impl Tool {
    wcore_mcp::tool_proxy::McpToolProxy::new(
        "remote_query".into(),
        "remote_query".into(),
        "some-server".into(),
        "a tool on a user-configured MCP server".into(),
        json!({"type": "object"}),
        Arc::new(wcore_mcp::manager::McpManager::new_for_test(vec![])),
        false,
    )
    .with_annotations(annotations)
}

#[tokio::test]
async fn an_interrupted_read_only_mcp_call_settles_without_an_operator() {
    let tool = mcp_proxy(Some(wcore_mcp::protocol::McpToolAnnotations {
        read_only_hint: Some(true),
        ..wcore_mcp::protocol::McpToolAnnotations::default()
    }));
    let mut state = interrupt_tool(&tool, json!({ "q": "anything" })).await;
    assert_settles_without_an_operator(&mut state, wcore_types::tool::READ_ONLY_MCP_RECONCILER)
        .await;
}

#[tokio::test]
async fn an_interrupted_mcp_call_with_no_declaration_still_needs_an_operator() {
    let tool = mcp_proxy(None);
    let mut state = interrupt_tool(&tool, json!({ "q": "anything" })).await;
    assert_still_needs_an_operator(&mut state).await;
}

/// A server that claims read-only and destructive in the same breath has
/// contradicted itself. Picking the convenient half would write a durable
/// receipt on a contradiction.
#[tokio::test]
async fn an_interrupted_self_contradicting_mcp_call_still_needs_an_operator() {
    let tool = mcp_proxy(Some(wcore_mcp::protocol::McpToolAnnotations {
        read_only_hint: Some(true),
        destructive_hint: Some(true),
        ..wcore_mcp::protocol::McpToolAnnotations::default()
    }));
    let mut state = interrupt_tool(&tool, json!({ "q": "anything" })).await;
    assert_still_needs_an_operator(&mut state).await;
}

// ---------------------------------------------------------------------------
// Filesystem reads — the wiring the engine never had
// ---------------------------------------------------------------------------

/// `Read` has declared `RepeatSafe` since well before this issue, and
/// `wayland-core session cancel` has always settled it without asking. The
/// ENGINE's own startup recovery never did: it ran the filesystem receipt
/// reconciler and nothing else, so a crash during a plain `Read` left an
/// unresolved effect that refused to let the session resume.
#[tokio::test]
async fn an_interrupted_read_settles_without_an_operator() {
    let mut state = interrupt_tool(
        &wcore_tools::read::ReadTool::new(None),
        json!({ "file_path": "/etc/hostname" }),
    )
    .await;
    assert_settles_without_an_operator(
        &mut state,
        wcore_types::tool::READ_ONLY_FILESYSTEM_RECONCILER,
    )
    .await;
}

// ---------------------------------------------------------------------------
// The arm that keeps the other six honest
// ---------------------------------------------------------------------------

/// Recovery dispatches on the reconciler NAME, not on the kind.
///
/// This is the same contract as the certified shell arm with one character
/// changed in the identifier. If the kind alone were enough, this would settle
/// — and then any tool at all, including a plugin or a remote MCP proxy, could
/// mint a durable "nothing happened" receipt for itself just by declaring
/// `RepeatSafe`.
#[tokio::test]
async fn a_repeat_safe_contract_naming_an_unregistered_reconciler_settles_nothing() {
    let registered = wcore_types::tool::READ_ONLY_SHELL_RECONCILER;
    assert!(
        wcore_types::tool::repeat_safe_reconciler_is_registered(registered),
        "positive control: the real name is registered"
    );
    let unregistered = format!("{registered}9");
    assert!(!wcore_types::tool::repeat_safe_reconciler_is_registered(
        &unregistered
    ));

    let mut state = interrupt(
        "Bash",
        json!({ "command": "ls -la src" }),
        ToolEffectContract {
            kind: ToolEffectKind::RepeatSafe,
            reconciler: Some(unregistered),
        },
    )
    .await;
    assert_still_needs_an_operator(&mut state).await;
}

/// The other half of the same rule: the KIND is load-bearing too. A tool
/// naming a registered reconciler while declaring itself opaque gets nothing.
#[tokio::test]
async fn an_opaque_contract_naming_a_registered_reconciler_settles_nothing() {
    let mut state = interrupt(
        "Bash",
        json!({ "command": "ls -la src" }),
        ToolEffectContract {
            kind: ToolEffectKind::Opaque,
            reconciler: Some(wcore_types::tool::READ_ONLY_SHELL_RECONCILER.to_owned()),
        },
    )
    .await;
    assert_still_needs_an_operator(&mut state).await;
}
