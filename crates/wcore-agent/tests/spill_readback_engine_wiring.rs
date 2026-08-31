//! FerroxLabs/wayland#1097 — grades the ENGINE CALL SITE, not the registry.
//!
//! `spill_readback_containment.rs` grades `ToolRegistry::spill_storage()` by
//! calling it directly. That leaves the one production consumer — the #636
//! shed in `AgentEngine::run_turn` — ungraded: swapping it back to
//! `StorageDir::os_default()` keeps every existing test green.
//!
//! This drives a real `AgentEngine` turn over the context ceiling and reads
//! the spilled file back through the same jail the session's `Read` uses.

mod common;

use std::sync::Arc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::vfs::{RealFs, SandboxedFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{ContentBlock, StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// The path the shed put in front of the model, pulled out of the persisted
/// history exactly as the model would read it.
fn spilled_path(engine: &AgentEngine) -> std::path::PathBuf {
    for message in engine.conversation_messages() {
        for block in &message.content {
            if let ContentBlock::ToolResult { content, .. } = block
                && let Some(line) = content
                    .lines()
                    .find_map(|l| l.strip_prefix("Full output saved to: "))
            {
                return std::path::PathBuf::from(line.trim());
            }
        }
    }
    panic!("no spilled tool result in the persisted history — the shed never ran");
}

#[tokio::test]
async fn the_engine_spills_where_this_session_can_read_it_back() {
    let workspace = tempfile::tempdir().expect("workspace");
    let policy = Arc::new(WorkspacePolicy::contained(workspace.path()));

    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "big".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 5_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Continued after shedding".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: TokenUsage {
                input_tokens: 5_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));

    // FerroxLabs/wayland#1235 ask 3 — the payload is DERIVED from the two
    // thresholds it has to sit between, not written down beside them.
    //
    // It was `"x".repeat(480_000)` next to a hand-written window, and halving
    // it to 120,000 made the test PASS WITHOUT SPILLING: the subject of the
    // assertion quietly disappeared while every assertion still held, because
    // `spilled_path` panics only when nothing spilled and 120,000 is under the
    // shed's trigger. A test whose subject vanishes when one constant moves is
    // one refactor away from grading nothing.
    //
    // The shed (#636) fires when the estimated request exceeds
    // `input_ceiling_for_window(window)`; `truncate_result` caps the result at
    // `max_result_size` BEFORE the estimate is taken, so the payload must clear
    // the ceiling and stay under the cap. Both bounds are asserted below rather
    // than trusted, so a change to the reserves, to `MAX_RESERVE_FRACTION`, or
    // to the estimator's chars-per-token reds HERE with the arithmetic printed
    // instead of silently un-spilling.
    const CHARS_PER_TOKEN: usize = 4;
    const WINDOW: usize = 60_000;
    const MAX_RESULT_SIZE: usize = 600_000;

    let mut config = test_config();
    config.compact.enabled = false;
    config.compact.context_window = Some(WINDOW);
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000;

    let ceiling_chars = config.compact.input_ceiling_for_window(WINDOW) * CHARS_PER_TOKEN;
    // Twice the ceiling: comfortably over the trigger without sitting on it, so
    // a small change in the estimator's overhead cannot flip the subject off.
    let payload_chars = ceiling_chars * 2;
    assert!(
        payload_chars > ceiling_chars,
        "the payload ({payload_chars} chars) must exceed the {ceiling_chars}-char shed trigger, \
         or this test passes without ever spilling"
    );
    assert!(
        payload_chars < MAX_RESULT_SIZE,
        "the payload ({payload_chars} chars) must stay under the {MAX_RESULT_SIZE}-char \
         max_result_size, or truncate_result cuts it back below the trigger before the shed \
         is ever consulted"
    );

    let mut registry = ToolRegistry::new();
    let huge = "x".repeat(payload_chars);
    registry.register(Box::new(
        MockTool::new("mock_tool", &huge, false).with_max_result_size(MAX_RESULT_SIZE),
    ));
    // Exactly what bootstrap installs for a Workspace posture.
    registry.set_tool_vfs(Arc::new(SandboxedFs::new(RealFs, workspace.path())));
    registry.set_workspace_policy(Arc::clone(&policy));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output());
    engine
        .run("summarize the file", "msg-1")
        .await
        .expect("run");

    let path = spilled_path(&engine);
    let jail = SandboxedFs::new(RealFs, workspace.path());
    let bytes = jail.read(&path).await.unwrap_or_else(|err| {
        panic!(
            "the engine told the model to read {} and this session's own file tools refused: {err}",
            path.display()
        )
    });
    assert!(!bytes.is_empty());
}
