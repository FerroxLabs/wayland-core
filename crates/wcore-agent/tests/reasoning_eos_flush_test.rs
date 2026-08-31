//! wayland#1221 / wayland#1222 — the reasoning filter sits on the DURABLE
//! conversation record, so anything it holds back at end of stream is history
//! that is gone for good.
//!
//! 508405d4 (#908 c1) routed `assistant_text` — the assistant
//! `ContentBlock::Text`, the session mirror, the journal, and the text
//! replayed upstream on the next request — through `ReasoningFilter::process`.
//! `process` is a lossy view on its own: it withholds an undecided
//! `<`-prefix, and it withholds everything after an opening reasoning tag
//! until that tag closes. Neither was ever drained.
//!
//! Every assertion below is on the STORED assistant text, not on the streamed
//! copy — that is the sentence both tickets are written against. The controls
//! at the bottom keep the fix honest in both directions: a CLOSED reasoning
//! block must still be stripped (#908 c1 must not regress), and a turn that
//! lost nothing must not emit the loss notice.

mod common;

use std::sync::{Arc, Mutex};

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{ContentBlock, FinishReason, Role, StopReason, TokenUsage};

use common::{MockLlmProvider, test_config};

// ---------------------------------------------------------------------------
// Capturing sink — the `emit_info` channel is where the wayland#1221 c3
// partial-strip notice lands, and `emit_error` is where the pre-existing
// empty-turn notice lands. Both are recorded so a test can tell which guard
// fired.
// ---------------------------------------------------------------------------
#[derive(Default)]
struct CapSink {
    infos: Mutex<Vec<String>>,
    errors: Mutex<Vec<String>>,
}

impl CapSink {
    fn infos(&self) -> Vec<String> {
        self.infos.lock().unwrap().clone()
    }
    fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}

impl OutputSink for CapSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(
        &self,
        message: &str,
        _: bool,
        _category: wcore_protocol::events::FailureCategory,
    ) {
        self.errors.lock().unwrap().push(message.to_string());
    }
    fn emit_info(&self, message: &str) {
        self.infos.lock().unwrap().push(message.to_string());
    }
}

fn end_turn() -> LlmEvent {
    LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
        usage: TokenUsage::default(),
    }
}

struct Turn {
    stored: Vec<String>,
    sink: Arc<CapSink>,
}

/// Run one turn whose text deltas are `deltas` and report what the DURABLE
/// record kept, plus everything the user was told.
async fn run_turn(deltas: &[&str]) -> Turn {
    let mut events: Vec<LlmEvent> = deltas
        .iter()
        .map(|d| LlmEvent::TextDelta((*d).to_string()))
        .collect();
    events.push(end_turn());

    let provider = Arc::new(MockLlmProvider::with_turns(vec![events]));
    let sink = Arc::new(CapSink::default());
    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        sink.clone() as Arc<dyn OutputSink>,
    );
    engine.run("hi", "").await.expect("engine should succeed");

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

    Turn { stored, sink }
}

fn notice(infos: &[String]) -> Option<&String> {
    infos.iter().find(|m| m.contains("NOT stored in the"))
}

// ---------------------------------------------------------------------------
// wayland#1221 c1 / c4 — the measured input, asserted on the stored
// ContentBlock::Text.
// ---------------------------------------------------------------------------

/// The exact string the ticket measured. `<thinking>` never closes, so before
/// the end-of-stream drain the stored block was `"Use the "` and the rest of
/// the sentence was filed into the capture buffer and thrown away with it.
#[tokio::test]
async fn wayland1221_c1_prose_that_mentions_a_reasoning_tag_survives_intact_in_stored_history() {
    const INPUT: &str = "Use the <thinking> tag to wrap reasoning. Then answer.";

    let turn = run_turn(&[INPUT]).await;

    assert_eq!(
        turn.stored,
        vec![INPUT.to_string()],
        "the stored assistant ContentBlock::Text lost everything after `<thinking>`"
    );
}

/// The same defect with the tag straddling a chunk boundary — the state that
/// makes the filter stateful in the first place. A per-delta drain would pass
/// the test above and fail this one.
#[tokio::test]
async fn wayland1221_an_unclosed_tag_split_across_deltas_is_recovered_whole() {
    let turn = run_turn(&["Use the <thi", "nking> tag, then answer."]).await;

    assert_eq!(
        turn.stored,
        vec!["Use the <thinking> tag, then answer.".to_string()],
        "recovery must reassemble the opening tag from both deltas"
    );
}

// ---------------------------------------------------------------------------
// wayland#1222 c1 — the ambiguous-prefix buffer, byte-exact through a
// completed turn.
// ---------------------------------------------------------------------------

/// Both inputs the ticket measured, in one test so the criterion has one
/// anchor: `"the answer is 5 <"` lost its trailing `<`, `"result: <th"` lost
/// three characters. Neither ever became a tag — the stream simply ended
/// while the filter was still deciding.
#[tokio::test]
async fn wayland1222_c1_both_measured_inputs_survive_byte_exact() {
    for input in ["the answer is 5 <", "result: <th"] {
        let turn = run_turn(&[input]).await;
        assert_eq!(
            turn.stored,
            vec![input.to_string()],
            "`{input}` did not survive byte-exact into stored history"
        );
    }
}

// ---------------------------------------------------------------------------
// wayland#1222 c4 — the controls the ticket names stay byte-exact. These pass
// TODAY; they are here so a flush that over-emits (double-writing a pending
// buffer, or re-emitting a resolved tag) reds instead of shipping.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wayland1222_c4_controls_stay_byte_exact() {
    for input in ["if a < b then", "if a <b then c", "<div>hello</div>"] {
        let turn = run_turn(&[input]).await;
        assert_eq!(
            turn.stored,
            vec![input.to_string()],
            "control `{input}` is no longer byte-exact in stored history"
        );
        assert!(
            notice(&turn.sink.infos()).is_none(),
            "control `{input}` lost nothing, so nothing should be announced"
        );
    }
}

// ---------------------------------------------------------------------------
// wayland#908 c1 must NOT regress: a reasoning block that CLOSES is still
// stripped from the durable record. This is the control that stops "recover
// everything" from passing as a fix.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn control_a_closed_reasoning_block_is_still_stripped_from_stored_history() {
    let turn = run_turn(&["<think>plan the answer</think>", "42"]).await;

    assert_eq!(
        turn.stored,
        vec!["42".to_string()],
        "a CLOSED reasoning block leaked back into durable history"
    );
}

// ---------------------------------------------------------------------------
// wayland#1221 c3 — a PARTIAL strip is announced. The pre-existing guard fires
// only when the strip is total; this turn is non-empty, so it cannot.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wayland1221_c3_a_partial_strip_is_announced_to_the_user() {
    let turn = run_turn(&["<think>hidden</think>The answer is 42."]).await;

    assert_eq!(
        turn.stored,
        vec!["The answer is 42.".to_string()],
        "harness precondition: the turn must be non-empty and partially stripped"
    );
    let infos = turn.sink.infos();
    let told = notice(&infos).unwrap_or_else(|| {
        panic!("a partial strip went unannounced; infos were {infos:?}");
    });
    assert!(
        told.starts_with("21 characters"),
        "the notice must say HOW MUCH left, got {told:?}"
    );
    assert!(
        turn.sink.errors().is_empty(),
        "the empty-turn guard must not fire on a non-empty turn"
    );
}

/// Anti-vacuity control for the notice: a turn that lost nothing must say
/// nothing. Without this, "always emit the notice" would pass the test above.
#[tokio::test]
async fn control_a_turn_that_lost_nothing_announces_nothing() {
    let turn = run_turn(&["The answer is 42."]).await;

    assert_eq!(turn.stored, vec!["The answer is 42.".to_string()]);
    assert!(
        notice(&turn.sink.infos()).is_none(),
        "nothing was stripped, so nothing should be announced: {:?}",
        turn.sink.infos()
    );
}
