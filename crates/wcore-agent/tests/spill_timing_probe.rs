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
async fn timing_probe() {
    let t0 = std::time::Instant::now();
    let payload: usize = std::env::var("SPILL_PROBE_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(480_000);
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
    config.compact.context_window = Some(60_000);
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000;

    let mut registry = ToolRegistry::new();
    let huge = "x".repeat(payload);
    let t_setup = t0.elapsed();
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

    let t_run = t0.elapsed();
    let path = spilled_path(&engine);
    let jail = SandboxedFs::new(RealFs, workspace.path());
    let bytes = jail.read(&path).await.unwrap_or_else(|err| {
        panic!(
            "the engine told the model to read {} and this session's own file tools refused: {err}",
            path.display()
        )
    });
    let t_end = t0.elapsed();
    eprintln!("PHASE bytes={payload} setup_s={:.3} run_s={:.3} readback_s={:.3} total_s={:.3}", t_setup.as_secs_f64(), (t_run-t_setup).as_secs_f64(), (t_end-t_run).as_secs_f64(), t_end.as_secs_f64());
    assert!(!bytes.is_empty());
}
