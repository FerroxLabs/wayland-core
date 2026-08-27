//! #559 — cache-prefix stability across an agentic loop.
//!
//! Prompt caching is the only thing standing between an agentic leader and
//! re-billing its whole context on every sub-call. On BOTH wires a hit
//! requires the same thing: the earlier request must be a byte-prefix of the
//! later one. Anthropic reads `cache_control` breakpoints against that prefix;
//! OpenAI-compatible upstreams (FluxRouter included) match the longest common
//! prefix automatically. Either way, one mutation anywhere behind the tail
//! discards everything after it.
//!
//! The engine takes deliberate care to keep it stable — the tool array is an
//! append-only union, the system prompt is built once, and the two volatile
//! values (current date, skill-router hint) are attached as TRANSIENT blocks on
//! the request's last user-role message rather than the cached prefix. All of
//! that is invisible at the provider boundary, and nothing gated it end to end:
//! `tools_array_byte_stable_across_roundtrips` (in `wcore-providers`) grades
//! the tool serializer on synthetic input, not the assembled request the engine
//! actually hands a provider.
//!
//! This gates it where it is decided. The invariant:
//!
//!   request N's messages, MINUS its last message, must be a byte-prefix of
//!   request N+1's messages
//!
//! i.e. history is append-only and the volatile region is exactly one message
//! deep. `system` must be byte-identical throughout.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockTool, test_config};

/// One captured request, reduced to the parts a prompt cache keys on.
#[derive(Clone)]
struct SeenRequest {
    system: String,
    messages: Vec<Value>,
}

/// Scripted provider that records what the engine actually sent.
struct RecordingProvider {
    responses: Mutex<Vec<Vec<LlmEvent>>>,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

impl RecordingProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> (Self, Arc<Mutex<Vec<SeenRequest>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                responses: Mutex::new(turns),
                seen: seen.clone(),
            },
            seen,
        )
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        if let Ok(mut g) = self.seen.lock() {
            g.push(SeenRequest {
                system: request.system.clone(),
                messages: request
                    .messages
                    .iter()
                    .map(|m| serde_json::to_value(m).expect("message serializes"))
                    .collect(),
            });
        }
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                    usage: TokenUsage::default(),
                }]
            } else {
                responses.remove(0)
            }
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

/// Quiet sink — this test observes the provider boundary, not the output side.
struct QuietSink(Arc<TerminalSink>);

impl OutputSink for QuietSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.0.emit_text_delta(text, msg_id);
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.0.emit_thinking(text, msg_id);
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        self.0.emit_tool_call(name, input);
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        self.0.emit_tool_result(name, is_error, content);
    }
    fn emit_stream_start(&self, msg_id: &str) {
        self.0.emit_stream_start(msg_id);
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
        self.0.emit_stream_end(
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
        self.0.emit_error(msg, retryable);
    }
    fn emit_info(&self, msg: &str) {
        self.0.emit_info(msg);
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
                input_tokens: 5_000,
                output_tokens: 40,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

async fn capture_agentic_loop() -> Vec<SeenRequest> {
    let (provider, seen) = RecordingProvider::new(vec![
        tool_turn("tu_1"),
        tool_turn("tu_2"),
        tool_turn("tu_3"),
        tool_turn("tu_4"),
        vec![
            LlmEvent::TextDelta("done".into()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                usage: TokenUsage {
                    input_tokens: 5_000,
                    output_tokens: 10,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ],
    ]);
    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new(
        "mock_tool",
        "127.0.0.1 localhost",
        false,
    )));

    let output: Arc<dyn OutputSink> = Arc::new(QuietSink(Arc::new(TerminalSink::new(true))));
    let mut engine = AgentEngine::new_with_provider(Arc::new(provider), config, registry, output);
    let result = engine.run("do the thing", "m-1").await.expect("engine ok");
    assert_eq!(result.turns, 5, "harness precondition: 5 round trips");

    let captured = seen.lock().unwrap().clone();
    assert_eq!(captured.len(), 5, "one recorded request per round trip");
    captured
}

/// The system block is cache zone 1. It must never move.
#[tokio::test]
async fn system_prompt_is_byte_identical_across_the_agentic_loop() {
    let seen = capture_agentic_loop().await;
    let first = &seen[0].system;
    for (i, r) in seen.iter().enumerate().skip(1) {
        assert_eq!(
            &r.system, first,
            "request {i}'s system block differs from request 0's — the whole \
             cached prefix is discarded on every turn"
        );
    }
}

/// History must be append-only, with a volatile region exactly one message deep.
#[tokio::test]
async fn message_history_is_append_only_behind_the_volatile_tail() {
    let seen = capture_agentic_loop().await;
    for w in seen.windows(2) {
        let (prev, next) = (&w[0], &w[1]);
        assert!(
            next.messages.len() > prev.messages.len(),
            "the conversation must grow: {} -> {}",
            prev.messages.len(),
            next.messages.len()
        );
        // Everything before the previous request's TAIL is cached prefix and
        // must survive byte-identical.
        let stable = prev.messages.len().saturating_sub(1);
        for i in 0..stable {
            assert_eq!(
                prev.messages[i],
                next.messages[i],
                "message[{i}] was rewritten between two consecutive requests \
                 ({} -> {} messages). Everything from index {i} onward is a \
                 cache MISS and gets re-billed at full rate.\nprev: {}\nnext: {}",
                prev.messages.len(),
                next.messages.len(),
                prev.messages[i],
                next.messages[i],
            );
        }
    }
}
