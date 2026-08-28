//! #1150, the runaway itself — driven through `AgentEngine::run()`.
//!
//! The reporter ran a 32k local model over an OpenAI-compatible endpoint and
//! sat at **83,208 input tokens**. Nothing stopped them, because an unlisted
//! model's compaction boundaries were computed from a fabricated 200,000-token
//! window: microcompact at 83,500, autocompact at 167,000, emergency at
//! 197,000. On a 32k model those are numbers the session can never reach — the
//! endpoint truncates or 400s first — so by default the context simply grew.
//!
//! These tests take the reporter's position and assert on what the ENGINE
//! does, not on what a helper returns: the exact watermark they reported, the
//! default config (`[compact] context_window` unset), and an unlisted model.
//!
//! `common::test_config()` is already that configuration — `model:
//! "test-model"` matches no arm of `wcore_config::limits::model_output_ceiling`
//! and `compact.context_window` defaults to `None`.

mod common;

use std::sync::{Arc, Mutex};

use wcore_agent::engine::{AgentEngine, AgentError};
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

/// The watermark from the report.
const REPORTED_TOKENS: u64 = 83_208;

/// The compaction fallback for a model whose window is unverified, and the
/// boundaries that follow from it under the shipped buffer defaults.
const UNVERIFIED_WINDOW: u64 = 32_768;
const EXPECTED_EMERGENCY_LIMIT: usize = 29_768; // 32_768 - emergency_buffer 3_000

/// Records what the user is told; every other surface is inert.
#[derive(Default)]
struct NoticeSink {
    infos: Mutex<Vec<String>>,
}

impl NoticeSink {
    fn infos(&self) -> Vec<String> {
        self.infos.lock().unwrap().clone()
    }
}

impl OutputSink for NoticeSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        NullSink.emit_text_delta(text, msg_id);
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        NullSink.emit_thinking(text, msg_id);
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        NullSink.emit_tool_call(name, input);
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        NullSink.emit_tool_result(name, is_error, content);
    }
    fn emit_stream_start(&self, msg_id: &str) {
        NullSink.emit_stream_start(msg_id);
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
        finish: wcore_types::message::FinishReason,
    ) {
        NullSink.emit_stream_end(
            msg_id,
            turns,
            input,
            output,
            cache_creation,
            cache_read,
            finish,
        );
    }
    fn emit_error(&self, msg: &str, retryable: bool) {
        NullSink.emit_error(msg, retryable);
    }
    fn emit_info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }
}

/// Turn 1: a tool call whose usage reports the reporter's watermark. This is
/// what puts `last_input_tokens` / `last_real_input_tokens` there before the
/// engine decides whether to run another turn.
fn turn_at(input_tokens: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ]
}

fn text_turn(text: &str, input_tokens: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: TokenUsage {
                input_tokens,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ]
}

fn registry_with_mock_tool() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));
    registry
}

// ── the runaway, with compaction off: the emergency net ─────────────────────

/// THE #1150 GUARD. With autocompact disabled the emergency hard stop is the
/// only thing between the session and unbounded growth. On an unlisted model
/// with no operator override it must fire far below where the reporter sat.
///
/// Red arm (origin/main and commit 92aed93d): the limit is 197,000, 83,208 is
/// comfortably under it, `run()` returns `Ok`, and the session takes another
/// turn on a context the endpoint cannot serve.
#[tokio::test]
async fn an_unlisted_model_hard_stops_far_below_the_reporters_watermark() {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        turn_at(REPORTED_TOKENS),
        // Queued, and it must never be consumed.
        text_turn("this turn must never be dispatched", 90_000),
    ]));

    let mut config = test_config();
    // Isolate the emergency arithmetic, exactly as tc_2_6_03 does.
    config.compact.enabled = false;
    // The reporter's configuration: no operator override, unlisted model.
    assert_eq!(
        config.compact.context_window, None,
        "precondition: this test is about the DEFAULT, unconfigured window"
    );
    assert_eq!(config.model, "test-model", "precondition: unlisted model");

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry_with_mock_tool(),
        Arc::new(NoticeSink::default()),
    );
    let err = engine.run("do something long", "msg-1").await.expect_err(
        "an unlisted model sitting at the reporter's 83,208 input tokens must be stopped, \
             not handed another turn",
    );

    match err {
        AgentError::ContextTooLong {
            input_tokens,
            limit,
        } => {
            assert_eq!(input_tokens, REPORTED_TOKENS);
            assert_eq!(
                limit, EXPECTED_EMERGENCY_LIMIT,
                "the hard stop must be derived from the conservative unverified window \
                 ({UNVERIFIED_WINDOW}), not from a fabricated 200,000"
            );
        }
        other => panic!("expected ContextTooLong, got: {other:?}"),
    }
}

// ── the runaway, with compaction on: the autocompact trigger ────────────────

/// The same position with the shipped defaults (compaction ENABLED). The
/// engine must relieve the pressure rather than carry 83,208 tokens into the
/// next turn.
///
/// Red arm (commit 92aed93d): the autocompact threshold is 167,000, nothing
/// fires, and the only `emit_info` traffic is unrelated.
#[tokio::test]
async fn an_unlisted_model_compacts_before_the_reporters_watermark() {
    let notices = Arc::new(NoticeSink::default());
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        turn_at(REPORTED_TOKENS),
        // The summarization call autocompact makes at the top of turn 2.
        text_turn("summary of the conversation so far", 5_000),
        text_turn("done", 4_000),
    ]));

    let config = test_config();
    assert!(
        config.compact.enabled,
        "precondition: this arm is about the SHIPPED defaults"
    );

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry_with_mock_tool(),
        notices.clone(),
    );
    let _ = engine.run("do something long", "msg-1").await;

    let infos = notices.infos();
    assert!(
        infos
            .iter()
            .any(|m| m.starts_with("Autocompact: summarized")),
        "at 83,208 input tokens on an unlisted model the engine must summarize rather than \
         keep growing. Everything it said instead: {infos:?}"
    );
}

// ── the control: a model we DO know must be untouched ───────────────────────

/// The reason the two assertions above are not simply "compact sooner, always".
/// A registry-KNOWN 1,000,000-token model at the same watermark is at 8% of its
/// window and must neither compact nor hard-stop. Without this arm, lowering
/// the unverified fallback could have been implemented as a blanket cut that
/// wrecked every large model, and both tests above would still pass.
#[tokio::test]
async fn a_known_large_window_model_is_untouched_at_the_same_watermark() {
    let notices = Arc::new(NoticeSink::default());
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        turn_at(REPORTED_TOKENS),
        text_turn("done", 84_000),
    ]));

    let mut config = test_config();
    // 1,000,000-token window, straight from `wcore_config::limits`.
    config.model = "claude-opus-4-8".to_string();

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry_with_mock_tool(),
        notices.clone(),
    );
    let result = engine.run("do something long", "msg-1").await;

    assert!(
        result.is_ok(),
        "83,208 tokens is 8% of a 1M window; nothing may stop it: {:?}",
        result.err()
    );
    let infos = notices.infos();
    assert!(
        !infos
            .iter()
            .any(|m| m.starts_with("Autocompact: summarized")),
        "a known 1M-window model must not be compacted at 8% full: {infos:?}"
    );
}
