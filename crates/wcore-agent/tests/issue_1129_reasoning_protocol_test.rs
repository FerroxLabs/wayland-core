//! #1129 — reasoning tags on the JSON-stream protocol path.
//!
//! The CLI TUI strips `<think>`/`<thinking>`/`<reasoning>` before the text
//! reaches its visible buffer. Nothing on the JSON-stream protocol path did,
//! so a Desktop host rendered the literal tag body inside the assistant
//! bubble.
//!
//! These tests drive the REAL producer (`ProtocolSink`) through the REAL
//! serializer (`serde_json::to_vec`, byte-identical to `ProtocolWriter::emit`)
//! and assert on the wire lines a host actually reads.

use std::sync::{Arc, Mutex};

use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_types::message::FinishReason;

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
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
    fn dump(&self, label: &str) {
        println!("--- {label} ---");
        for l in self.lines() {
            println!("{l}");
        }
        println!("--- end {label} ---");
    }
    /// Concatenated `text` of every `text_delta` frame.
    fn visible_text(&self) -> String {
        self.lines()
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["type"] == "text_delta")
            .map(|v| v["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }
    /// Concatenated `text` of every `thinking` frame.
    fn thinking_text(&self) -> String {
        self.lines()
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["type"] == "thinking")
            .map(|v| v["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

fn drive(chunks: &[&str]) -> Arc<WireRecorder> {
    let rec = Arc::new(WireRecorder::default());
    let sink = ProtocolSink::with_emitter(rec.clone());
    sink.emit_stream_start("m-1");
    for c in chunks {
        sink.emit_text_delta(c, "m-1");
    }
    sink.emit_stream_end("m-1", 1, 0, 0, 0, 0, FinishReason::Stop);
    rec
}

/// Every spelling the filter recognises, each in its own turn, plus the
/// attribute / case / self-closing / nested forms.
#[test]
fn no_reasoning_tag_reaches_text_delta_in_any_spelling() {
    let cases: &[(&str, &[&str], &str)] = &[
        (
            "plain think",
            &["Hello <think>secret</think> world"],
            "Hello  world",
        ),
        (
            "thinking",
            &["<thinking>secret</thinking>visible"],
            "visible",
        ),
        (
            "reasoning",
            &["<reasoning>secret</reasoning>visible"],
            "visible",
        ),
        ("uppercase", &["<THINK>secret</think>visible"], "visible"),
        ("mixed case", &["<Thinking>secret</Thinking>tail"], "tail"),
        (
            "attributes",
            &["<thinking budget=\"8k\">secret</thinking>ok"],
            "ok",
        ),
        ("self closing", &["a<think/>b"], "ab"),
        ("nested", &["<think>a<think>b</think>c</think>out"], "out"),
        // The one that regresses silently: the tag straddles chunk boundaries.
        (
            "split open tag",
            &["Hi <thi", "nk>secret</think> bye"],
            "Hi  bye",
        ),
        ("split close tag", &["<think>secret</thi", "nk>bye"], "bye"),
        ("split one char at a time", split_chars(), "AB"),
    ];
    let mut failures = Vec::new();
    for (label, chunks, expect_visible) in cases {
        let rec = drive(chunks);
        rec.dump(label);
        let visible = rec.visible_text();
        if visible != *expect_visible {
            failures.push(format!(
                "{label}: text_delta carried {visible:?}, expected {expect_visible:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "#1129 leaks:\n{}", failures.join("\n"));
}

fn split_chars() -> &'static [&'static str] {
    &[
        "A", "<", "t", "h", "i", "n", "k", ">", "s", "<", "/", "t", "h", "i", "n", "k", ">", "B",
    ]
}

/// The stripped reasoning must not be DELETED — it rides its own typed event
/// so a host can render it collapsed, exactly as the TUI does.
#[test]
fn stripped_reasoning_is_re_emitted_as_a_typed_thinking_event() {
    let rec = drive(&["Hello <think>the secret plan</think> world"]);
    rec.dump("typed thinking");
    assert_eq!(rec.visible_text(), "Hello  world");
    assert_eq!(rec.thinking_text(), "the secret plan");
}

/// Multiple blocks in one turn keep the TUI's `\n` separator on the wire.
#[test]
fn two_reasoning_blocks_are_separated_on_the_wire() {
    let rec = drive(&["<think>one</think>mid<think>two</think>end"]);
    rec.dump("two blocks");
    assert_eq!(rec.visible_text(), "midend");
    assert_eq!(rec.thinking_text(), "one\ntwo");
}

/// An unclosed tag eats to end of stream in the filter. The content must still
/// reach the host as `thinking` rather than vanishing.
#[test]
fn unclosed_reasoning_block_is_flushed_at_stream_end() {
    let rec = drive(&["visible <think>runaway tail"]);
    rec.dump("unclosed");
    assert_eq!(rec.visible_text(), "visible ");
    assert_eq!(rec.thinking_text(), "runaway tail");
}

/// Ordinary prose with tag-like content that is NOT a reasoning tag must pass
/// through untouched — the filter is not an HTML sanitiser. This is the
/// positive control for the assertions above: it proves the pipeline under
/// test transports text at all.
#[test]
fn non_reasoning_markup_passes_through_untouched() {
    let rec = drive(&["use <b>bold</b> and 5 < 6 and <thinker>x</thinker>"]);
    rec.dump("control");
    assert_eq!(
        rec.visible_text(),
        "use <b>bold</b> and 5 < 6 and <thinker>x</thinker>"
    );
    assert_eq!(rec.thinking_text(), "");
}

/// A runaway UNCLOSED `<think>` eats to end of stream by design. The filter is
/// stateful and the sink outlives the turn, so without a reset at
/// `stream_start` that block would keep eating the NEXT turn's entire visible
/// answer — a silent empty reply, which is worse than the leak this change
/// fixes. One sink, two turns.
#[test]
fn a_runaway_block_cannot_swallow_the_next_turn() {
    let rec = Arc::new(WireRecorder::default());
    let sink = ProtocolSink::with_emitter(rec.clone());

    sink.emit_stream_start("m-1");
    sink.emit_text_delta("turn one <think>never closed", "m-1");
    sink.emit_stream_end("m-1", 1, 0, 0, 0, 0, FinishReason::Stop);

    sink.emit_stream_start("m-2");
    sink.emit_text_delta("turn two answer", "m-2");
    sink.emit_stream_end("m-2", 1, 0, 0, 0, 0, FinishReason::Stop);

    rec.dump("two turns");
    let turn_two: String = rec
        .lines()
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["type"] == "text_delta" && v["msg_id"] == "m-2")
        .map(|v| v["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(turn_two, "turn two answer");
}
