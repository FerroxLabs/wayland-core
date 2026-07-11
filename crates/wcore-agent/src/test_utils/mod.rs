// W7 Pre-flight 0.0d: test-driver helpers.
//
// Gated behind the `test-utils` Cargo feature so it never ships in
// release binaries. Provides three primitives that the W7 (and future)
// integration tests need to drive an `AgentEngine` end-to-end without
// hitting a real LLM provider:
//
// 1. `ScriptedProvider` — `LlmProvider` impl that replays a configurable
//    `Vec<LlmEvent>` on every `stream()` call.
// 2. `TestSink` — `OutputSink` impl that records every emission as a
//    typed `ProtocolEvent` into an internal `Mutex<Vec<ProtocolEvent>>`.
// 3. `SyntheticTurnOutput` — bundle returned by
//    `AgentEngine::run_synthetic_turn`: final text, captured events, turns.
//
// And companion entry points on the engine + bootstrap (defined alongside
// their host structs):
// - `AgentBootstrap::build_for_test(config) -> AgentEngine`
// - `AgentEngine::run_synthetic_turn(input: &str) -> SyntheticTurnOutput`
// - `AgentEngine::captured_protocol_events(&self) -> Vec<ProtocolEvent>`
//
// The captured-events accessor is wired through `TestSinkHandle`, which
// `build_for_test` installs on the engine. Engines constructed by
// production paths return an empty Vec (the handle defaults to a
// detached buffer).

// W7 (debt B.8): high-level fixture-builder DSL that composes the
// primitives in this module into a one-line setup for end-to-end tests.
// See `e2e_fixture` for usage.
pub mod e2e_fixture;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use wcore_protocol::events::{ErrorInfo, ProtocolEvent, Usage};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::FinishReason;

use crate::output::OutputSink;

/// W7 Pre-0: events captured by `TestSink` are stored as serialised
/// JSON values (the protocol wire form) because `ProtocolEvent` is
/// `Debug + Serialize` only — no `Clone` impl available — and we don't
/// want to drive-by-add one across crate boundaries for a test helper.
pub type CapturedEvent = Value;

/// W7 Pre-0: scripted `LlmProvider` for tests. Emits a fixed `Vec<LlmEvent>`
/// on each `stream()` call. Useful for driving the engine through a known
/// sequence (e.g. a single TextDelta followed by Done).
pub struct ScriptedProvider {
    events: Mutex<Vec<LlmEvent>>,
}

impl ScriptedProvider {
    /// Construct with a script that's replayed on every `stream()` call.
    /// The script is cloned per-call so multiple turns see the same events.
    pub fn new(events: Vec<LlmEvent>) -> Self {
        Self {
            events: Mutex::new(events),
        }
    }

    /// Minimal one-turn script: a single text delta then a clean Done.
    pub fn single_text_turn(text: impl Into<String>) -> Self {
        Self::new(vec![
            LlmEvent::TextDelta(text.into()),
            LlmEvent::Done {
                stop_reason: wcore_types::message::StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: wcore_types::message::TokenUsage::default(),
            },
        ])
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        // SAFETY: this entire module is gated behind the `test-utils`
        // feature flag (see crate header) and never ships in release
        // binaries. Tests are allowed to use `.expect()` since a
        // poisoned mutex here would indicate a test bug, not a
        // production failure mode.
        let snapshot = self.events.lock().expect("ScriptedProvider mutex").clone();
        let (tx, rx) = mpsc::channel(snapshot.len().max(1));
        tokio::spawn(async move {
            for ev in snapshot {
                if tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

/// W7 Pre-0: `OutputSink` that records every emission as a
/// JSON-serialised `ProtocolEvent` into a shared `Vec`. Mirrors
/// `ProtocolSink`'s type→event mapping for the methods the engine drives.
#[derive(Default)]
pub struct TestSink {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl TestSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clone of the event-buffer handle. The new handle shares the same
    /// `Vec` so callers that pass `Arc::new(sink)` to the engine can
    /// still observe captured events.
    pub fn handle(&self) -> TestSinkHandle {
        TestSinkHandle {
            events: self.events.clone(),
        }
    }

    fn record(&self, event: &ProtocolEvent) {
        let value = serde_json::to_value(event).unwrap_or_else(|err| {
            serde_json::json!({
                "type": "test_sink_serialize_failure",
                "error": err.to_string(),
            })
        });
        if let Ok(mut guard) = self.events.lock() {
            guard.push(value);
        }
    }
}

/// W7 Pre-0: read handle on a `TestSink`'s captured-events buffer.
/// Held by the engine so `captured_protocol_events()` can return a
/// snapshot without consuming the buffer.
#[derive(Clone, Default)]
pub struct TestSinkHandle {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl TestSinkHandle {
    pub fn snapshot(&self) -> Vec<CapturedEvent> {
        self.events.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl OutputSink for TestSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.record(&ProtocolEvent::TextDelta {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
        });
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.record(&ProtocolEvent::Thinking {
            text: text.to_string(),
            msg_id: msg_id.to_string(),
            subject: None,
        });
    }
    fn emit_thinking_subject(&self, subject: &str, msg_id: &str) {
        self.record(&ProtocolEvent::Thinking {
            text: String::new(),
            msg_id: msg_id.to_string(),
            subject: Some(subject.to_string()),
        });
    }
    fn emit_tool_call(&self, name: &str, _input: &str) {
        // Mirror ProtocolSink fallback: route through Info.
        self.record(&ProtocolEvent::Info {
            msg_id: String::new(),
            message: format!("Tool call: {name}"),
        });
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        let status = if is_error { "error" } else { "success" };
        self.record(&ProtocolEvent::Info {
            msg_id: String::new(),
            message: format!("[{name} {status}] {content}"),
        });
    }
    fn emit_stream_start(&self, msg_id: &str) {
        self.record(&ProtocolEvent::StreamStart {
            msg_id: msg_id.to_string(),
        });
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        _turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
    ) {
        self.record(&ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            finish_reason,
            usage: Some(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: if cache_read_tokens > 0 {
                    Some(cache_read_tokens)
                } else {
                    None
                },
                cache_write_tokens: if cache_creation_tokens > 0 {
                    Some(cache_creation_tokens)
                } else {
                    None
                },
                active_window_percent: None,
            }),
            usage_delta: None,
            agent_run_id: None,
        });
    }
    #[allow(clippy::too_many_arguments)]
    fn emit_stream_end_full(
        &self,
        msg_id: &str,
        _turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
        active_window_percent: Option<u32>,
        agent_run_id: Option<&str>,
        usage_delta: Option<&wcore_types::message::TokenUsage>,
    ) {
        // CORE-2: mirror ProtocolSink's enriched mapping (gauge, run id,
        // per-run delta) so tests can assert the terminal stream_end
        // carries the delta the same way the real wire does.
        self.record(&ProtocolEvent::StreamEnd {
            msg_id: msg_id.to_string(),
            finish_reason,
            usage: Some(Usage {
                input_tokens,
                output_tokens,
                cache_read_tokens: if cache_read_tokens > 0 {
                    Some(cache_read_tokens)
                } else {
                    None
                },
                cache_write_tokens: if cache_creation_tokens > 0 {
                    Some(cache_creation_tokens)
                } else {
                    None
                },
                active_window_percent,
            }),
            usage_delta: usage_delta.map(|d| Usage {
                input_tokens: d.input_tokens,
                output_tokens: d.output_tokens,
                cache_read_tokens: if d.cache_read_tokens > 0 {
                    Some(d.cache_read_tokens)
                } else {
                    None
                },
                cache_write_tokens: if d.cache_creation_tokens > 0 {
                    Some(d.cache_creation_tokens)
                } else {
                    None
                },
                active_window_percent: None,
            }),
            agent_run_id: agent_run_id.map(str::to_string),
        });
    }
    fn emit_error(&self, msg: &str, retryable: bool) {
        self.record(&ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "test_sink_error".to_string(),
                message: msg.to_string(),
                retryable,
            },
        });
    }
    fn emit_info(&self, msg: &str) {
        self.record(&ProtocolEvent::Info {
            msg_id: String::new(),
            message: msg.to_string(),
        });
    }
    /// W9.1 T3 (T10b): record `TraceEvent` so tests can observe both the
    /// W1 per-turn trace and the W9.1 `skill_drafted` payload that rides
    /// inside the same envelope. Production `ProtocolSink` gates this on
    /// `with_structured_traces(true)`; `TestSink` records
    /// unconditionally so test assertions can read whichever payload
    /// shape they're driving.
    fn emit_trace(&self, msg_id: &str, trace_json: &Value) {
        self.record(&ProtocolEvent::TraceEvent {
            msg_id: msg_id.to_string(),
            trace: trace_json.clone(),
        });
    }
}

/// W7 Pre-0: bundle returned by `AgentEngine::run_synthetic_turn`.
#[derive(Debug)]
pub struct SyntheticTurnOutput {
    /// Final text emitted by the engine on this turn.
    pub final_text: String,
    /// Captured protocol events from the TestSink, as serialised JSON.
    pub events: Vec<CapturedEvent>,
    /// Turn count returned by `engine.run()`.
    pub turns: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scripted_provider_replays_script_per_call() {
        let p = ScriptedProvider::single_text_turn("hello");
        let req = LlmRequest {
            model: "test".into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
        };
        let mut rx = p.stream(&req).await.unwrap();
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, LlmEvent::TextDelta(s) if s == "hello"));
        let second = rx.recv().await.unwrap();
        assert!(matches!(second, LlmEvent::Done { .. }));
    }

    #[test]
    fn test_sink_captures_emissions() {
        let sink = TestSink::new();
        let handle = sink.handle();
        sink.emit_text_delta("hi", "m-1");
        sink.emit_info("note");
        let snap = handle.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0]["type"], "text_delta");
        assert_eq!(snap[0]["text"], "hi");
        assert_eq!(snap[0]["msg_id"], "m-1");
        assert_eq!(snap[1]["type"], "info");
        assert_eq!(snap[1]["message"], "note");
    }
}
