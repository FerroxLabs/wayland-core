//! #174 / #559 — the spend meter must update WHILE spend accumulates.
//!
//! `SessionCost` is the only cost surface the host has: the TUI status bar
//! reads `app.cost.total_cost_usd` from it and the `/cost` screen renders its
//! `per_turn` rows. A single long user turn can drive dozens of agentic
//! round-trips (issue #559 measured one leader turn at 4.88M input tokens),
//! so a meter that only publishes when the run ENDS shows a stale figure for
//! the entire window in which a runaway is actually burning money.
//!
//! These tests measure the number of cost publications and the row count
//! carried by each one, across a 4-round-trip agentic run.

mod common;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

/// Records every `emit_session_cost` payload in emission order.
struct CostSink {
    inner: Arc<TerminalSink>,
    costs: Arc<Mutex<Vec<Value>>>,
}

impl CostSink {
    fn new() -> (Self, Arc<Mutex<Vec<Value>>>) {
        let costs = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                inner: Arc::new(TerminalSink::new(true)),
                costs: costs.clone(),
            },
            costs,
        )
    }
}

impl OutputSink for CostSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.inner.emit_text_delta(text, msg_id);
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.inner.emit_thinking(text, msg_id);
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        self.inner.emit_tool_call(name, input);
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        self.inner.emit_tool_result(name, is_error, content);
    }
    fn emit_stream_start(&self, msg_id: &str) {
        self.inner.emit_stream_start(msg_id);
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
        finish: FinishReason,
    ) {
        self.inner.emit_stream_end(
            msg_id,
            turns,
            input,
            output,
            cache_creation,
            cache_read,
            finish,
        );
    }
    fn emit_error(&self, msg: &str, retryable: bool) {
        self.inner.emit_error(msg, retryable);
    }
    fn emit_info(&self, msg: &str) {
        self.inner.emit_info(msg);
    }
    fn emit_session_cost(&self, _session_id: &str, cost_payload: &Value) {
        if let Ok(mut g) = self.costs.lock() {
            g.push(cost_payload.clone());
        }
    }
}

fn tool_turn(id: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: id.into(),
            name: "mock_tool".into(),
            input: json!({ "path": "/etc/hosts" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage {
                input_tokens: 50_000,
                output_tokens: 100,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

fn final_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("done".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 50_000,
                output_tokens: 10,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

/// Drive one user turn through 3 tool round-trips + a final text turn and
/// return every `SessionCost` payload published during the run.
async fn run_four_round_trips() -> Vec<Value> {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        tool_turn("tu_1"),
        tool_turn("tu_2"),
        tool_turn("tu_3"),
        final_turn(),
    ]));
    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "ok", false)));

    let (sink, costs) = CostSink::new();
    let output: Arc<dyn OutputSink> = Arc::new(sink);
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("burn some tokens", "m-1")
        .await
        .expect("engine ok");
    assert_eq!(result.turns, 4, "harness precondition: 4 round trips");

    costs.lock().unwrap().clone()
}

/// The meter must publish once per completed round trip, not once per run.
#[tokio::test]
async fn spend_meter_publishes_on_every_round_trip() {
    let captured = run_four_round_trips().await;
    assert!(
        captured.len() >= 4,
        "4 round trips burned 200k input tokens but the host received only \
         {} cost publication(s): the spend meter is frozen for the whole \
         window in which a runaway burns. payloads={captured:?}",
        captured.len()
    );
}

/// The published row count must GROW: a host watching the meter has to be able
/// to see spend accumulating, not just its final total.
#[tokio::test]
async fn spend_meter_row_count_grows_during_the_run() {
    let captured = run_four_round_trips().await;
    let row_counts: Vec<usize> = captured
        .iter()
        .map(|p| {
            p.get("per_turn")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0)
        })
        .collect();
    assert!(
        row_counts.first().copied().unwrap_or(0) == 1,
        "the first publication must carry exactly the first turn's row \
         (saw {row_counts:?})"
    );
    assert!(
        row_counts.windows(2).all(|w| w[1] >= w[0]),
        "per-turn row count must be monotonically non-decreasing, saw {row_counts:?}"
    );
    assert!(
        row_counts.last().copied().unwrap_or(0) == 4,
        "the final publication must carry all 4 turns (saw {row_counts:?})"
    );
}
