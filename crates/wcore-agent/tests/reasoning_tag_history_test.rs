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

// ---------------------------------------------------------------------------
// #908 — the history-side filter must not LOSE text it was only meant to move.
//
// The filter is a display filter: an opening tag that never closes eats to the
// end of the stream (a deliberate v0.9.0 choice — better to hide a runaway
// reasoning tail than leak it), and an ambiguous `<` prefix that never
// resolves is dropped with it. Both were survivable while the only cost was a
// rendering. They are not survivable on the DURABLE record: `assistant_text`
// is the stored assistant block, the session mirror, the journal, and the text
// replayed to the provider on the next turn.
// ---------------------------------------------------------------------------

/// #908. An assistant answer that merely MENTIONS a reasoning tag in prose has
/// no closing tag, so the filter ate everything after the word — silently, and
/// with no empty-turn notice, because the surviving prefix is not empty.
///
/// Asking the agent about `<think>` tags, a prompt format, or this very bug is
/// an ordinary thing to do.
#[tokio::test]
async fn a_prose_mention_of_a_reasoning_tag_survives_in_history() {
    let answer = "Use the <thinking> tag to wrap reasoning. Then answer.";
    let (text, stored) = run_turn(&[answer]).await;

    assert_eq!(
        text, answer,
        "an unclosed reasoning tag ate the rest of the answer"
    );
    assert_eq!(
        stored,
        vec![answer.to_string()],
        "the durable conversation record lost everything after `<thinking>`"
    );
}

/// #908. Same loss, arriving across chunk boundaries — the shape a real
/// provider streams it in.
#[tokio::test]
async fn a_prose_mention_split_across_deltas_survives_in_history() {
    let (text, stored) = run_turn(&["Wrap it in <thin", "king> and then ", "answer."]).await;

    assert_eq!(text, "Wrap it in <thinking> and then answer.");
    assert_eq!(
        stored,
        vec!["Wrap it in <thinking> and then answer.".to_string()]
    );
}

/// #908. The filter buffers an ambiguous `<` prefix waiting to learn whether it
/// starts a tag. Nothing drained that buffer at end of stream, so an answer
/// that ends in `<` — or in a partial angle-bracket token — lost those
/// characters from the stored record.
#[tokio::test]
async fn an_answer_ending_in_an_ambiguous_prefix_keeps_every_character() {
    let (text, stored) = run_turn(&["the answer is 5 <"]).await;
    assert_eq!(text, "the answer is 5 <", "the trailing `<` was dropped");
    assert_eq!(stored, vec!["the answer is 5 <".to_string()]);

    let (text, stored) = run_turn(&["result: <th"]).await;
    assert_eq!(text, "result: <th", "three characters were dropped");
    assert_eq!(stored, vec!["result: <th".to_string()]);
}

/// Control — a properly CLOSED reasoning block is still removed from history,
/// including one that closes on the very last delta. Without this a fix that
/// simply stopped filtering would pass every test above.
#[tokio::test]
async fn a_closed_reasoning_block_is_still_removed_from_history() {
    let (text, stored) = run_turn(&["a<think>hidden", "</think>b"]).await;
    assert_eq!(text, "ab");
    assert_eq!(stored, vec!["ab".to_string()]);

    let (text, stored) = run_turn(&["<think>all of it</think>"]).await;
    assert_eq!(text, "", "a wholly-reasoning turn must still store nothing");
    assert_eq!(stored, Vec::<String>::new());
}
