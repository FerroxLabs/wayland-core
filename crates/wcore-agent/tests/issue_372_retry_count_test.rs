//! #372 — the retry count has to be on the wire, and it has to reset per turn.
//!
//! The ticket asks by name for "retry count", and for it to be shown separately
//! from the run timer: the reporter could not tell a stalled run from a
//! silently-retrying one because every re-send looked like a fresh start. Core
//! already numbered its re-sends internally (`stream_attempt`, rendered into
//! the "attempt 3/10" notice), but nothing machine-readable carried the number
//! — `provider_retry` published only `failure: Option<String>`.
//!
//! Counting `provider_retry` frames is NOT a substitute. The event is additive,
//! so a host pinned below the minor that introduced it drops it under the W0
//! decoder contract, and a host that attaches mid-run never saw the earlier
//! frames at all; either way the ordinal it missed is unrecoverable unless the
//! frame carries it.
//!
//! These tests drive the REAL engine through the REAL producer
//! (`ProtocolSink`) and the REAL serializer, and assert on the JSON Lines a
//! host actually reads.

mod common;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{MockLlmProvider, MockTool, test_config};

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
    fn of_type(&self, wanted: &str) -> Vec<Value> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value["type"] == wanted)
            .collect()
    }
}

/// An attempt that streams some text and then dies mid-stream. Text first so
/// the attempt counts as having produced output, which keeps the
/// output-stall monitor out of a test that is about the retry ordinal.
fn failing_attempt() -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("thinking".to_string()),
        LlmEvent::Error("connection reset".to_string()),
    ]
}

fn tool_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-1".to_string(),
            name: "inventory".to_string(),
            input: json!({ "q": "tools" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage {
                input_tokens: 80,
                output_tokens: 30,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                // These fixtures carry no provider-reported price. `None` is
                // "the provider said nothing about cost", which is the honest
                // value here -- zero would claim the calls were free.
                reported_cost_usd: None,
            },
        },
    ]
}

fn text_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                // These fixtures carry no provider-reported price. `None` is
                // "the provider said nothing about cost", which is the honest
                // value here -- zero would claim the calls were free.
                reported_cost_usd: None,
            },
        },
    ]
}

/// Drive the engine over the scripted provider attempts and return the wire.
async fn run_scripted(attempts: Vec<Vec<LlmEvent>>) -> Arc<WireRecorder> {
    let recorder = Arc::new(WireRecorder::default());
    let sink = ProtocolSink::with_emitter(recorder.clone());
    let output: Arc<dyn OutputSink> = Arc::new(sink);

    let provider = Arc::new(MockLlmProvider::with_turns(attempts));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("inventory", "ok", false)));

    let mut config = test_config();
    // The route from the ticket: an OpenAI-compatible local endpoint.
    config.provider = wcore_config::config::ProviderType::OpenAI;
    config.compat = wcore_config::compat::ProviderCompat::openai_defaults();
    config.model = "qwen3:8b".to_string();
    config.base_url = "http://127.0.0.1:11434/v1".to_string();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine
        .run("inventory the tools", "")
        .await
        .expect("the run recovers and completes");
    recorder
}

/// Two re-sends inside one turn, then one inside the next: the ordinal counts
/// up within a turn and restarts on the next one, so a host renders "retry 2 of
/// this step" rather than a running total that never resets.
#[tokio::test]
async fn each_provider_retry_carries_its_ordinal_within_the_turn() {
    let recorder = run_scripted(vec![
        // turn 1: fail, fail, then a tool call that commits the turn.
        failing_attempt(),
        failing_attempt(),
        tool_turn(),
        // turn 2: fail once, then the final answer.
        failing_attempt(),
        text_turn(),
    ])
    .await;

    let retries = recorder.of_type("provider_retry");
    assert_eq!(
        retries.len(),
        3,
        "two re-sends on turn 1 and one on turn 2: {retries:?}"
    );
    let ordinals: Vec<&Value> = retries.iter().map(|event| &event["retry"]).collect();
    assert_eq!(
        ordinals,
        vec![&json!(1), &json!(2), &json!(1)],
        "the retry count must number the re-sends of ITS turn and restart on \
         the next one — a flat 0 or a never-resetting total is the gap #372 \
         reports: {retries:?}"
    );
    for event in &retries {
        assert_eq!(
            event["failure"], "stream_error",
            "the ordinal must not displace the stable failure class: {retries:?}"
        );
    }

    // Known-positive control in the same run: the events whose absence would
    // make the assertion above vacuous were really emitted, and the run really
    // reached its end rather than dying early with an empty wire.
    assert_eq!(
        recorder.of_type("provider_failure").len(),
        3,
        "one typed failure per failed attempt"
    );
    assert_eq!(
        recorder.of_type("route_info").len(),
        2,
        "one route_info per COMMITTED turn — failed attempts are not turns"
    );
}

/// A run that never fails must not claim a retry. Without this, a fix that
/// emitted `retry: 1` unconditionally would pass the test above.
#[tokio::test]
async fn a_clean_run_publishes_no_retry_at_all() {
    let recorder = run_scripted(vec![tool_turn(), text_turn()]).await;
    let retries = recorder.of_type("provider_retry");
    assert!(
        retries.is_empty(),
        "a run with no failed attempt must publish no retry: {retries:?}"
    );
    // Control: the wire is not empty for an unrelated reason.
    assert_eq!(
        recorder.of_type("route_info").len(),
        2,
        "the run really executed two turns"
    );
}

/// The sibling ordinal, over the real physical-send boundary. `provider_retry`
/// is emitted from the engine directly; `provider_attempt` is emitted from the
/// provider-attempt OBSERVER, a different call site with its own `Arc` clone of
/// the sequence, so it needs its own graded path rather than inheriting the
/// retry test's verdict.
#[tokio::test]
async fn a_physical_attempt_carries_its_ordinal_and_restarts_each_turn() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let recorder = Arc::new(WireRecorder::default());
    let sink = ProtocolSink::with_emitter(recorder.clone());
    let output: Arc<dyn OutputSink> = Arc::new(sink);

    let provider = Arc::new(
        MockLlmProvider::with_turns(vec![tool_turn(), text_turn()]).with_physical_url(server.uri()),
    );
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("inventory", "ok", false)));

    let mut config = test_config();
    config.provider = wcore_config::config::ProviderType::OpenAI;
    config.compat = wcore_config::compat::ProviderCompat::openai_defaults();
    config.model = "qwen3:8b".to_string();
    config.base_url = "http://127.0.0.1:11434/v1".to_string();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine.run("inventory the tools", "").await.expect("run");

    let attempts = recorder.of_type("provider_attempt");
    assert_eq!(
        attempts.len(),
        2,
        "one clean physical send per turn: {attempts:?}"
    );
    let ordinals: Vec<&Value> = attempts.iter().map(|event| &event["attempt"]).collect();
    assert_eq!(
        ordinals,
        vec![&json!(1), &json!(1)],
        "each turn's first physical send is attempt 1 — a run-scoped counter \
         would publish 1 then 2, and a dropped ordinal 0 then 0: {attempts:?}"
    );
    for event in &attempts {
        assert!(
            event.get("failure").is_none(),
            "a clean attempt carries no failure class, but still carries its \
             ordinal: {attempts:?}"
        );
    }
}
