//! FerroxLabs/wayland#1231 — an all-reasoning turn must give the user an ANSWER,
//! not only an accurate explanation of why there is none.
//!
//! Split out of wayland#908 c3 after that criterion was refuted twice over:
//! the evidence was a hand-authored `TextDelta` (a mock of a HYPOTHESISED
//! cause), and every assertion in it was about the error STRING, none about
//! the user receiving an answer.
//!
//! Both substitutions are undone here. The stream is CAPTURED from a real
//! model (see `fixtures/issue_1231_qwen3_all_reasoning.rs`), and the
//! assertions are about the ANSWER — what reached the text stream, and what
//! reached the conversation.

mod common;

#[path = "fixtures/issue_1231_qwen3_all_reasoning.rs"]
mod captured;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::{AgentEngine, REASONING_RECOVERY_LABEL};
use wcore_agent::output::OutputSink;
use wcore_protocol::events::FailureCategory;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, Role, StopReason, TokenUsage};

use common::test_config;

/// Replays one scripted turn's events.
struct ScriptedProvider {
    script: Mutex<std::collections::VecDeque<Vec<LlmEvent>>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let events = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(done_only);
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 10,
        output_tokens: 60,
        ..Default::default()
    }
}

fn done() -> LlmEvent {
    LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::Stop,
        usage: usage(),
    }
}

fn done_only() -> Vec<LlmEvent> {
    vec![done()]
}

/// The captured stream, replayed delta for delta.
fn captured_turn() -> Vec<LlmEvent> {
    let mut events: Vec<LlmEvent> = captured::CAPTURED_DELTAS
        .iter()
        .map(|d| LlmEvent::TextDelta((*d).to_string()))
        .collect();
    events.push(done());
    events
}

#[derive(Default)]
struct CapSink {
    text: Mutex<String>,
    errors: Mutex<Vec<String>>,
    infos: Mutex<Vec<String>>,
}

impl OutputSink for CapSink {
    fn emit_text_delta(&self, text: &str, _: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, message: &str, _: bool, _: FailureCategory) {
        self.errors.lock().unwrap().push(message.to_string());
    }
    fn emit_info(&self, message: &str) {
        self.infos.lock().unwrap().push(message.to_string());
    }
}

fn engine_replaying(script: Vec<Vec<LlmEvent>>) -> (AgentEngine, Arc<CapSink>) {
    let provider = Arc::new(ScriptedProvider {
        script: Mutex::new(script.into_iter().collect()),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let mut config = test_config();
    config.max_turns = Some(2);
    let sink = Arc::new(CapSink::default());
    let engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        sink.clone() as Arc<dyn OutputSink>,
    );
    (engine, sink)
}

/// The captured stream really is the shape c1 asks for. Without this the tests
/// below could pass against a fixture that was never all-reasoning at all.
#[test]
fn control_the_captured_stream_is_entirely_inside_reasoning_tags() {
    let joined: String = captured::CAPTURED_DELTAS.concat();
    assert!(!joined.is_empty(), "the fixture is not empty");
    assert!(
        joined.trim().starts_with("<thought>") && joined.trim().ends_with("</thought>"),
        "c1: the captured reply must be entirely inside reasoning tags, or it \
         is not the class under test. Got: {joined:?}"
    );
    assert!(
        captured::CAPTURED_DELTAS.len() > 20,
        "c1: a captured stream arrives as many small deltas ({} here); a \
         one-shot string would be the hand-authored fixture c1 refuses",
        captured::CAPTURED_DELTAS.len()
    );
    // The filter really does empty it — the precondition the whole issue rests
    // on. If a future filter change stopped emptying this, every assertion
    // below would pass for the wrong reason.
    let mut filter = wcore_types::reasoning_filter::ReasoningFilter::new();
    let mut filtered = String::new();
    for delta in captured::CAPTURED_DELTAS {
        filtered.push_str(&filter.process(delta));
    }
    filtered.push_str(&filter.finish());
    assert!(
        filtered.trim().is_empty(),
        "c1: `assistant_text` must be empty while raw_text_chars > 0. The \
         filter left: {filtered:?}"
    );
}

/// c2 — THE USER GETS AN ANSWER.
#[tokio::test]
async fn an_all_reasoning_turn_gives_the_user_an_answer() {
    let (mut engine, sink) = engine_replaying(vec![captured_turn()]);
    let _ = engine.run("what is 2 + 2?", "msg-1231").await;

    let text = sink.text.lock().unwrap().clone();
    assert!(
        text.contains(REASONING_RECOVERY_LABEL),
        "c2: the recovered answer must be CLEARLY LABELLED as coming from the \
         model's reasoning. Text stream was: {text:?}"
    );
    assert!(
        text.contains("the answer is 4"),
        "c2: the model's actual answer -- which was there the whole time and \
         our filter removed it -- must reach the user. Text stream was: {text:?}"
    );
}

/// c3 — THE CONVERSATION SURVIVES IT. Today the empty turn is deliberately
/// dropped, so this is a separate criterion from c2.
#[tokio::test]
async fn the_recovered_answer_is_committed_to_history() {
    let (mut engine, sink) = engine_replaying(vec![captured_turn()]);
    let _ = engine.run("what is 2 + 2?", "msg-1231").await;
    let _ = sink;

    let assistant: Vec<_> = engine
        .conversation_messages()
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .collect();
    assert!(
        !assistant.is_empty(),
        "c3: the turn must be committed. An empty turn is dropped, so a \
         recovered answer that is not committed leaves the conversation with \
         no record of it and the next turn cannot see it"
    );
    let recorded = format!("{assistant:?}");
    assert!(
        recorded.contains("the answer is 4"),
        "c3: the recovered answer must be IN the committed message, not merely \
         alongside a committed empty one: {recorded}"
    );
}

/// c4 — NEGATIVE CONTROL. A turn that genuinely produced nothing still gets
/// the honest empty-turn diagnosis and NO fabricated answer.
///
/// This is the row that blocks an always-fires recovery.
#[tokio::test]
async fn control_a_turn_that_produced_nothing_gets_the_diagnosis_and_no_answer() {
    let (mut engine, sink) = engine_replaying(vec![done_only()]);
    let _ = engine.run("hello", "msg-1231").await;

    let text = sink.text.lock().unwrap().clone();
    assert!(
        text.trim().is_empty(),
        "c4: nothing arrived, so nothing may be surfaced as an answer. Got: {text:?}"
    );
    assert!(
        !text.contains(REASONING_RECOVERY_LABEL),
        "c4: no recovery label on a turn with nothing to recover"
    );
    let errors = sink.errors.lock().unwrap().clone();
    assert!(
        errors.iter().any(|e| e.contains("empty response")),
        "c4: the honest empty-turn diagnosis must still fire: {errors:?}"
    );
}

/// c5's other half, driven through the ENGINE rather than the filter.
///
/// A reply that is nothing but unmatched closing tags has raw text and no
/// captured body, so there is no answer in it to recover. The recovery must
/// NOT fire and the #908 diagnosis must stand — this is the case where "there
/// is genuinely no answer" is the true statement.
#[tokio::test]
async fn control_a_turn_of_only_stray_closing_tags_recovers_nothing() {
    let strays: Vec<LlmEvent> = vec![
        LlmEvent::TextDelta("</thought>".to_string()),
        LlmEvent::TextDelta("</thought>".to_string()),
        LlmEvent::TextDelta("</thought>".to_string()),
        LlmEvent::TextDelta("</thought>".to_string()),
        LlmEvent::TextDelta("</thought>".to_string()),
        done(),
    ];
    let (mut engine, sink) = engine_replaying(vec![strays]);
    let _ = engine.run("hello", "msg-1231").await;

    let text = sink.text.lock().unwrap().clone();
    assert!(
        !text.contains(REASONING_RECOVERY_LABEL),
        "c5: five bare closing tags carry no answer, so nothing may be \
         labelled as a recovered one. Got: {text:?}"
    );
    let errors = sink.errors.lock().unwrap().clone();
    assert!(
        errors.iter().any(|e| e.contains("reasoning tags")),
        "c5: the #908 diagnosis is the right answer here, and must stand: {errors:?}"
    );
}

/// The recovery does not fire on an ORDINARY turn. Without this, a change that
/// labelled every reply as recovered would pass everything above.
#[tokio::test]
async fn control_an_ordinary_answer_is_not_labelled_as_recovered() {
    let ordinary = vec![LlmEvent::TextDelta("The answer is 4.".to_string()), done()];
    let (mut engine, sink) = engine_replaying(vec![ordinary]);
    let _ = engine.run("what is 2 + 2?", "msg-1231").await;

    let text = sink.text.lock().unwrap().clone();
    assert!(
        text.contains("The answer is 4."),
        "control: the answer arrived"
    );
    assert!(
        !text.contains(REASONING_RECOVERY_LABEL),
        "an ordinary answer must not be labelled as recovered: {text:?}"
    );
    assert!(
        sink.errors.lock().unwrap().is_empty(),
        "control: an ordinary turn emits no error"
    );
}
