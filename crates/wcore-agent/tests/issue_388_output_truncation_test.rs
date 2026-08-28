//! #388(b) — a response the provider cut at its output cap must never have its
//! tool calls executed, even when every individual call happens to have closed.
//!
//! The OUTPUT-CAP TRUNCATION GATE (T3) in `engine.rs` was armed only by
//! `LlmEvent::TruncatedToolCall`, i.e. only when the cut landed INSIDE a call's
//! argument JSON. A `finish_reason=length` response whose tool calls all closed
//! before the cut therefore committed and ran: the model was mid-plan, the rest
//! of what it intended to do was discarded, and the engine treated the prefix
//! as a finished turn. That is the same silent failure T3 exists to stop, one
//! byte to the left.
//!
//! RED-arm evidence (pre-fix, `cargo test -p wcore-agent --test
//! issue_388_output_truncation_test`): the tool executes and the run walks to
//! the turn cap.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::TestSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wcore_types::tool::ToolResult;

/// Every turn: one COMPLETE tool call, then `finish_reason=length`. No
/// `TruncatedToolCall` event — the provider saw the call close cleanly and the
/// cut land after it.
struct LengthCutProvider {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for LengthCutProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            let _ = tx
                .send(LlmEvent::ToolUse {
                    id: format!("call-{n}"),
                    name: "probe_tool".to_string(),
                    input: json!({ "step": n }),
                    extra: None,
                })
                .await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::MaxTokens,
                    finish_reason: FinishReason::Length,
                    usage: TokenUsage {
                        input_tokens: 40,
                        output_tokens: 8,
                        ..Default::default()
                    },
                })
                .await;
        });
        Ok(rx)
    }
}

/// Counts executions. A single execution is the defect: the call came out of a
/// severed response.
struct CountingTool {
    runs: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &str {
        "probe_tool"
    }

    fn description(&self) -> &str {
        "Records that it ran"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> ToolResult {
        self.runs.fetch_add(1, Ordering::SeqCst);
        ToolResult {
            content: "ran".to_string(),
            is_error: false,
        }
    }
}

/// Tools must actually EXECUTE here, or `tool_runs == 0` would hold for the
/// wrong reason (approval refusal, not the truncation gate). See the vacuity
/// control at the bottom of this file.
fn config() -> wcore_config::config::Config {
    wcore_config::config::Config {
        max_turns: Some(6),
        tools: wcore_config::config::ToolsConfig {
            auto_approve: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn complete_tool_calls_in_a_length_cut_response_are_not_executed() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let tool_runs = Arc::new(AtomicUsize::new(0));

    let provider = Arc::new(LengthCutProvider {
        calls: Arc::clone(&provider_calls),
    });
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool {
        runs: Arc::clone(&tool_runs),
    }));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;

    let mut engine = AgentEngine::new_with_provider(provider, config(), registry, output);
    let result = engine
        .run("do the thing", "")
        .await
        .expect("a truncated-output run terminates cleanly, not Err");

    assert_eq!(
        tool_runs.load(Ordering::SeqCst),
        0,
        "a tool call carved out of a finish_reason=length response must NOT run: the model \
         was still writing, so the calls that did arrive are a prefix of a plan whose \
         remainder was discarded"
    );

    let events = handle.snapshot();
    let saw_truncation_error = events.iter().any(|event| {
        event["type"].as_str() == Some("error") && event.to_string().contains("output token limit")
    });
    assert!(
        saw_truncation_error,
        "the run must say the output cap severed the turn; got {events:?}"
    );
    assert_eq!(
        result.finish_reason,
        FinishReason::Length,
        "the run ends on the output cap, not on the turn cap"
    );
    // One send + exactly one retry, then stop. Never a walk to max_turns.
    assert_eq!(
        provider_calls.load(Ordering::SeqCst),
        2,
        "the gate retries the severed turn exactly once"
    );
}

/// The gate must not fire on an ordinary length-cut TEXT answer: a truncated
/// prose reply carries no action, so it commits as before.
#[tokio::test]
async fn a_length_cut_text_answer_without_tool_calls_still_commits() {
    struct TextOnlyLengthCut;

    #[async_trait]
    impl LlmProvider for TextOnlyLengthCut {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx
                    .send(LlmEvent::TextDelta("half an answ".to_string()))
                    .await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: StopReason::MaxTokens,
                        finish_reason: FinishReason::Length,
                        usage: TokenUsage {
                            input_tokens: 40,
                            output_tokens: 8,
                            ..Default::default()
                        },
                    })
                    .await;
            });
            Ok(rx)
        }
    }

    let sink = Arc::new(TestSink::new());
    let output: Arc<dyn OutputSink> = sink;
    let mut engine = AgentEngine::new_with_provider(
        Arc::new(TextOnlyLengthCut),
        config(),
        ToolRegistry::new(),
        output,
    );
    let result = engine
        .run("write me an essay", "")
        .await
        .expect("run completes");

    assert!(
        result.text.contains("half an answ"),
        "a truncated prose answer is still the user's answer; got {:?}",
        result.text
    );
}

/// Keeps the ORIGINAL T3 arming path honest: a call cut mid-argument still
/// aborts, and still never runs.
#[tokio::test]
async fn a_call_cut_mid_argument_still_aborts() {
    struct MidArgumentCut {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmProvider for MidArgumentCut {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx
                    .send(LlmEvent::TruncatedToolCall {
                        name: "probe_tool".to_string(),
                        partial_arg_bytes: 41,
                    })
                    .await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: StopReason::MaxTokens,
                        finish_reason: FinishReason::Length,
                        usage: TokenUsage {
                            input_tokens: 40,
                            output_tokens: 8,
                            ..Default::default()
                        },
                    })
                    .await;
            });
            Ok(rx)
        }
    }

    let tool_runs = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool {
        runs: Arc::clone(&tool_runs),
    }));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let calls = Arc::new(AtomicUsize::new(0));
    let mut engine = AgentEngine::new_with_provider(
        Arc::new(MidArgumentCut {
            calls: Arc::clone(&calls),
        }),
        config(),
        registry,
        output,
    );
    engine.run("do the thing", "").await.expect("run completes");

    assert_eq!(tool_runs.load(Ordering::SeqCst), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "one send + one retry");
    let events = handle.snapshot();
    assert!(
        events.iter().any(|event| {
            event["type"].as_str() == Some("error")
                && event.to_string().contains("41 bytes of arguments received")
        }),
        "the mid-argument report must still name the partial payload size; got {events:?}"
    );
}

/// Vacuity control: with the SAME script but a clean `finish_reason=Stop`, the
/// tool DOES run. Without this, "tool_runs == 0" above could be satisfied by a
/// harness that never dispatches tools at all.
#[tokio::test]
async fn the_same_tool_call_runs_when_the_response_is_not_truncated() {
    struct CleanThenDone {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmProvider for CleanThenDone {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                if n == 0 {
                    let _ = tx
                        .send(LlmEvent::ToolUse {
                            id: "call-clean".to_string(),
                            name: "probe_tool".to_string(),
                            input: json!({ "step": 0 }),
                            extra: None,
                        })
                        .await;
                    let _ = tx
                        .send(LlmEvent::Done {
                            stop_reason: StopReason::ToolUse,
                            finish_reason: FinishReason::Stop,
                            usage: TokenUsage::default(),
                        })
                        .await;
                } else {
                    let _ = tx.send(LlmEvent::TextDelta("done".to_string())).await;
                    let _ = tx
                        .send(LlmEvent::Done {
                            stop_reason: StopReason::EndTurn,
                            finish_reason: FinishReason::Stop,
                            usage: TokenUsage::default(),
                        })
                        .await;
                }
            });
            Ok(rx)
        }
    }

    let tool_runs = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(CountingTool {
        runs: Arc::clone(&tool_runs),
    }));

    let sink = Arc::new(TestSink::new());
    let output: Arc<dyn OutputSink> = sink;
    let mut engine = AgentEngine::new_with_provider(
        Arc::new(CleanThenDone {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        config(),
        registry,
        output,
    );
    engine.run("do the thing", "").await.expect("run completes");

    assert_eq!(
        tool_runs.load(Ordering::SeqCst),
        1,
        "the harness really does dispatch tools — so a 0 above means the gate refused"
    );
}
