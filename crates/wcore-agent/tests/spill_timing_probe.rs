//! Measurement instrument for FerroxLabs/wayland-core#378 c1.
//!
//! #378 c1 asks that the >30s runtime of
//! `spill_readback_engine_wiring::the_engine_spills_where_this_session_can_read_it_back`
//! be ATTRIBUTED by measurement — product latency on the spill/read-back path,
//! or the fixture's own cost — rather than inferred.
//!
//! This is a copy of that test's setup with three levers the graded test does
//! not have:
//!
//!   * `SPILL_PROBE_BYTES`   — the tool-result payload size, so the runtime can
//!                             be measured against payload and the relationship
//!                             read off rather than guessed at.
//!   * `SPILL_PROBE_CEILING` — the context ceiling. Raising it above the payload
//!                             turns the #636 shed OFF while leaving every other
//!                             byte of the fixture identical, which is the
//!                             control that separates "the spill path is slow"
//!                             from "handling a payload this size is slow".
//!   * phase timing          — fixture construction, `engine.run()`, and the
//!                             read-back through the jail are timed separately.
//!
//! It is `#[ignore]`d, so it costs the suite nothing and is run deliberately
//! with `--run-ignored all`.

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

/// The path the shed put in front of the model, or `None` when the shed did not
/// fire. The graded test panics here; the probe must be able to time a run in
/// which no spill happened, because that run is the control.
fn spilled_path(engine: &AgentEngine) -> Option<std::path::PathBuf> {
    for message in engine.conversation_messages() {
        for block in &message.content {
            if let ContentBlock::ToolResult { content, .. } = block
                && let Some(line) = content
                    .lines()
                    .find_map(|l| l.strip_prefix("Full output saved to: "))
            {
                return Some(std::path::PathBuf::from(line.trim()));
            }
        }
    }
    None
}

#[tokio::test]
#[ignore = "measurement instrument for wayland-core#378; run with --run-ignored all"]
async fn timing_probe() {
    let t0 = std::time::Instant::now();
    let payload: usize = std::env::var("SPILL_PROBE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(480_000);
    let ceiling: usize = std::env::var("SPILL_PROBE_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000);
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
    config.compact.context_window = Some(ceiling);
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
    let shed = path.is_some();
    let mut read_bytes = 0usize;
    if let Some(path) = path {
        let jail = SandboxedFs::new(RealFs, workspace.path());
        let bytes = jail.read(&path).await.unwrap_or_else(|err| {
            panic!(
                "the engine told the model to read {} and this session's own file tools refused: {err}",
                path.display()
            )
        });
        read_bytes = bytes.len();
    }
    let t_end = t0.elapsed();
    eprintln!(
        "PHASE bytes={payload} ceiling={ceiling} shed={shed} read={read_bytes} setup_s={:.3} run_s={:.3} readback_s={:.3} total_s={:.3}",
        t_setup.as_secs_f64(),
        (t_run - t_setup).as_secs_f64(),
        (t_end - t_run).as_secs_f64(),
        t_end.as_secs_f64()
    );
}
