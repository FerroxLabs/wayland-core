//! A12-D1 — microcompact must be pressure RELIEF, not hygiene.
//!
//! `should_microcompact` had no context-size term at all: its count trigger
//! fired whenever the number of LIVE compactable tool results exceeded
//! `micro_keep_recent * 2`, and `microcompact` then cleared everything but the
//! last `micro_keep_recent` — resetting the live count to exactly that number.
//! The next fan-out re-armed it. `run_compaction` is awaited before EVERY LLM
//! round trip, so on a codebase-comprehension task the product deleted the
//! files it had just read at 34% occupancy, forced a file-cache generation
//! bump, and re-read them forever.
//!
//! These tests drive the real `AgentEngine::run()` loop and assert against the
//! messages the PROVIDER actually received — never engine internals and never
//! log text.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_config::compact::CompactConfig;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, StopReason, TokenUsage};
use wcore_types::tool::ToolResult;

use common::test_config;

/// Appears nowhere else in the repo, so its presence in a captured request can
/// only come from a tool result that was never cleared.
const CANARY: &str = "CANARY-MICRO-7f2b9e";

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// Every call returns a UNIQUE body, so "canary N survived" is a statement
/// about one specific tool result rather than about any tool result.
struct CanaryTool {
    calls: Mutex<usize>,
}

#[async_trait]
impl Tool for CanaryTool {
    fn name(&self) -> &str {
        "canary_tool"
    }
    fn description(&self) -> &str {
        "returns a unique canary body per call"
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn execute(&self, _input: serde_json::Value) -> ToolResult {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            let n = *calls;
            *calls += 1;
            n
        };
        ToolResult {
            content: format!("{CANARY}-{n} ::{}", "p".repeat(400)),
            is_error: false,
        }
    }
}

/// How the mock reports `input_tokens`.
#[derive(Clone, Copy)]
enum Watermark {
    /// The same number every turn.
    Fixed(u64),
    /// Closed loop: pressure is a function of how much LIVE tool output is
    /// actually in the window, so clearing results genuinely relieves it. This
    /// is what makes the sawtooth reproducible in a test.
    PerLiveResult { base: u64, per_result: u64 },
}

struct PressureProvider {
    tool_turns: usize,
    watermark: Watermark,
    captured: Arc<Mutex<Vec<Vec<Message>>>>,
    calls: Mutex<usize>,
}

fn live_compactable(messages: &[Message]) -> usize {
    messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { content, .. }
                    if content.starts_with(CANARY)
            )
        })
        .count()
}

fn cleared_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { content, .. }
                    if content == wcore_agent::compact::micro::CLEARED_TOOL_RESULT
            )
        })
        .count()
}

#[async_trait]
impl LlmProvider for PressureProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.captured.lock().unwrap().push(request.messages.clone());
        let turn = {
            let mut calls = self.calls.lock().unwrap();
            let turn = *calls;
            *calls += 1;
            turn
        };
        let input_tokens = match self.watermark {
            Watermark::Fixed(value) => value,
            Watermark::PerLiveResult { base, per_result } => {
                base + per_result * live_compactable(&request.messages) as u64
            }
        };
        let events = if turn < self.tool_turns {
            vec![
                LlmEvent::ToolUse {
                    id: format!("t{turn}"),
                    name: "canary_tool".to_string(),
                    input: serde_json::json!({}),
                    extra: None,
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                        StopReason::ToolUse,
                    ),
                    usage: TokenUsage {
                        input_tokens,
                        output_tokens: 100,
                        ..Default::default()
                    },
                },
            ]
        } else {
            vec![
                LlmEvent::TextDelta("finished".to_string()),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                        StopReason::EndTurn,
                    ),
                    usage: TokenUsage {
                        input_tokens,
                        output_tokens: 100,
                        ..Default::default()
                    },
                },
            ]
        };
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

/// Run the loop and hand back every request the provider saw.
async fn run_session(tool_turns: usize, watermark: Watermark) -> Vec<Vec<Message>> {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(PressureProvider {
        tool_turns,
        watermark,
        captured: Arc::clone(&captured),
        calls: Mutex::new(0),
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        // 5 is the SHIPPED default, not a number tuned to make this pass.
        micro_keep_recent: 5,
        compactable_tools: vec!["canary_tool".into()],
        context_window: Some(200_000),
        ..Default::default()
    };
    config.max_turns = Some(tool_turns + 5);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CanaryTool {
        calls: Mutex::new(0),
    }));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output());
    let result = engine.run("Trace the flow", "msg-1").await.expect("run");
    assert_eq!(
        result.text, "finished",
        "the loop must reach its final turn, or the assertions below are vacuous"
    );

    let captured = captured.lock().unwrap().clone();
    assert_eq!(
        captured.len(),
        tool_turns + 1,
        "a run that exited the loop early never accumulated the tool results \
         these tests are about"
    );
    captured
}

// ── Layer 1 — the defect: clearing at low occupancy ─────────────────────────

#[tokio::test]
async fn micro_does_not_clear_tool_results_under_low_pressure() {
    // 200k window → autocompact threshold 167k, pressure floor 100k.
    // The model reports 20_000 the whole way: 10% full, an order of magnitude
    // below any boundary. 14 live results > 5*2, so the COUNT trigger is true
    // and the only thing that can stop the clear is a pressure condition.
    let captured = run_session(14, Watermark::Fixed(20_000)).await;
    let last = captured.last().expect("captured");

    assert_eq!(
        cleared_count(last),
        0,
        "microcompact deleted tool output at 10% occupancy"
    );
    for n in 0..14 {
        let needle = format!("{CANARY}-{n} ");
        assert!(
            last.iter().any(|m| m.content.iter().any(|b| matches!(
                b,
                ContentBlock::ToolResult { content, .. } if content.starts_with(&needle)
            ))),
            "canary {n} was cleared out of the model's own context"
        );
    }
}

// ── Layer 2 — the positive control: it must still relieve real pressure ─────

#[tokio::test]
async fn micro_still_fires_when_the_window_is_actually_full() {
    // ONE number changed from Layer 1: 150_000 reported input. Above the
    // 100k floor, below the 167k autocompact threshold, so this isolates
    // microcompact. Every "just turn it off" fix passes Layer 1 and fails
    // here; every "leave it alone" non-fix fails Layer 1 and passes here.
    let captured = run_session(14, Watermark::Fixed(150_000)).await;
    let last = captured.last().expect("captured");

    assert!(
        cleared_count(last) > 0,
        "under genuine pressure microcompact must still clear old tool output"
    );
    // The keep-policy is unchanged: the most recent results survive verbatim.
    for n in 9..14 {
        let needle = format!("{CANARY}-{n} ");
        assert!(
            last.iter().any(|m| m.content.iter().any(|b| matches!(
                b,
                ContentBlock::ToolResult { content, .. } if content.starts_with(&needle)
            ))),
            "canary {n} is inside the keep window and must survive"
        );
    }
}

// ── Layer 3 — the sawtooth itself, with a closed-loop watermark ─────────────

#[tokio::test]
async fn a_long_session_above_the_floor_does_not_sawtooth() {
    // Pressure is a real function of live tool output, so a clear genuinely
    // lowers the watermark and the count trigger re-arms on the next fan-out.
    // That feedback loop is the defect: without a re-arm condition micro fires
    // once every ~6 turns forever, deleting reads the model is still using.
    //
    // 20_000 + 8_000 per live result: 11 live results = 108k, over the 100k
    // floor. A clear back to 5 live drops it to 60k.
    let captured = run_session(
        40,
        Watermark::PerLiveResult {
            base: 20_000,
            per_result: 8_000,
        },
    )
    .await;

    // Count the requests in which the cleared total went UP — one per fire.
    let mut fires = 0usize;
    let mut previous = 0usize;
    for messages in &captured {
        let now = cleared_count(messages);
        if now > previous {
            fires += 1;
        }
        previous = now;
    }

    assert!(
        fires >= 1,
        "the gate must still open above the floor, or this proves nothing"
    );
    assert!(
        fires <= 3,
        "microcompact fired {fires} times in 40 turns — the count trigger is \
         still re-arming off its own post-clear count"
    );
}
