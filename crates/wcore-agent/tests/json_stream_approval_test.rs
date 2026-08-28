mod common;

use std::sync::Arc;

use serde_json::json;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_protocol::writer::ProtocolWriter;
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{StopReason, TokenUsage};

use common::{ExecMockTool, MockLlmProvider, test_config};

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

fn token_usage(input: u64, output: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        ..Default::default()
    }
}

/// An exec-category tool that COUNTS its executions.
///
/// `ExecMockTool` cannot say whether it ran, which is why the two gate tests
/// below could not tell an approved run from an ungated one. The count is the
/// only thing that distinguishes "the engine parked, was answered, and then
/// dispatched" from "the engine never gated at all" — the canned second
/// provider turn is identical either way.
struct CountingExecTool {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl CountingExecTool {
    fn new() -> (Self, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Self {
                calls: calls.clone(),
            },
            calls,
        )
    }
}

#[async_trait::async_trait]
impl wcore_tools::Tool for CountingExecTool {
    fn name(&self) -> &str {
        "exec_tool"
    }
    fn description(&self) -> &str {
        "Counting mock exec tool"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Exec
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }
    async fn execute(&self, _input: serde_json::Value) -> wcore_types::tool::ToolResult {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        wcore_types::tool::ToolResult {
            content: "tool output".to_string(),
            is_error: false,
        }
    }
}

/// Wait for the engine to REGISTER a pending approval for `call_id`, then
/// answer it. Returns the tool name the manager had parked under that id, or
/// `None` if nothing was ever pending.
///
/// This replaces a resolver that computed `let has_pending = { true };` and
/// then resolved unconditionally: a `resolve` for an id that is not pending is
/// a no-op, so that loop answered nothing and proved nothing. `pending_tool_name`
/// is the manager's own view of its pending map, so `Some(..)` here is the
/// engine having actually parked the turn on the gate.
fn answer_when_parked(
    manager: Arc<ToolApprovalManager>,
    call_id: &'static str,
    result: ToolApprovalResult,
) -> tokio::task::JoinHandle<Option<String>> {
    tokio::spawn(async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(name) = manager.pending_tool_name(call_id) {
                manager.resolve(call_id, result);
                return Some(name);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
}

// ---------------------------------------------------------------------------
// test: tool approval approve flow
//
// LLM requests exec_tool → engine pauses at approval_manager.request_approval
// → background task observes the PENDING request and resolves it Approved →
// tool executes → LLM continues.
//
// # What this used to assert, and why it was not enough
//
// The resolver computed `let has_pending = { true };` and the test graded only
// the canned second provider turn (`result.text == "Done"`) and `turns == 2`.
// Both of those are produced by the mock regardless of whether an approval
// gate exists, so neither could see the gate. **Measured:** with
// `needs_approval` forced to `false` in `orchestration/mod.rs` — the whole HITL
// gate removed, every exec tool dispatching unasked — this test and the deny
// test below both passed.
//
// The two additions are the two facts the claim rests on: the engine PARKED
// (the manager really held a pending entry under this call id), and the tool
// really RAN after the approval.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_tool_approval_approve_flow() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-1".to_string(),
            name: "exec_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let (tool, calls) = CountingExecTool::new();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(tool));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    let resolver = answer_when_parked(
        approval_manager.clone(),
        "call-1",
        ToolApprovalResult::Approved { answer: None },
    );

    let result = engine
        .run("Use the tool", "msg-1")
        .await
        .expect("should succeed");
    let parked = resolver.await.expect("resolver task must not panic");

    assert_eq!(
        parked.as_deref(),
        Some("exec_tool"),
        "the engine never registered a pending approval for call-1, so the turn did not park \
         on the gate and the approval below answered nothing"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "an APPROVED tool must actually be dispatched exactly once"
    );
    assert_eq!(result.text, "Done");
    assert_eq!(result.turns, 2);
}

// ---------------------------------------------------------------------------
// test: tool approval deny flow
//
// LLM requests exec_tool → engine pauses → background observes the pending
// request and resolves it Denied → tool is NOT executed → the denial is fed
// back to the LLM → LLM responds with text.
//
// The tool-call count is the assertion that makes "denied" mean anything: the
// canned second turn says "Cannot run tool" whether or not the tool ran.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_tool_approval_deny_flow() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-2".to_string(),
            name: "exec_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Cannot run tool".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let (tool, calls) = CountingExecTool::new();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(tool));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    let resolver = answer_when_parked(
        approval_manager.clone(),
        "call-2",
        ToolApprovalResult::Denied {
            reason: "policy violation".into(),
        },
    );

    let result = engine
        .run("Use the tool", "msg-2")
        .await
        .expect("should succeed");
    let parked = resolver.await.expect("resolver task must not panic");

    assert_eq!(
        parked.as_deref(),
        Some("exec_tool"),
        "the engine never registered a pending approval for call-2: nothing was denied, the \
         tool simply was not gated"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a DENIED tool must never be dispatched - this is the whole of what the denial buys, \
         and the canned reply text below is identical either way"
    );
    assert_eq!(result.text, "Cannot run tool");
    assert_eq!(result.turns, 2);
}

// ---------------------------------------------------------------------------
// test: auto_approve bypasses approval wait
//
// With auto_approve=true, exec category tools should execute immediately
// without waiting for approval.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_auto_approve_bypasses_approval() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-3".to_string(),
            name: "exec_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Auto done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = true;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ExecMockTool::new("exec_tool", "tool output")));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    // Host-backed sessions use the manager as the sole live posture authority.
    // Production seeds it from the resolved config before installing it.
    approval_manager.set_mode(wcore_protocol::commands::SessionMode::Force);
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // No background task to approve. Bound the wait so future posture drift
    // fails this test instead of wedging the entire integration suite.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.run("Use the tool", "msg-3"),
    )
    .await
    .expect("Force mode must bypass the host approval wait")
    .expect("should succeed");
    assert_eq!(result.text, "Auto done");
    assert_eq!(result.turns, 2);
}

// ---------------------------------------------------------------------------
// test: session auto-approve (scope=always) bypasses future approvals
//
// After add_auto_approve("exec"), exec tools skip the approval wait.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_session_auto_approve_category() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-4".to_string(),
            name: "exec_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Session auto".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ExecMockTool::new("exec_tool", "tool output")));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    // Pre-approve the "exec" category
    approval_manager.add_auto_approve("exec");
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // No background task to approve — should not hang
    let result = engine
        .run("Use the tool", "msg-4")
        .await
        .expect("should succeed");
    assert_eq!(result.text, "Session auto");
    assert_eq!(result.turns, 2);
}

// ---------------------------------------------------------------------------
// test: W0 prefix-scoped auto-approve bypasses the wait for a matching command
//
// A pre-registered AlwaysPrefix{"cargo "} rule for the exec category means a
// `cargo ...` command auto-approves WITHOUT a background approver. If the gate
// did not thread the command string into is_auto_approved_cmd, this run would
// park on the approval await and the timeout would fire.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_prefix_scoped_auto_approve_bypasses_matching_command() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-pfx-1".to_string(),
            name: "exec_tool".to_string(),
            input: json!({ "command": "cargo test --lib" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Prefix auto".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ExecMockTool::new("exec_tool", "tool output")));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    // Register the prefix rule via the production approve() path: park a
    // pending exec call, then approve it with the AlwaysPrefix scope.
    let _rx = approval_manager.request_approval(
        "seed",
        &wcore_protocol::events::ToolCategory::Exec,
        "exec_tool",
    );
    approval_manager.approve(
        "seed",
        wcore_protocol::commands::ApprovalScope::AlwaysPrefix {
            prefix: "cargo ".to_string(),
        },
        None,
    );
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // No background approver. If the prefix rule is honored, the cargo
    // command auto-approves and the run completes; otherwise it parks.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.run("Use the tool", "msg-pfx-1"),
    )
    .await
    .expect("run must not park on approval — prefix rule should auto-approve")
    .expect("should succeed");
    assert_eq!(result.text, "Prefix auto");
    assert_eq!(result.turns, 2);
}

// ---------------------------------------------------------------------------
// test: W0 prefix rule does NOT cover a non-matching command
//
// With the same AlwaysPrefix{"cargo "} rule, an `rm -rf` command must still
// require approval. We prove it went through the gate by resolving the pending
// call as Denied and observing the denial fed back to the LLM.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_prefix_scoped_rule_still_prompts_for_non_matching_command() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-pfx-2".to_string(),
            name: "exec_tool".to_string(),
            input: json!({ "command": "rm -rf /tmp/x" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Refused rm".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ExecMockTool::new("exec_tool", "tool output")));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let _rx = approval_manager.request_approval(
        "seed-2",
        &wcore_protocol::events::ToolCategory::Exec,
        "exec_tool",
    );
    approval_manager.approve(
        "seed-2",
        wcore_protocol::commands::ApprovalScope::AlwaysPrefix {
            prefix: "cargo ".to_string(),
        },
        None,
    );
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // The rm command is NOT covered by the cargo prefix, so it parks for
    // approval. Resolve it as Denied to prove it reached the approval gate.
    let am = approval_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        am.resolve(
            "call-pfx-2",
            ToolApprovalResult::Denied {
                reason: "rm not allowed".into(),
            },
        );
    });

    let result = engine
        .run("Use the tool", "msg-pfx-2")
        .await
        .expect("should succeed");
    assert_eq!(result.text, "Refused rm");
    assert_eq!(result.turns, 2);
}

// ---------------------------------------------------------------------------
// test: client disconnect (channel drop) causes UserAborted
//
// If the approval channel sender is dropped before resolve, the engine
// should return an abort error.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_client_disconnect_aborts() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-5".to_string(),
            name: "exec_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ExecMockTool::new("exec_tool", "tool output")));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // Simulate client disconnect: drop the pending sender without resolving
    let am = approval_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        am.drop_pending("call-5");
    });

    let err = engine.run("Use the tool", "msg-5").await.unwrap_err();
    assert!(
        format!("{:?}", err).contains("UserAborted"),
        "expected UserAborted, got: {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// v0.9.3 W0.3 — Approved { answer: Some(s) } synthesizes ToolResult from the
// answer string and SHORT-CIRCUITS dispatch.
//
// Setup: register an Exec-category tool whose execute() panics. If the
// orchestration synthesis path is correct, execute() must never run; the
// engine completes turn 2 with "Done" using the synthesized result content.
// If the short-circuit is missing, execute() panics and the test fails loudly.
// ---------------------------------------------------------------------------

use async_trait::async_trait;
use serde_json::Value as SerdeValue;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_types::tool::ToolResult;

struct PanicOnExecuteTool {
    tool_name: String,
}

impl PanicOnExecuteTool {
    fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for PanicOnExecuteTool {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> &str {
        "Panics if invoked — proves W0.3 synthesis short-circuited dispatch"
    }
    fn input_schema(&self) -> SerdeValue {
        json!({"type": "object"})
    }
    fn category(&self) -> ToolCategory {
        // Exec category triggers the approval gate (matches AskUserQuestion
        // routing path used in production). Info also gates, but Exec keeps
        // the test surface identical to the existing approve-flow test above.
        ToolCategory::Exec
    }
    fn is_concurrency_safe(&self, _input: &SerdeValue) -> bool {
        false
    }
    async fn execute(&self, _input: SerdeValue) -> ToolResult {
        panic!(
            "PanicOnExecuteTool::execute called — W0.3 synthesis path failed \
             to short-circuit dispatch when Approved.answer was Some(_)"
        );
    }
}

#[tokio::test]
async fn approved_with_answer_synthesizes_tool_result() {
    // v0.9.3 W8 H1-reliability: synth is GUARDED on `name == "AskUserQuestion"`.
    // Register PanicOnExecuteTool under that name so the LLM emits a
    // ToolUse for AskUserQuestion → synth fires → execute() never runs
    // (would panic). If the guard regresses to ANY tool, this test still
    // passes; the negative guard at `non_askuser_answer_falls_through_to_execute`
    // proves the guard is in place.
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-ask-1".to_string(),
            name: "AskUserQuestion".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(PanicOnExecuteTool::new("AskUserQuestion")));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // Background: resolve the pending approval with an `answer` payload.
    let am = approval_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        am.resolve(
            "call-ask-1",
            ToolApprovalResult::Approved {
                answer: Some("Choice C".to_string()),
            },
        );
    });

    // If the W0.3 short-circuit is wired AND H1-reliability guard is on
    // (name == "AskUserQuestion"), execute() never runs, the synthesized
    // tool result feeds into turn 2, and the engine completes with "Done".
    let result = engine
        .run("Pick an option", "msg-ask-1")
        .await
        .expect("synthesis path must succeed without invoking execute()");
    assert_eq!(result.text, "Done");
    assert_eq!(result.turns, 2);
}

// ---------------------------------------------------------------------------
// v0.9.3 W8 H1-reliability — synth is GUARDED on `name == "AskUserQuestion"`.
// A compromised/buggy host sending `Approved { answer: Some(s) }` for a
// non-AskUser tool (Bash/Edit/Write/etc.) must NOT have `s` fabricated as
// "tool output"; the real tool's execute() must run instead so the LLM sees
// the real result, not host-supplied text. This test seeds a non-AskUser
// tool whose execute() RETURNS a marker output (not panic): the engine path
// proves we fell through to execute() by surfacing the marker.
// ---------------------------------------------------------------------------

struct MarkerOnExecuteTool {
    tool_name: String,
    flag: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl Tool for MarkerOnExecuteTool {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> &str {
        "Sets a flag in execute() — proves we reached execute() not synth"
    }
    fn input_schema(&self) -> SerdeValue {
        json!({"type": "object"})
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }
    fn is_concurrency_safe(&self, _input: &SerdeValue) -> bool {
        false
    }
    async fn execute(&self, _input: SerdeValue) -> ToolResult {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        ToolResult {
            content: "REAL_EXECUTE_RAN".to_string(),
            is_error: false,
        }
    }
}

#[tokio::test]
async fn non_askuser_answer_falls_through_to_execute() {
    // Bash-class non-AskUser tool. Host sends `answer: Some("fabricated")`.
    // The W8 H1-reliability guard at orchestration/mod.rs:911 (now scoped
    // to `name == "AskUserQuestion"`) must reject the synth path and fall
    // through to dispatch — execute() returns "REAL_EXECUTE_RAN". If the
    // guard regresses, the LLM would see "fabricated" instead.
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-bash-1".to_string(),
            name: "FakeBash".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    // Turn 2 echoes the tool result text so the test can assert which
    // path produced the content. The mock LLM is dumb so we use a single
    // text delta and let the assertion below check that the engine's
    // last turn-text reflects the real execute path having run.
    let turn2 = vec![
        LlmEvent::TextDelta("done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MarkerOnExecuteTool {
        tool_name: "FakeBash".to_string(),
        flag: executed.clone(),
    }));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // Host sends a fabricated answer for a non-AskUser tool — the guard
    // must reject this and fall through to execute().
    let am = approval_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        am.resolve(
            "call-bash-1",
            ToolApprovalResult::Approved {
                answer: Some("fabricated_by_host".to_string()),
            },
        );
    });

    // The engine completes turn 2. The MarkerOnExecuteTool flips `executed`
    // when its execute() runs. If the synth path wrongly fired (guard
    // regression), execute() never runs and the flag stays false.
    let result = engine
        .run("Run the tool", "msg-bash-1")
        .await
        .expect("fall-through path must invoke execute() and succeed");
    assert_eq!(result.turns, 2);
    assert!(
        executed.load(std::sync::atomic::Ordering::SeqCst),
        "non-AskUser tool's execute() must have run — W8 H1-reliability \
         guard at orchestration/mod.rs:911 must reject the synth path when \
         tool name is NOT 'AskUserQuestion'"
    );
}

// ---------------------------------------------------------------------------
// v0.9.3 W8 H2-integration — AskUserQuestion must ALWAYS need approval, even
// in SessionMode::AutoEdit (where Info-category tools are auto-approved).
// Without the carve-out, AutoEdit skips the approval gate for AskUserQuestion,
// then AskUserQuestionTool::execute() returns its W0.4 loud-defensive
// is_error: true fallback, and the LLM sees an error for a question it asked.
//
// Approach: set mode to AutoEdit on the approval manager, register an
// AskUserQuestion-named tool, run the engine with NO background approver,
// and assert the run TIMES OUT (proves it parked on approval). Before the
// fix, AutoEdit would skip approval and complete immediately. After the fix,
// the tool gates on approval which never arrives → timeout.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn askuser_always_needs_approval_even_in_autoedit_mode_v093_w8_h2() {
    use wcore_protocol::commands::SessionMode;

    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-ask-h2".to_string(),
            name: "AskUserQuestion".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("never reached".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    // MarkerOnExecuteTool flips `executed` if execute() runs (regression
    // signal: in AutoEdit without H2, AskUser execute would fire).
    let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MarkerOnExecuteTool {
        tool_name: "AskUserQuestion".to_string(),
        flag: executed.clone(),
    }));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    // Crank to AutoEdit: Info category auto-approves. Without H2, AskUser
    // would skip the gate and dispatch.
    approval_manager.set_mode(SessionMode::AutoEdit);
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // No background approver. With the H2 carve-out, the engine PARKS on
    // approval await and times out. Without it, the engine would skip the
    // gate, hit execute() (sets the flag), and complete cleanly.
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        engine.run("Pick one", "msg-h2"),
    )
    .await;

    assert!(
        result.is_err(),
        "AutoEdit + AskUser must PARK on approval (H2 carve-out), not auto-approve \
         and dispatch; got: {:?}",
        result
    );
    assert!(
        !executed.load(std::sync::atomic::Ordering::SeqCst),
        "AskUser execute() must NOT run in AutoEdit mode — H2 carve-out must \
         force approval; flag flipped means AutoEdit auto-approved the tool"
    );
}

// ---------------------------------------------------------------------------
// v0.9.4 W1.5 — scope-drop regression test.
//
// Approve the FIRST exec_tool call with `scope=Always`. The SECOND identical
// call must auto-approve (no background approver needed) because the `Always`
// scope registered the category-level rule. Before the W1.3 fix in main.rs,
// the streaming arm called `resolve()` which silently dropped the scope and
// the second call would park waiting for approval that never arrives.
//
// This test lives at the engine level (not main.rs): it uses
// `approval_manager.approve()` directly (the same call that main.rs now makes)
// to prove the approval_manager correctly persists the `Always` rule.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn scope_always_auto_approves_second_matching_call_v094_w1() {
    use wcore_protocol::commands::ApprovalScope;

    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "call-scope-1".to_string(),
            name: "exec_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    // Turn 2: another exec_tool call — must auto-approve due to Always scope.
    let turn2 = vec![
        LlmEvent::ToolUse {
            id: "call-scope-2".to_string(),
            name: "exec_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: token_usage(80, 30),
        },
    ];
    let turn3 = vec![
        LlmEvent::TextDelta("both approved".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: token_usage(100, 50),
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2, turn3]));
    let mut config = test_config();
    config.tools.auto_approve = false;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ExecMockTool::new("exec_tool", "tool output")));

    let output = silent_output();
    let approval_manager = Arc::new(ToolApprovalManager::new());
    let writer = Arc::new(ProtocolWriter::new());

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.set_approval_manager(approval_manager.clone());
    engine.set_protocol_writer(writer);

    // Approve the first call with scope=Always — this is what main.rs now does
    // via approval_manager.approve() instead of the old resolve() stub.
    let am = approval_manager.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        // Seed the pending entry via request_approval so approve() can find it.
        // (In production the engine calls request_approval internally.)
        // We approve "call-scope-1" with Always scope so the exec category
        // is registered for auto-approval. The second call (call-scope-2) must
        // then auto-approve without any background task.
        am.approve("call-scope-1", ApprovalScope::Always, None);
    });

    // Run with a timeout — if the second call parks on approval (scope-drop
    // regression), the timeout fires and the test fails.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.run("use the tool twice", "msg-scope-1"),
    )
    .await
    .expect("engine must not park on second approval — Always scope must persist")
    .expect("engine must succeed");

    assert_eq!(result.turns, 3, "should complete 3 turns");
    assert_eq!(result.text, "both approved");
}
