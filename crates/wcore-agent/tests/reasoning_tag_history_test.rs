//! #908 — inline reasoning tags must not survive into conversation history.
//!
//! `ProtocolSink`/`ChannelSink` filter the STREAMED COPY of each text delta,
//! but the engine appends the RAW delta to `assistant_text`. That string
//! becomes the assistant `ContentBlock::Text` in the conversation, the session
//! mirror and the journal — so a resumed session re-renders the reasoning the
//! live stream had already hidden, and the tags are sent back to the provider.

mod common;

use std::sync::Arc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{ContentBlock, FinishReason, Role, StopReason, TokenUsage};

use common::{MockLlmProvider, test_config};

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

fn end_turn() -> LlmEvent {
    LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
        usage: TokenUsage::default(),
    }
}

/// Run one turn whose text deltas are `deltas`, and return
/// `(result.text, stored assistant text blocks)`.
async fn run_turn(deltas: &[&str]) -> (String, Vec<String>) {
    let mut events: Vec<LlmEvent> = deltas
        .iter()
        .map(|d| LlmEvent::TextDelta((*d).to_string()))
        .collect();
    events.push(end_turn());

    let provider = Arc::new(MockLlmProvider::with_turns(vec![events]));
    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        silent_output(),
    );
    let result = engine.run("hi", "").await.expect("engine should succeed");

    let stored = engine
        .conversation_messages()
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();

    (result.text, stored)
}

#[tokio::test]
async fn inline_reasoning_block_is_stripped_from_conversation_history() {
    let (text, stored) = run_turn(&["<think>plan the answer</think>", "42"]).await;

    assert_eq!(text, "42", "returned answer still carries reasoning");
    assert_eq!(
        stored,
        vec!["42".to_string()],
        "history still carries reasoning"
    );
}

#[tokio::test]
async fn stray_close_tag_is_stripped_from_conversation_history() {
    let (text, stored) = run_turn(&["The answer is 42.", "</thought>"]).await;

    assert_eq!(text, "The answer is 42.");
    assert_eq!(stored, vec!["The answer is 42.".to_string()]);
}

#[tokio::test]
async fn reasoning_tag_split_across_deltas_is_stripped_from_history() {
    // The tag straddles the chunk boundary, so a per-delta filter would miss
    // it: the engine's history filter has to be stateful across the turn.
    let (text, stored) = run_turn(&["a<thi", "nk>hidden</th", "ink>b"]).await;

    assert_eq!(text, "ab");
    assert_eq!(stored, vec!["ab".to_string()]);
}
