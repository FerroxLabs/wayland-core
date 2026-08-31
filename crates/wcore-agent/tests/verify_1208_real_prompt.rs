//! INDEPENDENT VERIFIER instrument for wayland#1208 c1/c2 — written by the
//! adversarial verifier lane, sharing no fixture with the lane under test.
//!
//! The lane graded the wire against a HAND-BUILT three-line prompt shaped
//! "like the one build_system_prompt bakes". This grades the wire against the
//! prompt `build_system_prompt` ACTUALLY bakes — the production literal, with
//! its model line, its working-directory line and its quoted "today" sentence
//! — aged by replacing the day it rendered. If `refresh_current_date`'s line
//! matcher were sensitive to anything in the real text the lane's fixture
//! smoothed over, this is where it shows.

use std::sync::Arc;

use serde_json::{Value, json};
use wcore_agent::context::{
    SystemPromptCache, build_system_prompt, current_date_block, today_string,
};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_tools::registry::ToolRegistry;
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const YESTERYEAR: &str = "2019-07-04";

fn anthropic_text_sse(text: &str) -> String {
    format!(
        "event: message_start\ndata: {message_start}\n\n\
         event: content_block_start\ndata: {block_start}\n\n\
         event: content_block_delta\ndata: {delta}\n\n\
         event: content_block_stop\ndata: {block_stop}\n\n\
         event: message_delta\ndata: {message_delta}\n\n\
         event: message_stop\ndata: {message_stop}\n\n",
        message_start = json!({
            "type": "message_start",
            "message": {
                "id": "msg_verify_1208", "type": "message", "role": "assistant",
                "content": [], "model": "claude-mock",
                "stop_reason": Value::Null, "stop_sequence": Value::Null,
                "usage": { "input_tokens": 10, "output_tokens": 1 }
            }
        }),
        block_start = json!({
            "type": "content_block_start", "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        delta = json!({
            "type": "content_block_delta", "index": 0,
            "delta": { "type": "text_delta", "text": text }
        }),
        block_stop = json!({ "type": "content_block_stop", "index": 0 }),
        message_delta = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn", "stop_sequence": Value::Null },
            "usage": { "output_tokens": 1 }
        }),
        message_stop = json!({ "type": "message_stop" }),
    )
}

async fn start_mock() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wm_path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(anthropic_text_sse("ok")),
        )
        .mount(&server)
        .await;
    server
}

fn wire_system_text(body: &Value) -> String {
    if let Some(blocks) = body.get("system").and_then(Value::as_array) {
        return blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
    }
    body["messages"]
        .as_array()
        .expect("messages array")
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The prompt production actually bakes, rendered for TODAY and then aged to
/// [`YESTERYEAR`] — i.e. the exact string an engine bootstrapped yesterday is
/// still holding this morning.
fn real_prompt_aged_by_a_day() -> String {
    let mut cache = SystemPromptCache::new();
    let built = build_system_prompt(
        &mut cache,
        None,
        "/tmp",
        "claude-mock",
        &[],
        None,
        None,
        false,
        false,
        &[],
        false,
    );
    let today = today_string();
    assert!(
        built.contains(&current_date_block(&today)),
        "precondition: the production prompt must carry today. prompt: {built}"
    );
    let aged = built.replace(&current_date_block(&today), &current_date_block(YESTERYEAR));
    assert!(
        aged.contains(&current_date_block(YESTERYEAR)),
        "the fixture failed to age the production prompt"
    );
    assert!(
        !aged.contains(&current_date_block(&today)),
        "the aged prompt still carries today, so the assertion below is free"
    );
    aged
}

/// wayland#1208 c1, re-graded on the REAL prompt shape.
#[tokio::test]
async fn verify_1208_the_real_production_prompt_is_re_rendered_on_the_wire() {
    let server = start_mock().await;
    let aged = real_prompt_aged_by_a_day();

    let config = Config {
        provider_label: "anthropic".into(),
        provider: ProviderType::Anthropic,
        api_key: "verify-1208-key".into(),
        base_url: server.uri(),
        model: "claude-mock".into(),
        max_tokens: 256,
        max_turns: Some(1),
        compat: ProviderCompat::anthropic_defaults(),
        system_prompt: Some(aged),
        ..Default::default()
    };
    let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::new(
        "verify-1208-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    ));
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);
    let mut engine = AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), sink);

    assert!(
        engine
            .system_prompt()
            .contains(&current_date_block(YESTERYEAR)),
        "precondition: the engine must start out holding the stale day, or this \
         test proves nothing. prompt: {:?}",
        engine.system_prompt()
    );

    engine
        .run("what is today", "verify-1208-msg-1")
        .await
        .expect("the turn reached the mock provider");

    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .into_iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).expect("request body is JSON"))
        .collect();
    assert!(!bodies.is_empty(), "no request reached the provider");

    let system = wire_system_text(&bodies[0]);
    assert!(
        !system.is_empty(),
        "the captured system field is EMPTY, so both assertions below pass free"
    );
    assert!(
        !system.contains(&current_date_block(YESTERYEAR)),
        "the wire still declares {YESTERYEAR} as the authoritative today. \
         system was: {system:?}"
    );
    assert!(
        system.contains(&current_date_block(&today_string())),
        "the wire does not declare today. system was: {system:?}"
    );
    // The instruction that makes a stale date harmful must survive the
    // rewrite — the criterion's other branch is "stops telling the model the
    // baked date is authoritative", and the fix chose the first branch, so the
    // sentence has to still be there for the first branch to be the one met.
    assert!(
        system.contains("authoritative"),
        "the rewrite dropped the authoritative-date instruction, which is a \
         DIFFERENT criterion branch than the one the lane claims. system: {system:?}"
    );
}

/// A SECOND turn on the SAME engine — the pooled shape c2 is about — must not
/// regress to the baked day, and must not thrash the prefix within one day.
#[tokio::test]
async fn verify_1208_a_second_turn_on_one_engine_is_stable_and_fresh() {
    let server = start_mock().await;
    let aged = real_prompt_aged_by_a_day();

    let config = Config {
        provider_label: "anthropic".into(),
        provider: ProviderType::Anthropic,
        api_key: "verify-1208-key".into(),
        base_url: server.uri(),
        model: "claude-mock".into(),
        max_tokens: 256,
        max_turns: Some(1),
        compat: ProviderCompat::anthropic_defaults(),
        system_prompt: Some(aged),
        ..Default::default()
    };
    let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::new(
        "verify-1208-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    ));
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);
    let mut engine = AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), sink);

    for (n, text) in [(1u8, "hello"), (2u8, "and what is today")] {
        engine
            .run(text, &format!("verify-1208-msg-{n}"))
            .await
            .unwrap_or_else(|e| panic!("turn {n} failed: {e}"));
    }

    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .into_iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).expect("request body is JSON"))
        .collect();
    assert_eq!(bodies.len(), 2, "two turns must produce two requests");
    assert!(
        bodies[1]["messages"]
            .as_array()
            .expect("messages array")
            .len()
            > 1,
        "turn 2 carried no transcript, so it was not the same long-lived engine"
    );

    let first = wire_system_text(&bodies[0]);
    let second = wire_system_text(&bodies[1]);
    for (i, system) in [&first, &second].iter().enumerate() {
        assert!(
            !system.contains(&current_date_block(YESTERYEAR)),
            "turn {} still declares the baked day. system: {system:?}",
            i + 1
        );
        assert!(
            system.contains(&current_date_block(&today_string())),
            "turn {} does not declare today. system: {system:?}",
            i + 1
        );
    }
    assert_eq!(
        first, second,
        "within ONE day the system prefix must stay byte-identical, or the fix \
         traded a stale date for a per-turn prompt-cache miss"
    );
}
