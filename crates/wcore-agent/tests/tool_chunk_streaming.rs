//! W7 F4-3 integration test: streaming Bash chunks flow through the
//! orchestration dispatcher's streaming branch and arrive at the
//! parent OutputSink as `tool_chunk` events.

#![cfg(unix)] // BashTool::execute_streaming streams via printf, Unix-only

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wcore_agent::confirm::ToolConfirmer;
use wcore_agent::orchestration::{StreamingContext, execute_tool_calls_with_streaming};
use wcore_agent::output::OutputSink;
use wcore_types::message::{ContentBlock, FinishReason};

/// (msg_id, call_id, tool_name, chunk) — captured by the recording sinks.
type Captured = (String, String, String, String);
type CapturedBuf = Arc<Mutex<Vec<Captured>>>;

#[derive(Default)]
struct CapSink {
    chunks: Mutex<Vec<Captured>>, // msg_id, call_id, tool_name, chunk
    streaming_on: bool,
}

impl OutputSink for CapSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, _: &str) {}
    fn emit_tool_chunk(&self, msg_id: &str, call_id: &str, tool_name: &str, chunk: &str) {
        self.chunks.lock().unwrap().push((
            msg_id.into(),
            call_id.into(),
            tool_name.into(),
            chunk.into(),
        ));
    }
    fn streaming_tools_advertised(&self) -> bool {
        self.streaming_on
    }
}

fn make_registry() -> wcore_tools::registry::ToolRegistry {
    let mut reg = wcore_tools::registry::ToolRegistry::new();
    reg.set_sandbox_runtime(Arc::new(wcore_sandbox::SandboxRegistry::new(Arc::new(
        wcore_sandbox::backends::no_sandbox::NoSandboxBackend::new(),
    ))));
    reg.register(Box::new(wcore_tools::bash::BashTool));
    reg
}

fn bash_call(id: &str, command: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: "Bash".into(),
        input: json!({"command": command}),
        extra: None,
    }
}

#[tokio::test]
async fn bash_emits_tool_chunks_when_streaming_advertised() {
    let registry = make_registry();
    let confirmer = Arc::new(std::sync::Mutex::new(ToolConfirmer::new(true, vec![])));
    let sink: Arc<dyn OutputSink> = Arc::new(CapSink {
        chunks: Mutex::new(Vec::new()),
        streaming_on: true,
    });

    let calls = vec![bash_call("c-1", "printf 'line1\\nline2\\nline3\\n'")];
    let _ = execute_tool_calls_with_streaming(
        &registry,
        &calls,
        &confirmer,
        None,
        wcore_compact::CompactionLevel::default(),
        false,
        Some(StreamingContext {
            output: Arc::clone(&sink),
            msg_id: "m-1".into(),
        }),
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch should succeed");

    // Reach into the concrete CapSink via unsafe? Better: build the
    // recording sink directly so we can read .chunks via the Arc.
    // The trait object hides the inner Vec, so we wire a second handle
    // pattern: pre-construct a recording sink, hand its Arc to both the
    // dispatcher and the test scope.
    let _ = sink; // tests below use a recording-sink-with-handle pattern.
}

/// Helper: a CapSink with an external handle on its `chunks` Vec so the
/// test scope can inspect captured chunks without downcasting.
struct CapSinkShared {
    chunks: CapturedBuf,
    streaming_on: bool,
}

impl OutputSink for CapSinkShared {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, _: &str) {}
    fn emit_tool_chunk(&self, msg_id: &str, call_id: &str, tool_name: &str, chunk: &str) {
        self.chunks.lock().unwrap().push((
            msg_id.into(),
            call_id.into(),
            tool_name.into(),
            chunk.into(),
        ));
    }
    fn streaming_tools_advertised(&self) -> bool {
        self.streaming_on
    }
}

#[tokio::test]
async fn bash_streams_through_dispatcher_when_advertised_on() {
    let registry = make_registry();
    let confirmer = Arc::new(std::sync::Mutex::new(ToolConfirmer::new(true, vec![])));
    let chunks: CapturedBuf = Arc::new(Mutex::new(Vec::new()));
    let sink: Arc<dyn OutputSink> = Arc::new(CapSinkShared {
        chunks: Arc::clone(&chunks),
        streaming_on: true,
    });

    let calls = vec![bash_call("c-1", "printf 'A\\nB\\nC\\n'")];
    let _ = execute_tool_calls_with_streaming(
        &registry,
        &calls,
        &confirmer,
        None,
        wcore_compact::CompactionLevel::default(),
        false,
        Some(StreamingContext {
            output: Arc::clone(&sink),
            msg_id: "m-1".into(),
        }),
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();

    let captured = chunks.lock().unwrap();
    assert!(
        !captured.is_empty(),
        "streaming should emit at least one tool_chunk; got {captured:?}"
    );
    for (msg_id, call_id, tool_name, _chunk) in captured.iter() {
        assert_eq!(msg_id, "m-1");
        assert_eq!(call_id, "c-1");
        assert_eq!(tool_name, "Bash");
    }
    // Stdout contained "A", "B", "C" — chunks should reflect at least 3 lines.
    let chunk_texts: Vec<&String> = captured.iter().map(|(_, _, _, c)| c).collect();
    let combined = chunk_texts.iter().fold(String::new(), |mut acc, s| {
        acc.push_str(s);
        acc.push(' ');
        acc
    });
    assert!(
        combined.contains('A') && combined.contains('B') && combined.contains('C'),
        "expected A/B/C in chunks; got {combined:?}"
    );
}

#[tokio::test]
async fn bash_does_not_stream_when_advertised_off() {
    let registry = make_registry();
    let confirmer = Arc::new(std::sync::Mutex::new(ToolConfirmer::new(true, vec![])));
    let chunks: CapturedBuf = Arc::new(Mutex::new(Vec::new()));
    let sink: Arc<dyn OutputSink> = Arc::new(CapSinkShared {
        chunks: Arc::clone(&chunks),
        streaming_on: false, // <-- advertise off
    });

    let calls = vec![bash_call("c-2", "printf 'no_stream\\n'")];
    let _ = execute_tool_calls_with_streaming(
        &registry,
        &calls,
        &confirmer,
        None,
        wcore_compact::CompactionLevel::default(),
        false,
        Some(StreamingContext {
            output: Arc::clone(&sink),
            msg_id: "m-2".into(),
        }),
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();

    let captured = chunks.lock().unwrap();
    assert!(
        captured.is_empty(),
        "streaming OFF: dispatcher should not emit any tool_chunk; got {captured:?}"
    );
}

#[tokio::test]
async fn streaming_none_falls_back_to_buffered_execute() {
    // No streaming context at all — must run through the legacy
    // buffered execute() path. Nothing to assert on chunks (sink omitted),
    // just verify the call completes without panicking.
    let registry = make_registry();
    let confirmer = Arc::new(std::sync::Mutex::new(ToolConfirmer::new(true, vec![])));
    let calls = vec![bash_call("c-3", "printf 'legacy\\n'")];
    let outcome = execute_tool_calls_with_streaming(
        &registry,
        &calls,
        &confirmer,
        None,
        wcore_compact::CompactionLevel::default(),
        false,
        None, // <-- no streaming context
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome.results.len(), 1);
}

#[allow(dead_code)]
fn _ensure_value_in_scope(_v: Value) {}
