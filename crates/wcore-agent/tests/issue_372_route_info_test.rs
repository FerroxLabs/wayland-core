//! #372 — the run's route has to be visible on the wire.
//!
//! The reporter ran the same task against a local Ollama endpoint
//! (`http://127.0.0.1:11434`) and against a cloud OpenRouter endpoint. Both
//! were driven as `--provider openai`, so every route-bearing field Core
//! published — `turn_trace.provider`, `turn_cost.provider` — read `openai` for
//! both, and nothing on the wire could tell the two runs apart. The endpoint
//! itself was absent from the protocol entirely.
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
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn route_info(&self) -> Vec<Value> {
        self.lines()
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value["type"] == "route_info")
            .collect()
    }
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
                // No provider-reported price in this fixture. `None` is
                // "the provider said nothing about cost"; zero would
                // claim the call was free.
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
                // No provider-reported price in this fixture. `None` is
                // "the provider said nothing about cost"; zero would
                // claim the call was free.
                reported_cost_usd: None,
            },
        },
    ]
}

/// Run one tool turn plus one final text turn against `base_url`, returning the
/// `route_info` lines the host would have received. Two turns so BOTH engine
/// emission sites (the tool-continuing turn and the final no-tool-calls turn)
/// are exercised.
async fn run_against(base_url: &str) -> Vec<Value> {
    let recorder = Arc::new(WireRecorder::default());
    let sink = ProtocolSink::with_emitter(recorder.clone());
    let output: Arc<dyn OutputSink> = Arc::new(sink);

    let provider = Arc::new(MockLlmProvider::with_turns(vec![tool_turn(), text_turn()]));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("inventory", "ok", false)));

    let mut config = test_config();
    // The exact shape of #372: an OpenAI-compatible route, where the provider
    // id alone cannot say whether the endpoint is local or cloud.
    config.provider = wcore_config::config::ProviderType::OpenAI;
    config.compat = wcore_config::compat::ProviderCompat::openai_defaults();
    config.model = "qwen3:8b".to_string();
    config.base_url = base_url.to_string();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine
        .run("inventory the tools", "")
        .await
        .expect("run completes");
    recorder.route_info()
}

#[tokio::test]
async fn a_local_route_is_labelled_local_on_every_turn() {
    let routes = run_against("http://127.0.0.1:11434/v1").await;
    assert_eq!(
        routes.len(),
        2,
        "one route_info per turn, from both engine emission sites: {routes:?}"
    );
    for (index, route) in routes.iter().enumerate() {
        assert_eq!(route["route"]["turn"], index, "{routes:?}");
        assert_eq!(route["route"]["provider"], "openai", "{routes:?}");
        assert_eq!(route["route"]["model"], "qwen3:8b", "{routes:?}");
        assert_eq!(
            route["route"]["base_url"], "http://127.0.0.1:11434/v1",
            "the endpoint the reporter could not see must be on the wire: {routes:?}"
        );
        assert_eq!(
            route["route"]["local"], true,
            "a loopback endpoint must be labelled local: {routes:?}"
        );
    }
}

#[tokio::test]
async fn a_cloud_route_on_the_same_provider_id_is_not_labelled_local() {
    let routes = run_against("https://openrouter.ai/api/v1").await;
    assert_eq!(routes.len(), 2, "{routes:?}");
    for route in &routes {
        assert_eq!(
            route["route"]["provider"], "openai",
            "both of the reporter's routes report the same provider id: {routes:?}"
        );
        assert_eq!(
            route["route"]["base_url"], "https://openrouter.ai/api/v1",
            "{routes:?}"
        );
        assert_eq!(
            route["route"]["local"], false,
            "a cloud endpoint must NOT be labelled local: {routes:?}"
        );
    }
}

/// The endpoint reaches a host log and a user's screen. A `base_url` can carry
/// the API key in userinfo or in a query string, and neither may survive the
/// trip through the engine.
#[tokio::test]
async fn a_credential_in_the_base_url_never_reaches_the_wire() {
    let recorder_lines = {
        let routes =
            run_against("https://key:s3cr3t-372@gateway.example.com/v1?api_key=s3cr3t-372").await;
        assert_eq!(routes.len(), 2, "{routes:?}");
        routes
    };
    for route in &recorder_lines {
        let wire = route.to_string();
        assert!(
            !wire.contains("s3cr3t-372"),
            "the base_url credential reached the wire: {wire}"
        );
        assert_eq!(
            route["route"]["base_url"], "https://gateway.example.com/v1",
            "scrubbing must keep the diagnostic host and path: {wire}"
        );
        assert_eq!(route["route"]["local"], false, "{wire}");
    }
}
