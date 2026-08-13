//! C-4b — a provider signature covering the turn's REASONING must survive the
//! engine and be replayed on the next request.
//!
//! Gemini is stateless about reasoning: it attaches `thoughtSignature` to the
//! part that carries the thought, and a replayed thought that comes back
//! without its signature is rejected. The signature therefore has to travel
//! provider → engine → assistant message → next request. This test drives the
//! real `AgentEngine::run()` loop over two turns and inspects the messages the
//! engine hands the provider on turn 2 — the only place that proves the whole
//! path, rather than one hop of it.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, StopReason, TokenUsage};

/// A mock provider that plays scripted turns and records every request it is
/// given, so the test can read back the history the engine replayed.
struct RecordingProvider {
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl RecordingProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(VecDeque::from(turns)),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn request(&self, index: usize) -> Vec<Message> {
        self.requests.lock().unwrap()[index].clone()
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.messages.clone());
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                usage: TokenUsage::default(),
            }]
        });
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

fn done(stop_reason: StopReason) -> LlmEvent {
    LlmEvent::Done {
        stop_reason,
        finish_reason: FinishReason::from_stop_reason(stop_reason),
        usage: TokenUsage::default(),
    }
}

#[tokio::test]
async fn signed_reasoning_is_replayed_on_the_next_request() {
    // Turn 1 mirrors what Gemini streams when it reasons before calling a
    // tool: reasoning text, a signature covering that reasoning, then the
    // call with its OWN, different signature.
    let turn1 = vec![
        LlmEvent::ThinkingDelta("weighing it".to_string()),
        LlmEvent::ThinkingSignature("sig-thought".to_string()),
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: Some(serde_json::json!({"thoughtSignature": "sig-call"})),
        },
        done(StopReason::ToolUse),
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("done".to_string()),
        done(StopReason::EndTurn),
    ];

    let provider = Arc::new(RecordingProvider::new(vec![turn1, turn2]));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "result",
        false,
    )));
    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));

    let mut engine =
        AgentEngine::new_with_provider(provider.clone(), common::test_config(), registry, output);
    engine
        .run("Do something", "msg-1")
        .await
        .expect("turn runs");

    // The second request carries the assistant turn the engine committed.
    let replayed = provider.request(1);
    let thinking = replayed
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|block| match block {
            ContentBlock::Thinking { thinking, extra } => Some((thinking.clone(), extra.clone())),
            _ => None,
        })
        .expect("the replayed history must still contain the reasoning block");

    assert_eq!(thinking.0, "weighing it");
    let extra = thinking
        .1
        .expect("the reasoning signature must be replayed — a stateless provider rejects a signed thought sent back bare");
    assert_eq!(extra["thoughtSignature"], "sig-thought");

    // The call's signature is a DIFFERENT value on a DIFFERENT block; the two
    // must not be crossed or collapsed into one.
    let call_extra = replayed
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|block| match block {
            ContentBlock::ToolUse { extra, .. } => extra.clone(),
            _ => None,
        })
        .expect("the tool call must keep its own signature");
    assert_eq!(call_extra["thoughtSignature"], "sig-call");
}
