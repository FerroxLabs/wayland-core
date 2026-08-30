//! wayland#1242 — the display half of wayland#1221 / wayland#1222.
//!
//! Those two drained the reasoning filter on the DURABLE record. Every
//! DISPLAY-side consumer of the same filter still called only `process` and
//! `reset`, so each still withheld an undecided `<`-prefix and everything
//! after an unclosed reasoning tag, and still dropped it when the stream
//! ended. The screen and the stored turn disagreed about the same answer.
//!
//! The assertions here read BOTH from ONE run: the text a host is SHOWN
//! (`text_delta` frames off a real `ProtocolSink`, the display-side consumer
//! that is also the wire contract with the desktop app) and the text the
//! engine STORED (the assistant `ContentBlock::Text`). One engine turn, one
//! provider stream, two lanes, byte-compared.

mod common;

use std::sync::{Arc, Mutex};

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{ContentBlock, FinishReason, Role, StopReason, TokenUsage};

use common::{MockLlmProvider, test_config};

/// Records the exact JSON Lines `ProtocolWriter` would have written.
#[derive(Default)]
struct WireRecorder {
    lines: Mutex<Vec<String>>,
}

impl ProtocolEmitter for WireRecorder {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        let line = String::from_utf8(serde_json::to_vec(event).expect("serialize")).expect("utf8");
        self.lines.lock().unwrap().push(line);
        Ok(())
    }
}

impl WireRecorder {
    fn frames(&self) -> Vec<serde_json::Value> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .collect()
    }
    fn of_type(&self, kind: &str) -> String {
        self.frames()
            .iter()
            .filter(|v| v["type"] == kind)
            .map(|v| v["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }
    fn shown(&self) -> String {
        self.of_type("text_delta")
    }
    fn thought(&self) -> String {
        self.of_type("thinking")
    }
    fn dump(&self, label: &str) {
        println!("--- {label} ---");
        for l in self.lines.lock().unwrap().iter() {
            println!("{l}");
        }
        println!("--- end {label} ---");
    }
}

struct Turn {
    stored: String,
    wire: Arc<WireRecorder>,
}

/// One engine turn whose provider streams `deltas`, with a REAL `ProtocolSink`
/// attached as the display consumer.
async fn run_turn(deltas: &[&str]) -> Turn {
    let mut events: Vec<LlmEvent> = deltas
        .iter()
        .map(|d| LlmEvent::TextDelta((*d).to_string()))
        .collect();
    events.push(LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
        usage: TokenUsage::default(),
    });

    let provider = Arc::new(MockLlmProvider::with_turns(vec![events]));
    let wire = Arc::new(WireRecorder::default());
    let sink = Arc::new(ProtocolSink::with_emitter(wire.clone()));

    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        sink.clone() as Arc<dyn OutputSink>,
    );
    engine.run("hi", "").await.expect("engine should succeed");
    // The engine does NOT close the stream: `emit_stream_end` is the CALLER's,
    // and in production that caller is `wcore_cli::main`, which emits it on
    // every terminal path of a turn. This line stands in for exactly that, so
    // the sink under test sees the same end-of-stream hook it sees live.
    OutputSink::emit_stream_end(&*sink, "", 1, 0, 0, 0, 0, FinishReason::Stop);

    let stored = engine
        .conversation_messages()
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    Turn { stored, wire }
}

// ---------------------------------------------------------------------------
// c2 — what the user is SHOWN and what is STORED are the same bytes.
// ---------------------------------------------------------------------------

/// The input the ticket names, plus the two wayland#1222 measured its own
/// defect with. Each is one turn; each asserts the two lanes against each
/// other AND against the provider's own bytes, so a fix that made them agree
/// by breaking BOTH would still fail.
#[tokio::test]
async fn wayland1242_c2_the_shown_text_and_the_stored_text_are_the_same_bytes() {
    for input in [
        "Use the <thinking> tag to wrap reasoning. Then answer.",
        "the answer is 5 <",
        "result: <th",
    ] {
        let turn = run_turn(&[input]).await;
        turn.wire.dump(input);

        assert_eq!(
            turn.stored, input,
            "the stored turn is not what the provider streamed"
        );
        assert_eq!(
            turn.wire.shown(),
            turn.stored,
            "the host is shown a different answer from the one stored for the \
             same turn"
        );
    }
}

/// The same defect with the tag straddling a chunk boundary — the state the
/// filter is stateful for. A drain done per-delta rather than per-stream
/// passes the test above and fails this one.
#[tokio::test]
async fn an_unclosed_tag_split_across_deltas_reaches_the_host_whole() {
    let turn = run_turn(&["Use the <thi", "nking> tag, then answer."]).await;
    turn.wire.dump("split");

    assert_eq!(turn.stored, "Use the <thinking> tag, then answer.");
    assert_eq!(turn.wire.shown(), turn.stored);
}

// ---------------------------------------------------------------------------
// c4 — the control. A CLOSED block is still stripped and still rendered as a
// Thought, on the same run that proves the recovery above.
// ---------------------------------------------------------------------------

/// wayland#908 c1 must not regress: a real, closed reasoning block is still
/// taken out of the answer and still reaches the host as `thinking`.
///
/// This is also the sensitivity control for the two tests above. If the
/// display-side drain were implemented by NOT filtering — the inadmissible
/// fix — those two would pass and this one would fail, because the tag body
/// would arrive as answer text and no `thinking` frame would be produced.
#[tokio::test]
async fn wayland1242_c4_a_closed_reasoning_block_is_still_stripped_and_still_a_thought() {
    let turn = run_turn(&["Hello <think>the secret plan</think> world"]).await;
    turn.wire.dump("closed block");

    assert_eq!(
        turn.wire.shown(),
        "Hello  world",
        "a closed reasoning block leaked into the answer the host is shown"
    );
    assert_eq!(
        turn.wire.thought(),
        "the secret plan",
        "the closed block is no longer rendered as a Thought"
    );
    assert_eq!(
        turn.stored, "Hello  world",
        "the stored turn disagrees with the host about a CLOSED block"
    );
}

/// Over-emission control. A turn with nothing withheld must not gain a
/// trailing empty or duplicated `text_delta` from the new drain.
#[tokio::test]
async fn a_turn_that_withheld_nothing_gains_nothing() {
    let turn = run_turn(&["plain answer, ", "no tags at all"]).await;
    turn.wire.dump("nothing withheld");

    assert_eq!(turn.wire.shown(), "plain answer, no tags at all");
    assert_eq!(turn.stored, turn.wire.shown());
    let deltas = turn
        .wire
        .frames()
        .iter()
        .filter(|v| v["type"] == "text_delta")
        .count();
    assert_eq!(
        deltas, 2,
        "the drain emitted an extra text_delta for a turn that withheld nothing"
    );
}
