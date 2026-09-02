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

    let mut config = test_config();
    config.compact.enabled = false;
    config.compact.context_window = Some(25_000);
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000;

    let mut registry = ToolRegistry::new();
    // wayland-core#378: 60_000, not 480_000. The graded property is that the
    // shed FIRES and that this session's own jail can read the spilled file
    // back -- neither depends on the payload being half a megabyte. The
    // over-ceiling RATIO is what makes the shed fire, and it is preserved
    // exactly: the old fixture was 480_000 chars (120_000 tok) against a
    // 60_000 - 10_000 - 10_000 = 40_000 tok effective ceiling, i.e. 3x over;
    // this one is 60_000 chars (15_000 tok) against 25_000 - 10_000 - 10_000
    // = 5_000 tok, also 3x over.
    //
    // MEASURED on hetzner-dsm, this binary alone, phase-timed: the cost is
    // ~linear in the payload and sits almost entirely inside `engine.run()`
    // (fixture construction 0.001-0.002 s, read-back through the jail
    // 0.000-0.001 s, i.e. under 0.005% of the total between them). At
    // 480_000 the turn cost 48.5 / 48.6 / 52.7 s run-alone and was killed at
    // 60 s under host load; at 60_000 it is 5.9 s. The shed is NOT the cost:
    // with the ceiling raised so the shed never fires, the SAME 480_000
    // payload still cost 48.0 s, indistinguishable from the shed-on arm.
    // Shrinking the payload is therefore the only lever that moves the
    // runtime without changing what is graded.
    let huge = "x".repeat(60_000);
    registry.register(Box::new(
        MockTool::new("mock_tool", &huge, false).with_max_result_size(600_000),
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
