//! #1129 (live production gap) — reasoning tags on the SUB-AGENT relay path.
//!
//! v0.13.7 stripped inline reasoning in `ProtocolSink::emit_text_delta`, but a
//! spawned sub-agent does not write to a `ProtocolSink`. Its `OutputSink` is a
//! per-child `ChannelSink`, which relayed text VERBATIM; the parent's
//! `emit_sub_agent_event` then cloned that JSON onto the wire without
//! inspecting it. `main.rs` hardcodes `.with_sub_agent_traces(true)`, so every
//! Desktop session carried a literal `<think>…</think>` at
//! `sub_agent_event.inner.text`.
//!
//! The spec (§1.3) promises UNCONDITIONALLY that `text` never contains inline
//! reasoning tags and tells clients not to implement their own stripper, so
//! this path is in scope of that contract.
//!
//! These tests drive the REAL chain — `ChannelSink` → mpsc → the real
//! `ProtocolSink::emit_sub_agent_event` → real `serde_json` serialization —
//! and assert on the wire lines a host actually reads.

use std::sync::{Arc, Mutex};

use wcore_agent::agents::channel_sink::{CHANNEL_CAPACITY, ChannelSink, SubAgentRelay};
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_tools::ToolOutputSink;
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
    /// Every `sub_agent_event` frame's `inner`, parsed.
    fn inners(&self) -> Vec<serde_json::Value> {
        self.lines()
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["type"] == "sub_agent_event")
            .map(|v| v["inner"].clone())
            .collect()
    }
    /// Concatenated `text` of every relayed `text_delta`.
    fn relayed_visible(&self) -> String {
        self.inners()
            .iter()
            .filter(|v| v["type"] == "text_delta")
            .map(|v| v["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }
    /// Concatenated `text` of every relayed `thinking`.
    fn relayed_thinking(&self) -> String {
        self.inners()
            .iter()
            .filter(|v| v["type"] == "thinking")
            .map(|v| v["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

/// Drive one spawned child's text stream through the real relay chain.
///
/// `chunks` are the child engine's `emit_text_delta` calls; `tool_chunks` are
/// the child's streaming tool output (`ToolOutputSink::emit_chunk`), delivered
/// after the model text so both producers are exercised on one child.
fn drive(chunks: &[&str], tool_chunks: &[&str]) -> Arc<WireRecorder> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    rt.block_on(async move {
        let rec = Arc::new(WireRecorder::default());
        let parent: Arc<dyn OutputSink> = Arc::new(
            ProtocolSink::with_emitter(rec.clone() as Arc<dyn ProtocolEmitter>)
                .with_sub_agent_traces(true),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);
        let drain_parent = Arc::clone(&parent);
        let drain = tokio::spawn(async move {
            while let Some(relay) = rx.recv().await {
                drain_parent.emit_sub_agent_event(
                    &relay.parent_call_id,
                    &relay.agent_name,
                    &relay.inner,
                );
            }
        });

        let sink = ChannelSink::new("spawn:0:worker".into(), "worker".into(), tx);
        sink.emit_stream_start("c-1");
        for c in chunks {
            sink.emit_text_delta(c, "c-1");
        }
        for c in tool_chunks {
            sink.emit_chunk(c);
        }
        sink.emit_stream_end("c-1", 1, 0, 0, 0, 0, FinishReason::Stop);
        drop(sink);
        let _ = drain.await;
        rec
    })
}

/// Any reasoning tag, in any spelling, that a host would see verbatim.
fn tag_leak(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    for name in ["think", "thinking", "reasoning", "thought"] {
        for form in [format!("<{name}"), format!("</{name}")] {
            if lower.contains(&form) {
                return Some(form);
            }
        }
    }
    None
}

/// THE PRODUCTION GAP: a spawned R1/Qwen-class child's model text.
#[test]
fn no_reasoning_tag_reaches_sub_agent_text_delta() {
    let cases: &[(&str, &[&str], &str, &str)] = &[
        (
            "plain think",
            &["Hello <think>secret</think> world"],
            "Hello  world",
            "secret",
        ),
        (
            "thinking",
            &["<thinking>plan</thinking>visible"],
            "visible",
            "plan",
        ),
        (
            "reasoning",
            &["<reasoning>why</reasoning>visible"],
            "visible",
            "why",
        ),
        (
            "uppercase",
            &["<THINK>loud</think>visible"],
            "visible",
            "loud",
        ),
        (
            "attributes",
            &["<thinking budget=\"8k\">b</thinking>ok"],
            "ok",
            "b",
        ),
        // Tag straddling a chunk boundary — the case a naive per-chunk
        // string replace cannot catch.
        (
            "split open tag",
            &["Hi <thi", "nk>secret</think> bye"],
            "Hi  bye",
            "secret",
        ),
        (
            "split close tag",
            &["<think>secret</thi", "nk>bye"],
            "bye",
            "secret",
        ),
        // Unclosed block: the filter eats to end of stream. wayland#1242
        // moved this case to the TEXT lane — a block that never closed was
        // never a block, so it is recovered verbatim and NOT reported as
        // reasoning, which is what the engine already stored for it
        // (wayland#1222). Covered by
        // `an_unclosed_block_is_relayed_as_text_not_as_thinking` below, which
        // is the only case where a tag legitimately appears in the relayed
        // text, so it cannot live in this table's leak check.
        // The ticket asked for every spelling, `thought` included.
        (
            "thought",
            &["<thought>musing</thought>visible"],
            "visible",
            "musing",
        ),
    ];

    let mut failures = Vec::new();
    for (label, chunks, expect_visible, expect_thinking) in cases {
        let rec = drive(chunks, &[]);
        rec.dump(label);
        let visible = rec.relayed_visible();
        let thinking = rec.relayed_thinking();
        if let Some(tag) = tag_leak(&visible) {
            failures.push(format!(
                "[{label}] reasoning tag `{tag}` reached sub_agent_event.inner.text: {visible:?}"
            ));
            continue;
        }
        if visible != *expect_visible {
            failures.push(format!(
                "[{label}] visible text {visible:?} != expected {expect_visible:?}"
            ));
        }
        if !thinking.contains(expect_thinking) {
            failures.push(format!(
                "[{label}] reasoning body {expect_thinking:?} was DELETED, not relayed as thinking (thinking={thinking:?})"
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// wayland#1242 — the one case where a reasoning tag legitimately reaches the
/// relayed text, and why.
///
/// An unclosed `<think>` is not a reasoning block; it is prose that contains a
/// tag-shaped word and a stream that ended before anything could close it.
/// wayland#1222 settled that for the durable record — recover it verbatim,
/// retract it from the capture — and wayland#1242 makes the relay agree, so a
/// parent host is shown the same answer the child stored. The alternative,
/// keeping the strip on the relay only, leaves the two disagreeing about the
/// same turn with no way for either side to tell.
///
/// The body ALSO still arrives as `thinking`. A relay drains its capture
/// buffer after every chunk so a child's reasoning streams live (#1129), so
/// those bytes are on the wire long before the stream ends and the block is
/// known never to have closed; `finish`'s retraction reaches only what is
/// still buffered. Duplicated is the honest end state; truncated was the one
/// that cost the user the answer.
#[test]
fn an_unclosed_block_is_relayed_as_text() {
    let rec = drive(&["visible<think>dangling"], &[]);
    rec.dump("unclosed");
    assert_eq!(
        rec.relayed_visible(),
        "visible<think>dangling",
        "the tail of the answer was withheld from the host"
    );
    assert_eq!(rec.relayed_thinking(), "dangling");
}

/// The second relay producer: streaming tool output, which `ChannelSink`
/// deliberately maps onto the same `text_delta` wire shape.
#[test]
fn no_reasoning_tag_reaches_sub_agent_tool_chunk_relay() {
    let rec = drive(&[], &["out <think>hidden</think> done"]);
    rec.dump("tool chunk");
    let visible = rec.relayed_visible();
    assert!(
        tag_leak(&visible).is_none(),
        "reasoning tag reached the relayed tool-output text_delta: {visible:?}"
    );
    assert_eq!(visible, "out  done");
    assert!(
        rec.relayed_thinking().contains("hidden"),
        "tool-chunk reasoning body was deleted rather than relayed: {:?}",
        rec.relayed_thinking()
    );
}

/// Guard against over-stripping: prose that merely CONTAINS the words, and
/// non-reasoning tags whose names merely start like one, must survive byte
/// for byte. Also proves the two lanes do not corrupt each other's state.
#[test]
fn legitimate_text_is_not_corrupted_or_double_stripped() {
    let prose = "I thought about thinking, and my reasoning was <thoughtful> \
                 but <b>bold</b> and 3 < 4 > 2. <thinker>x</thinker>";
    let rec = drive(&[prose], &["tool says: I thought so <b>ok</b>"]);
    rec.dump("no corruption");
    let visible = rec.relayed_visible();
    assert_eq!(
        visible,
        format!("{prose}tool says: I thought so <b>ok</b>"),
        "legitimate text was corrupted by the reasoning filter"
    );
    assert_eq!(
        rec.relayed_thinking(),
        "",
        "no thinking should be produced from text with no reasoning block"
    );
}

/// Interleaving the two lanes must not let a tool chunk's `<` swallow the
/// model's answer, and must not let model reasoning swallow tool output.
#[test]
fn lanes_do_not_contaminate_each_other() {
    let rec = drive(&["answer"], &["file contains <think>literal"]);
    rec.dump("interleave");
    let visible = rec.relayed_visible();
    assert!(
        visible.starts_with("answer"),
        "model answer lost: {visible:?}"
    );
    // The tool lane's `<think>` never closes, so wayland#1242 recovers it onto
    // that lane's own text — verbatim, and only there. The property under test
    // is that the two state machines stay separate: the model lane's answer is
    // whole, and the tool lane's tail is whole, with neither having eaten the
    // other.
    assert_eq!(
        visible, "answerfile contains <think>literal",
        "one lane's state machine consumed the other's text: {visible:?}"
    );
    assert_eq!(
        rec.relayed_thinking(),
        "literal",
        "the tool lane's own open block, streamed live before it could be \
         known never to close"
    );
}
