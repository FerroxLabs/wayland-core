//! #923 — grade the PRE-SEND repair at the send boundary, not at the function.
//!
//! The issue asks for two things. This file grades the first:
//!
//! > validate/repair message history before send (drop or re-pair orphaned
//! > tool messages)
//!
//! `repair_all_orphaned_tool_uses` / `repair_orphaned_tool_results` are called
//! unconditionally just before the request is built (`engine.rs`, immediately
//! above `messages: self.messages.clone()`). That wiring was UNGRADED at
//! v0.13.5: every existing test — `orphan_repair_*` in `engine.rs`'s unit
//! module, and `autocompact_never_emits_orphaned_tool_result_285`, which
//! literally comments "run the same pre-send repairs the request-build path
//! runs" — invokes the two functions DIRECTLY. Deleting either call from the
//! send path therefore left the whole suite green, and an ungraded guard
//! regresses silently.
//!
//! Every test here reads the `LlmRequest` the provider was actually handed, so
//! the claim is about the array that goes on the wire. Each is paired with a
//! control that runs the same machinery on a history that needs no repair, so
//! a capture that silently stopped working cannot make a claim pass vacuously.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};

use common::{physical_attempt_server, test_config};

// ---------------------------------------------------------------------------
// A provider that answers cleanly and records the request array it was given.
// ---------------------------------------------------------------------------
struct RecordingProvider {
    seen: Arc<Mutex<Vec<Vec<Message>>>>,
    physical_url: String,
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(&self, r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.seen.lock().unwrap().push(r.messages.clone());
        // Cross a real HTTP boundary, exactly as the other #923 harnesses do,
        // so the send is a genuine physical attempt rather than a pure
        // in-memory shortcut.
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let _ =
            scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url))).await?;
        let (tx, rx) = mpsc::channel(64);
        for event in [
            LlmEvent::TextDelta("ok".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    ..Default::default()
                },
            },
        ] {
            let _ = tx.send(event).await;
        }
        Ok(rx)
    }
}

#[derive(Default)]
struct NullSink;

impl OutputSink for NullSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool, _: wcore_protocol::events::FailureCategory) {}
    fn emit_info(&self, _: &str) {}
}

struct Sent {
    requests: Vec<Vec<Message>>,
    _server: wiremock::MockServer,
}

impl Sent {
    /// The array handed to the FIRST (and here, only) provider send.
    fn first(&self) -> &[Message] {
        assert_eq!(
            self.requests.len(),
            1,
            "expected exactly one provider send; the assertions below are about \
             a single request array"
        );
        &self.requests[0]
    }
}

/// Seed `history` into a fresh engine, run one turn, return what the provider
/// was handed.
async fn send_with_history(history: Vec<Message>) -> Sent {
    let server = physical_attempt_server().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        seen: seen.clone(),
        physical_url: server.uri(),
    });
    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        Arc::new(NullSink) as Arc<dyn OutputSink>,
    );
    engine.load_conversation(history);
    engine
        .run("carry on", "")
        .await
        .expect("the scripted provider answers cleanly");
    let requests = seen.lock().unwrap().clone();
    Sent {
        requests,
        _server: server,
    }
}

// ---------------------------------------------------------------------------
// Shape helpers, evaluated over the array that went on the wire.
// ---------------------------------------------------------------------------
fn tool_use_ids(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_ids(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
            _ => None,
        })
        .collect()
}

fn assistant_tool_use(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"path": "config.toml"}),
            extra: None,
        }],
    )
}

fn user_tool_result(id: &str) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: "port = 8080".to_string(),
            is_error: false,
        }],
    )
}

fn user_text(text: &str) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    )
}

// ===========================================================================
// A. forward direction — `tool_use` with no answering `tool_result`
// ===========================================================================

/// GRADES: "validate/repair message history before send", forward direction,
/// AT THE SEND BOUNDARY.
///
/// A `tool_use` orphaned mid-history (the shape a cancel / reaper denial /
/// channel drop leaves behind) must never reach the provider. Anthropic 400s
/// on it, which is the session-killer this issue is about.
///
/// RED ARM OBSERVED: deleting `self.repair_all_orphaned_tool_uses();` from the
/// pre-send site in `engine.rs` fails this test and NOTHING else in the crate.
#[tokio::test]
async fn a_orphaned_tool_use_is_repaired_before_it_reaches_the_provider() {
    let sent = send_with_history(vec![
        user_text("read the config"),
        assistant_tool_use("toolu_orphan"),
        // No tool_result. Some other turn lands on top, so the trailing-only
        // repair never sees it.
        user_text("actually, never mind"),
    ])
    .await;
    let wire = sent.first();
    assert!(
        tool_use_ids(wire).contains(&"toolu_orphan".to_string()),
        "CONTROL BROKEN: the seeded tool_use never reached the request at all, \
         so the pairing assertion below would pass vacuously. Wire: {wire:?}"
    );
    assert!(
        tool_result_ids(wire).contains(&"toolu_orphan".to_string()),
        "#923: an orphaned tool_use went on the wire with no answering \
         tool_result — the pre-send forward repair is not wired into the \
         request-build path. Wire: {wire:?}"
    );
}

// ===========================================================================
// B. reverse direction (#285) — `tool_result` with no parent `tool_use`
// ===========================================================================

/// GRADES: the reverse direction, at the send boundary. A `tool_result` whose
/// `tool_use` was summarized away is the shape that makes DeepSeek reject the
/// whole array with `missing field tool_call_id` — the exact refusal in this
/// ticket's title.
///
/// RED ARM OBSERVED: deleting `self.repair_orphaned_tool_results();` from the
/// pre-send site fails this test and nothing else.
#[tokio::test]
async fn b_orphaned_tool_result_is_repaired_before_it_reaches_the_provider() {
    let sent = send_with_history(vec![
        user_text("read the config"),
        // The parent assistant tool_use is GONE (compaction folded it into
        // prose); only its result survives.
        user_tool_result("toolu_vanished"),
    ])
    .await;
    let wire = sent.first();
    let uses = tool_use_ids(wire);
    let dangling: Vec<String> = tool_result_ids(wire)
        .into_iter()
        .filter(|id| !uses.contains(id))
        .collect();
    assert!(
        dangling.is_empty(),
        "#923/#285: a tool_result with no parent tool_use went on the wire \
         (ids {dangling:?}) — the pre-send reverse repair is not wired into \
         the request-build path. Wire: {wire:?}"
    );
    // And the content must survive as context rather than being silently lost.
    let carried = wire.iter().flat_map(|m| &m.content).any(|b| match b {
        ContentBlock::Text { text } => text.contains("port = 8080"),
        _ => false,
    });
    assert!(
        carried,
        "the orphaned result's content was dropped instead of demoted to \
         text. Wire: {wire:?}"
    );
}

// ===========================================================================
// C. control — a clean history must survive untouched
// ===========================================================================

/// NEGATIVE CONTROL for A and B. A well-paired history must reach the provider
/// with the pair INTACT. If this fails, the repairs are mangling good history
/// and the two assertions above prove nothing about repair — only about
/// deletion. It also proves the capture sees real tool blocks, so the "no
/// dangling result" assertion in B is not passing on an empty array.
#[tokio::test]
async fn c_control_a_clean_tool_pair_reaches_the_provider_unchanged() {
    let sent = send_with_history(vec![
        user_text("read the config"),
        assistant_tool_use("toolu_clean"),
        user_tool_result("toolu_clean"),
    ])
    .await;
    let wire = sent.first();
    assert_eq!(
        tool_use_ids(wire),
        vec!["toolu_clean".to_string()],
        "CONTROL BROKEN: a clean tool_use did not survive to the wire: {wire:?}"
    );
    assert_eq!(
        tool_result_ids(wire),
        vec!["toolu_clean".to_string()],
        "CONTROL BROKEN: a clean tool_result was duplicated or dropped — the \
         repairs are not idempotent on good history: {wire:?}"
    );
}

// ===========================================================================
// D. the empty-id shape
// ===========================================================================

/// OBSERVED GAP at v0.13.5, then fixed.
///
/// Both pre-send repairs key on id EQUALITY, so an empty id is "matched" by
/// another empty id: `repair_orphaned_tool_results` puts `""` in its `live_ids`
/// set and `repair_all_orphaned_tool_uses` puts `""` in its `satisfied` set, so
/// a `tool_use`/`tool_result` pair that both carry an empty id passes both
/// guards untouched.
///
/// It is reachable: `anthropic_shared.rs` reads the streamed id with
/// `.unwrap_or("")`, and #170 is the report of a router dropping that id. On
/// the OpenAI family `strip_empty_tool_call_ids` (`openai.rs`, ungated by
/// compat) catches it at the serializer; the Anthropic-shape serializer has no
/// equivalent, so the empty id goes out as-is.
///
/// This does NOT claim to be the root cause of #923 — the reported array has
/// never been captured. It is a hole in the guard the ticket asks for, found by
/// reading the guard, and closed where the ticket asks for it to be closed:
/// before the send.
#[tokio::test]
async fn d_an_empty_tool_id_does_not_reach_the_provider() {
    let sent = send_with_history(vec![
        user_text("read the config"),
        assistant_tool_use(""),
        user_tool_result(""),
    ])
    .await;
    let wire = sent.first();
    assert!(
        !tool_result_ids(wire).iter().any(|id| id.is_empty()),
        "#923: a tool_result with an EMPTY tool_use_id went on the wire — this \
         serializes as `tool_call_id: \"\"` (OpenAI shape) / an unanswerable \
         pair (Anthropic shape) and is refused. Wire: {wire:?}"
    );
    assert!(
        !tool_use_ids(wire).iter().any(|id| id.is_empty()),
        "#923: a tool_use with an EMPTY id went on the wire. Wire: {wire:?}"
    );
    // The result's content must still be carried as context, same contract as
    // the reverse repair.
    let carried = wire.iter().flat_map(|m| &m.content).any(|b| match b {
        ContentBlock::Text { text } => text.contains("port = 8080"),
        _ => false,
    });
    assert!(
        carried,
        "the empty-id result's content was dropped rather than demoted: {wire:?}"
    );
}
