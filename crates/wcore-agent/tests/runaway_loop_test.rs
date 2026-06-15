//! Loop-convergence E2E for the engine-side runaway breaker.
//!
//! A model that repeats the SAME tool call every turn, against a tool that
//! returns the SAME failing result, must be stopped by the breaker WELL BEFORE
//! `max_turns` — proving a no-progress loop converges instead of burning tokens
//! to the turn cap (the "8.5M tokens in 2 hours" class of report).

mod common;

use std::sync::Arc;

use serde_json::json;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::TestSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

/// One turn that asks for the same tool with the same args. The id varies per
/// turn (so per-turn history is well-formed); the breaker keys on
/// name+args+result, not id, so every turn shares one signature.
fn loop_turn(i: usize) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: format!("call-{i}"),
            name: "loop_tool".to_string(),
            input: json!({ "q": "same" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage {
                input_tokens: 50,
                output_tokens: 10,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

#[tokio::test]
async fn repeated_identical_tool_call_converges_via_breaker() {
    // 30 identical tool-call turns queued; max_turns raised to 30 so the breaker
    // (default threshold 10) is what stops the loop, not the turn cap.
    let turns: Vec<Vec<LlmEvent>> = (0..30).map(loop_turn).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new(
        "loop_tool",
        "network is unreachable in this sandbox",
        true, // identical failing outcome every call
    )));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(30);

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("install the deps", "")
        .await
        .expect("run completes (terminated cleanly, not Err)");

    // The breaker (default threshold 10) must stop the loop well before
    // max_turns(30) — proving it converged, not just hit the turn cap.
    assert!(
        result.turns < 30,
        "runaway breaker must converge the loop before max_turns; turns = {}",
        result.turns
    );

    // …and surface a clear, user-visible no-progress-loop error.
    let events = handle.snapshot();
    let saw_loop_error = events
        .iter()
        .any(|e| e["type"].as_str() == Some("error") && e.to_string().contains("no-progress loop"));
    assert!(
        saw_loop_error,
        "expected a visible no-progress-loop error event; got {events:?}"
    );
}

/// Control: a tool whose result CHANGES every turn (real progress) must NOT
/// trip the breaker — it runs to the natural max_turns cap instead. Guards
/// against the breaker firing on a legitimate iterate-retest cadence.
#[tokio::test]
async fn changing_results_do_not_trip_the_breaker() {
    // Each turn the model calls the same tool, but the tool's output differs,
    // so the signature changes and the streak never accumulates.
    let turns: Vec<Vec<LlmEvent>> = (0..12).map(loop_turn).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ChangingTool::default()));

    let config = test_config(); // max_turns = Some(10)
    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("iterate", "").await.expect("run completes");

    // Reached the turn cap, NOT the breaker (12 turns queued, cap 10).
    assert_eq!(
        result.turns, 10,
        "changing results must run to max_turns, not be cut by the breaker"
    );
    let events = handle.snapshot();
    assert!(
        !events
            .iter()
            .any(|e| e["type"].as_str() == Some("error") && e.to_string().contains("no-progress")),
        "the breaker must not fire when each result differs"
    );
}

/// A tool that returns a different result on each call.
#[derive(Default)]
struct ChangingTool {
    calls: std::sync::Mutex<u32>,
}

#[async_trait::async_trait]
impl wcore_tools::Tool for ChangingTool {
    fn name(&self) -> &str {
        "loop_tool"
    }
    fn description(&self) -> &str {
        "Returns a different result each call"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn execute(&self, _input: serde_json::Value) -> wcore_types::tool::ToolResult {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        wcore_types::tool::ToolResult {
            content: format!("progress step {n}"),
            is_error: false,
        }
    }
}
