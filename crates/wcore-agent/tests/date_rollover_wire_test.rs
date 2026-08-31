//! wayland#1208 — a session that outlives the day it started must stop
//! asserting the day it started.
//!
//! #559 moved `Current date:` INTO the session-permanent cached system prefix
//! and out of the per-turn message tail. That is the right place for it, but
//! the prefix is built exactly once (`bootstrap.rs`) into a plain `String` on
//! the engine, and nothing re-rendered it — while the very same prefix tells
//! the model to treat that date as the authoritative today and forbids it
//! substituting a different month or year.
//!
//! Both tests here grade the bytes the provider actually put on the socket,
//! not a helper: the stale day is planted in the engine's prompt the way a
//! real overnight session holds it, a REAL turn is driven, and the captured
//! request body is read.
//!
//! * c1 grades a plain engine — the `AgentEngine::run` funnel every surface
//!   shares.
//! * c2 grades the CHANNEL GATEWAY specifically, through the real
//!   `ChannelTurnDispatcher`, because that is the surface where the defect is
//!   not a corner case: the dispatcher pools one `AgentEngine` per
//!   conversation and never evicts it (`channel_dispatch.rs` TODO(phase) 1),
//!   so a bot answers with the gateway's start day for as long as it runs.

mod common;

use std::sync::Arc;

use common::configure_persisted_test_session;
use serde_json::{Value, json};
use wcore_agent::channel_dispatch::ChannelTurnDispatcher;
use wcore_agent::channel_inbound::TurnDispatcher;
use wcore_agent::channel_policy::ChannelPolicyRegistry;
use wcore_agent::context::{current_date_block, today_string};
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

/// The day the session started, in the past by years so no clock skew, time
/// zone or midnight-crossing test run can make it accidentally equal today.
const START_DAY: &str = "2020-01-01";

/// A system prompt shaped like the one `build_system_prompt` bakes: the date
/// declaration on its own line, immediately followed by the sentence that
/// makes the stale value harmful rather than merely wrong.
fn prompt_baked_on(day: &str) -> String {
    format!(
        "You are an AI assistant that can use tools to help with tasks.\n\
         {date}\n\
         When constructing time-bound queries, use the current date given \
         above as the authoritative today. Do NOT substitute a different \
         month or year.",
        date = current_date_block(day)
    )
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

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
                "id": "msg_1208_mock", "type": "message", "role": "assistant",
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

fn resolved_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("workdir");
    let canonical = std::fs::canonicalize(dir.path()).expect("resolve the workdir");
    (dir, dunce::simplified(&canonical).to_path_buf())
}

fn anthropic_provider(base_url: &str) -> Arc<dyn LlmProvider> {
    Arc::new(AnthropicProvider::new(
        "anthropic-1208-test-key",
        base_url,
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    ))
}

fn base_config(base_url: &str) -> Config {
    Config {
        provider_label: "anthropic".into(),
        provider: ProviderType::Anthropic,
        api_key: "anthropic-1208-test-key".into(),
        base_url: base_url.into(),
        model: "claude-mock".into(),
        max_tokens: 256,
        max_turns: Some(1),
        compat: ProviderCompat::anthropic_defaults(),
        ..Default::default()
    }
}

/// The single system string the provider put on the wire.
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

async fn captured_bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .into_iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).expect("request body is JSON"))
        .collect()
}

/// Assert `system` reports today and no longer reports [`START_DAY`].
fn assert_reports_today(label: &str, system: &str) {
    assert!(
        !system.is_empty(),
        "{label}: the captured system field is EMPTY, so both assertions below \
         would pass for free"
    );
    let stale = current_date_block(START_DAY);
    let fresh = current_date_block(&today_string());
    assert!(
        !system.contains(&stale),
        "{label}: the turn still declares {stale:?} as the authoritative today. \
         system was: {system:?}"
    );
    assert!(
        system.contains(&fresh),
        "{label}: the turn does not declare {fresh:?}. system was: {system:?}"
    );
}

/// wayland#1208 c1 — a session that crosses midnight reports the REAL date.
///
/// The engine is constructed holding a prompt baked on [`START_DAY`], which is
/// exactly the state an overnight session is in: `build_system_prompt` ran
/// once, its result was moved into `AgentEngine::system_prompt`, and the day
/// then rolled over. One real turn is driven and the wire is read.
#[tokio::test]
async fn a_turn_after_the_day_rolled_over_declares_todays_date_on_the_wire() {
    let server = start_mock().await;
    let mut config = base_config(&server.uri());
    config.system_prompt = Some(prompt_baked_on(START_DAY));

    let mut engine = AgentEngine::new_with_provider(
        anthropic_provider(&server.uri()),
        config,
        ToolRegistry::new(),
        null_output(),
    );
    assert!(
        engine
            .system_prompt()
            .contains(&current_date_block(START_DAY)),
        "precondition: the engine must start out holding the STALE date, or \
         this test proves nothing. prompt was: {:?}",
        engine.system_prompt()
    );

    engine
        .run("what is today", "d1-msg-1")
        .await
        .expect("the turn reached the mock provider");

    let bodies = captured_bodies(&server).await;
    assert!(
        !bodies.is_empty(),
        "no request reached the provider, so nothing was graded"
    );
    assert_reports_today("engine-turn", &wire_system_text(&bodies[0]));
}

/// wayland#1208 c2 — the CHANNEL-GATEWAY ENGINE POOL is covered.
///
/// `ChannelTurnDispatcher` builds one `AgentEngine` per conversation and
/// caches the `Arc` forever, so the prompt baked when the gateway first saw a
/// conversation is the prompt every later turn in it sends. This drives the
/// real dispatcher — its own pool, its own `remote_channel_config`, its own
/// `engine.run` — over TWO turns on ONE session key.
///
/// Turn 2 is what the criterion is about: it is served by the pooled engine
/// built during turn 1, and the second request body proves the pooling
/// happened (it carries turn 1's exchange in `messages`) rather than the test
/// silently grading two fresh engines.
#[tokio::test]
async fn the_pooled_channel_engine_does_not_answer_with_the_gateways_start_day() {
    let server = start_mock().await;
    let (_dir, cwd) = resolved_workspace();
    let mut config = base_config(&server.uri());
    config.system_prompt = Some(prompt_baked_on(START_DAY));
    configure_persisted_test_session(&mut config, &cwd);

    let dispatcher = ChannelTurnDispatcher::new(
        config,
        cwd.to_str().expect("utf-8 workdir").to_string(),
        anthropic_provider(&server.uri()),
        Arc::new(ChannelPolicyRegistry::default()),
        None,
    );

    let session_key = "agent:main:slack:dm:c1";
    for (n, text) in [(1u8, "hello"), (2u8, "and what is today")] {
        let msg = wcore_channels::IncomingMessage::new(
            format!("m{n}"),
            "c1",
            "alice",
            text.to_string(),
            0,
        );
        let reply = dispatcher
            .dispatch(session_key, "c1", &msg)
            .await
            .unwrap_or_else(|e| panic!("dispatch {n} failed before reaching the provider: {e}"));
        assert_eq!(
            reply.as_deref(),
            Some("ok"),
            "dispatch {n} did not complete a real turn"
        );
    }

    let bodies = captured_bodies(&server).await;
    assert_eq!(
        bodies.len(),
        2,
        "two dispatches must produce two outbound requests"
    );

    // The pool is load-bearing here, so prove it rather than assume it: a
    // second engine would start with an empty transcript.
    let second_messages = bodies[1]["messages"]
        .as_array()
        .expect("messages array")
        .len();
    assert!(
        second_messages > 1,
        "turn 2 was served by a FRESH engine (transcript of {second_messages}), \
         so this test never graded the pooled engine the criterion is about"
    );

    for (i, body) in bodies.iter().enumerate() {
        assert_reports_today(
            &format!("pooled-channel-turn-{}", i + 1),
            &wire_system_text(body),
        );
    }
}
