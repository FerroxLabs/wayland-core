//! `MockLlm` — a scriptable Anthropic-shaped mock LLM for the TUI E2E harness.
//!
//! # Why this exists
//!
//! The end-to-end TUI harness ([`harness_tui_flow.rs`]) drives the real
//! `wayland-core` binary through a PTY, but the binary never makes a network
//! call until the user sends a prompt — and a real prompt would hit a live
//! provider. To assert on the *agent loop* (text turns, tool calls, multi-turn
//! conversations) we need a deterministic backend the real provider can talk
//! to. The Anthropic provider POSTs to `{base_url}/v1/messages`
//! (`wcore_providers::anthropic::stream`, anthropic.rs:176), so pointing
//! `base_url` at a local mock that speaks the Anthropic SSE wire format lets us
//! drive the *real* provider and parser without ever leaving the machine.
//!
//! # What it guarantees
//!
//! Every byte this builder emits is fed, in the `self_tests` module below,
//! through the **real** `wcore_providers::anthropic_shared::parse_sse_data`
//! parser. The parser treats a stream that ends without a `message_delta`
//! carrying a `stop_reason` as *truncated* — which makes the engine retry. The
//! self-test asserts that every scripted response produces a clean, terminal
//! `Done` event and is never classified as truncated. This keeps the mock from
//! silently drifting away from what the production parser accepts.
//!
//! # Usage
//!
//! ```ignore
//! let mock = MockLlm::new()
//!     .text("Hello from the mock!")               // turn 1: plain text
//!     .tool_use("Read", json!({"file_path": "x"})) // turn 2: a tool call
//!     .text("Done.");                              // turn 3: plain text
//!
//! let server = mock.start().await; // bind 127.0.0.1:0, mount /v1/messages
//! let base_url = server.uri();     // feed this to the real provider / binary
//! ```
//!
//! Each POST to `/v1/messages` pops the next scripted response (in order). When
//! the queue is exhausted the server replies with the last response so an
//! over-eager agent loop degrades gracefully instead of 404-ing.

#![allow(dead_code)] // Foundation module: not every helper is used by every test yet.

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers};

/// One scripted assistant turn.
#[derive(Clone, Debug)]
pub enum Turn {
    /// A plain-text response. The string is emitted as a single
    /// `text_delta` inside one `text` content block.
    Text(String),
    /// A tool call. Emitted as a `tool_use` content block whose accumulated
    /// `input_json_delta` deltas reconstruct `input`, terminated with a
    /// `stop_reason: "tool_use"` message_delta.
    ToolUse { name: String, input: Value },
    /// A transient HTTP error status (no SSE body) — drives the real
    /// provider's retry path. A 5xx/408 is retried inline by
    /// `wcore_providers::retry::builder_send_with_retry`, so scripting one
    /// before a `Text`/`ToolUse` turn exercises "fail then succeed" end to
    /// end. (429 is surfaced as `RateLimited` rather than retried inline.)
    HttpError(u16),
    /// A plain-text response whose HTTP reply is HELD for `delay_ms` before
    /// any bytes are sent. The engine emits `StreamStart` at turn-submission
    /// (before the provider call), so the turn is "in flight" — and
    /// interruptible — for the whole delay. Used to drive the Ctrl-C / ESC
    /// mid-stream cancellation journey deterministically without a real
    /// provider.
    SlowText { text: String, delay_ms: u64 },
}

impl Turn {
    /// Render this turn as a complete Anthropic SSE response body.
    ///
    /// The byte sequence always carries a terminal `message_delta`
    /// (`stop_reason`) + `message_stop`, so the real parser never flags it as
    /// truncated.
    pub fn to_sse(&self) -> String {
        match self {
            Turn::Text(text) => text_turn_sse(text),
            Turn::ToolUse { name, input } => tool_use_turn_sse(name, input),
            // Error turns carry no SSE body; `reply()` maps them to a non-200
            // status. Never fed through the conformance gate (which only
            // scripts Text/ToolUse), so an empty body here is unreachable there.
            Turn::HttpError(_) => String::new(),
            // A slow turn is a normal text turn whose HTTP reply is just held
            // back; the body is identical to a `Text` turn.
            Turn::SlowText { text, .. } => text_turn_sse(text),
        }
    }

    /// The (status, body, content-type, delay) this turn replies with. Success
    /// turns are `200 text/event-stream`; an [`Turn::HttpError`] is the scripted
    /// status with a minimal Anthropic-shaped error JSON body; a
    /// [`Turn::SlowText`] is a 200 text turn whose reply is delayed.
    fn reply(&self) -> (u16, String, &'static str, std::time::Duration) {
        let zero = std::time::Duration::ZERO;
        match self {
            Turn::Text(_) | Turn::ToolUse { .. } => (200, self.to_sse(), "text/event-stream", zero),
            Turn::HttpError(code) => (
                *code,
                format!(
                    "{{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\",\
                     \"message\":\"mock transient {code}\"}}}}"
                ),
                "application/json",
                zero,
            ),
            Turn::SlowText { text, delay_ms } => (
                200,
                text_turn_sse(text),
                "text/event-stream",
                std::time::Duration::from_millis(*delay_ms),
            ),
        }
    }
}

/// A scriptable, multi-turn Anthropic mock.
#[derive(Default, Clone)]
pub struct MockLlm {
    turns: Vec<Turn>,
}

impl MockLlm {
    /// Start an empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a plain-text response turn.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.turns.push(Turn::Text(text.into()));
        self
    }

    /// Queue a `tool_use` response turn with a name and JSON input.
    pub fn tool_use(mut self, name: impl Into<String>, input: Value) -> Self {
        self.turns.push(Turn::ToolUse {
            name: name.into(),
            input,
        });
        self
    }

    /// Queue a transient HTTP error turn (e.g. `503`). The next POST gets this
    /// status; because the real provider retries 5xx/408 inline, the POST after
    /// it pops the following turn — so `.http_error(503).text("ok")` models a
    /// server hiccup that recovers on retry.
    pub fn http_error(mut self, status: u16) -> Self {
        self.turns.push(Turn::HttpError(status));
        self
    }

    /// Queue a text turn whose HTTP reply is HELD for `delay_ms` before any
    /// bytes are sent. The turn is in-flight (and interruptible via ESC /
    /// Ctrl-C) for the whole delay — use it to drive the cancel-mid-stream
    /// journey deterministically.
    pub fn slow_text(mut self, text: impl Into<String>, delay_ms: u64) -> Self {
        self.turns.push(Turn::SlowText {
            text: text.into(),
            delay_ms,
        });
        self
    }

    /// The scripted turns, in order.
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    /// Render every queued turn as its SSE body (in order). Handy for the
    /// in-process conformance gate, which never needs a live socket.
    pub fn sse_bodies(&self) -> Vec<String> {
        self.turns.iter().map(Turn::to_sse).collect()
    }

    /// Bind a local HTTP server on `127.0.0.1:0` and mount the Anthropic
    /// `/v1/messages` endpoint. Each POST pops the next scripted turn; once
    /// the queue drains, the final turn is replayed so an extra request never
    /// 404s. Falls back to a minimal `end_turn` text turn when the script is
    /// empty.
    ///
    /// Returns the started [`MockServer`]; `.uri()` is the `base_url` to feed
    /// the real provider.
    pub async fn start(&self) -> MockServer {
        let server = MockServer::start().await;

        // Each reply is a (status, body, content-type, delay) tuple so error
        // turns can return a non-200 (exercising the provider's retry path),
        // success turns return a 200 SSE stream, and slow turns hold the reply
        // back (exercising mid-stream cancellation).
        let replies: Vec<(u16, String, &'static str, std::time::Duration)> =
            if self.turns.is_empty() {
                vec![(
                    200,
                    text_turn_sse("ok"),
                    "text/event-stream",
                    std::time::Duration::ZERO,
                )]
            } else {
                self.turns.iter().map(Turn::reply).collect()
            };
        let replies = Arc::new(replies);
        let cursor = Arc::new(Mutex::new(0usize));

        let responder = move |_req: &Request| {
            let mut idx = cursor.lock().expect("mock_llm cursor lock");
            let reply = if *idx < replies.len() {
                let r = replies[*idx].clone();
                *idx += 1;
                r
            } else {
                // Queue exhausted — replay the last scripted turn.
                replies[replies.len() - 1].clone()
            };
            let (status, body, content_type, delay) = reply;
            let mut tmpl = ResponseTemplate::new(status).set_body_raw(body, content_type);
            if !delay.is_zero() {
                tmpl = tmpl.set_delay(delay);
            }
            tmpl
        };

        Mock::given(matchers::method("POST"))
            .and(matchers::path("/v1/messages"))
            .respond_with(responder)
            .mount(&server)
            .await;

        server
    }
}

// ---------------------------------------------------------------------------
// Request recorder — read back what the binary actually sent.
//
// `wiremock::MockServer` records every received request by default
// (`MockServer::received_requests`). For the P0 smoke harness we need to assert
// on the OUTGOING request the spawned binary made — its `model`, `system`, and
// the `x-api-key` auth header — to prove that an onboarding/config change
// reached the LIVE engine (not just disk). The mock's *responses* are scripted
// by the builder above; this recorder reads the *requests* back after the fact.
// ---------------------------------------------------------------------------

/// One recorded POST to `/v1/messages`: the parsed JSON body plus the
/// `x-api-key` header value the provider attached. Everything a rebind-class
/// smoke check needs to assert "the request that actually went out".
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// The request body parsed as JSON (the Anthropic message payload). The
    /// `model` field is at `["model"]`, the system prompt at `["system"]`.
    pub body: Value,
    /// The `x-api-key` header the provider sent, if present. Used to assert the
    /// entered/onboarded key reached the live engine.
    pub api_key: Option<String>,
}

impl RecordedRequest {
    /// The `model` field of the outgoing request, if it is a string.
    pub fn model(&self) -> Option<&str> {
        self.body.get("model").and_then(Value::as_str)
    }
}

/// Read back every request a started [`MockServer`] received, parsing each body
/// as JSON and lifting the `x-api-key` header. Requests whose body is not valid
/// JSON are skipped (the smoke checks only ever drive JSON message POSTs).
///
/// Async because `wiremock::MockServer::received_requests` is async; call it
/// from inside the same runtime the server was started on
/// (`rt.block_on(received_requests(&server))`).
pub async fn received_requests(server: &MockServer) -> Vec<RecordedRequest> {
    let raw = server.received_requests().await.unwrap_or_default(); // None only when recording was disabled; it isn't.
    raw.into_iter()
        .filter_map(|req| {
            let body = serde_json::from_slice::<Value>(&req.body).ok()?;
            let api_key = req
                .headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            Some(RecordedRequest { body, api_key })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SSE fixture builders — the canonical building blocks every scenario reuses.
//
// These match the exact event grammar `wcore_providers::anthropic_shared`
// parses: `event: <type>\n` lines followed by `data: <json>\n`, events
// separated by a blank line (`\n\n`). The terminal `message_delta` carries the
// `stop_reason`; without it the parser flags the stream as truncated.
// ---------------------------------------------------------------------------

/// A correct, non-truncated **text turn**: message_start → one text block with
/// a text_delta → content_block_stop → message_delta(stop_reason=end_turn) →
/// message_stop.
pub fn text_turn_sse(text: &str) -> String {
    // Encode the text safely through serde so quotes / newlines / unicode in
    // the payload never break the SSE JSON framing.
    let delta = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": { "type": "text_delta", "text": text }
    });
    format!(
        "event: message_start\n\
         data: {message_start}\n\n\
         event: content_block_start\n\
         data: {block_start}\n\n\
         event: content_block_delta\n\
         data: {delta}\n\n\
         event: content_block_stop\n\
         data: {block_stop}\n\n\
         event: message_delta\n\
         data: {message_delta}\n\n\
         event: message_stop\n\
         data: {message_stop}\n\n",
        message_start = json!({
            "type": "message_start",
            "message": {
                "id": "msg_mock_text",
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-mock",
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": { "input_tokens": 10, "output_tokens": 1 }
            }
        }),
        block_start = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        delta = delta,
        block_stop = json!({ "type": "content_block_stop", "index": 0 }),
        message_delta = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn", "stop_sequence": Value::Null },
            "usage": { "output_tokens": 20 }
        }),
        message_stop = json!({ "type": "message_stop" }),
    )
}

/// A correct, non-truncated **tool_use turn**: message_start →
/// content_block_start(tool_use) → input_json_delta deltas streaming the input
/// JSON in chunks → content_block_stop → message_delta(stop_reason=tool_use) →
/// message_stop.
///
/// The `input` value is serialised and split into two `partial_json` deltas to
/// exercise the parser's accumulation path (it must concatenate fragments and
/// only parse the whole at `content_block_stop`).
pub fn tool_use_turn_sse(name: &str, input: &Value) -> String {
    let input_str = serde_json::to_string(input).expect("serialize tool input");
    // Split the input JSON mid-string into two fragments so the test exercises
    // multi-delta accumulation, not just a single-shot payload.
    let mid = input_str.len() / 2;
    // Respect UTF-8 boundaries when splitting.
    let split = (0..=mid)
        .rev()
        .find(|i| input_str.is_char_boundary(*i))
        .unwrap_or(0);
    let (frag_a, frag_b) = input_str.split_at(split);

    let delta_a = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": { "type": "input_json_delta", "partial_json": frag_a }
    });
    let delta_b = json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": { "type": "input_json_delta", "partial_json": frag_b }
    });

    format!(
        "event: message_start\n\
         data: {message_start}\n\n\
         event: content_block_start\n\
         data: {block_start}\n\n\
         event: content_block_delta\n\
         data: {delta_a}\n\n\
         event: content_block_delta\n\
         data: {delta_b}\n\n\
         event: content_block_stop\n\
         data: {block_stop}\n\n\
         event: message_delta\n\
         data: {message_delta}\n\n\
         event: message_stop\n\
         data: {message_stop}\n\n",
        message_start = json!({
            "type": "message_start",
            "message": {
                "id": "msg_mock_tool",
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-mock",
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": { "input_tokens": 12, "output_tokens": 1 }
            }
        }),
        block_start = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "tool_use", "id": "toolu_mock", "name": name, "input": {} }
        }),
        delta_a = delta_a,
        delta_b = delta_b,
        block_stop = json!({ "type": "content_block_stop", "index": 0 }),
        message_delta = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use", "stop_sequence": Value::Null },
            "usage": { "output_tokens": 25 }
        }),
        message_stop = json!({ "type": "message_stop" }),
    )
}

// ---------------------------------------------------------------------------
// Conformance gate — feed a scripted SSE body through the REAL parser.
// ---------------------------------------------------------------------------

/// The outcome of replaying an SSE body through the real Anthropic parser,
/// reproducing the terminal-event bookkeeping `process_sse_stream` performs.
#[derive(Debug, Default)]
pub struct ParsedStream {
    pub events: Vec<wcore_types::llm::LlmEvent>,
    /// True iff a terminal event (`Done` or `Error`) was emitted. The real
    /// `process_sse_stream` returns an error (→ engine retry) when this stays
    /// false at end-of-stream.
    pub terminal_seen: bool,
}

impl ParsedStream {
    /// The parser would treat the stream as truncated (and trigger a retry)
    /// iff no terminal event was seen. Mirrors `process_sse_stream`'s
    /// `!terminal_seen` branch (anthropic_shared.rs:332).
    pub fn is_truncated(&self) -> bool {
        !self.terminal_seen
    }

    /// The single `Done` event, if exactly one was produced.
    pub fn done(&self) -> Option<&wcore_types::llm::LlmEvent> {
        let dones: Vec<_> = self
            .events
            .iter()
            .filter(|e| matches!(e, wcore_types::llm::LlmEvent::Done { .. }))
            .collect();
        if dones.len() == 1 {
            Some(dones[0])
        } else {
            None
        }
    }
}

/// Replay a scripted Anthropic SSE byte sequence through the **real**
/// `wcore_providers::anthropic_shared::parse_sse_data`, framing the bytes the
/// same way `process_sse_stream` does (split on `\n\n`, route `event:` /
/// `data:` lines), and track whether a terminal event was seen.
///
/// This is the in-process conformance gate: it asserts the mock's fixtures are
/// exactly what the production parser accepts, with no socket, no tokio
/// runtime, and no live provider.
pub fn parse_with_real_parser(sse: &str) -> ParsedStream {
    use wcore_providers::anthropic_shared::{StreamState, parse_sse_data};
    use wcore_types::llm::LlmEvent;

    let mut state = StreamState::new();
    let mut current_event_type = String::new();
    let mut out = ParsedStream::default();

    // Frame exactly like process_sse_stream: events separated by "\n\n".
    for event_block in sse.split("\n\n") {
        if event_block.trim().is_empty() {
            continue;
        }
        for line in event_block.lines() {
            if let Some(ev_type) = line.strip_prefix("event: ") {
                current_event_type = ev_type.to_string();
            } else if let Some(data) = line.strip_prefix("data: ") {
                for event in parse_sse_data(&current_event_type, data, &mut state) {
                    if matches!(event, LlmEvent::Done { .. } | LlmEvent::Error(_)) {
                        out.terminal_seen = true;
                    }
                    out.events.push(event);
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Self-tests — the mock can never drift from the real parser.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod self_tests {
    use super::*;
    use wcore_types::llm::LlmEvent;
    use wcore_types::message::StopReason;

    /// The canonical **text turn** fixture is accepted by the real parser as a
    /// clean, non-truncated stream ending in a single `Done(end_turn)`.
    #[test]
    fn text_fixture_is_parser_conformant() {
        let parsed = parse_with_real_parser(&text_turn_sse("Hello, harness!"));

        assert!(
            !parsed.is_truncated(),
            "text fixture must NOT be flagged as truncated; events: {:?}",
            parsed.events
        );

        // Exactly one text delta carrying the payload.
        let text: Vec<&String> = parsed
            .events
            .iter()
            .filter_map(|e| match e {
                LlmEvent::TextDelta(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(text, vec![&"Hello, harness!".to_string()]);

        match parsed.done().expect("exactly one Done") {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::EndTurn);
            }
            other => panic!("expected Done(end_turn), got {other:?}"),
        }
    }

    /// The canonical **tool_use turn** fixture is accepted by the real parser:
    /// the accumulated `input_json_delta` fragments reconstruct the full input,
    /// and the stream ends in a single `Done(tool_use)` — never truncated.
    #[test]
    fn tool_use_fixture_is_parser_conformant() {
        let input = json!({ "file_path": "/tmp/x", "limit": 42 });
        let parsed = parse_with_real_parser(&tool_use_turn_sse("Read", &input));

        assert!(
            !parsed.is_truncated(),
            "tool_use fixture must NOT be flagged as truncated; events: {:?}",
            parsed.events
        );

        // Exactly one ToolUse, with the input reassembled from the split deltas.
        let tools: Vec<_> = parsed
            .events
            .iter()
            .filter(|e| matches!(e, LlmEvent::ToolUse { .. }))
            .collect();
        assert_eq!(
            tools.len(),
            1,
            "expected one ToolUse; got {:?}",
            parsed.events
        );
        match tools[0] {
            LlmEvent::ToolUse {
                name, input: got, ..
            } => {
                assert_eq!(name, "Read");
                assert_eq!(
                    got, &input,
                    "split input_json_delta must reassemble exactly"
                );
            }
            _ => unreachable!(),
        }

        match parsed.done().expect("exactly one Done") {
            LlmEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done(tool_use), got {other:?}"),
        }
    }

    /// Every body the `MockLlm` builder emits — across a mixed multi-turn
    /// script — passes the conformance gate. This is the anti-drift guard:
    /// if a future edit to the SSE builders produces something the real parser
    /// would retry on, this test fails loudly.
    #[test]
    fn every_builder_body_is_parser_conformant() {
        let mock = MockLlm::new()
            .text("first turn")
            .tool_use("Bash", json!({ "command": "ls -la" }))
            .text("final turn with \"quotes\" and 日本語");

        for (i, body) in mock.sse_bodies().iter().enumerate() {
            let parsed = parse_with_real_parser(body);
            assert!(
                !parsed.is_truncated(),
                "turn {i} flagged as truncated by the real parser; events: {:?}",
                parsed.events
            );
            assert!(
                parsed.done().is_some(),
                "turn {i} must yield exactly one Done; events: {:?}",
                parsed.events
            );
        }
    }

    /// Negative control: a stream cut before the terminal `message_delta` MUST
    /// be flagged as truncated. This proves the gate has teeth — it would
    /// catch a builder regression that dropped the terminal event.
    #[test]
    fn truncated_stream_is_detected() {
        let truncated = "event: message_start\n\
             data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n\
             event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n";

        let parsed = parse_with_real_parser(truncated);
        assert!(
            parsed.is_truncated(),
            "a stream without message_delta must be flagged truncated; events: {:?}",
            parsed.events
        );
        assert!(
            parsed.done().is_none(),
            "truncated stream must not yield a Done"
        );
    }
}
