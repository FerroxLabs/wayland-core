//! #559 — the engine must TELL THE USER when a warm session is being served
//! no prompt cache at all.
//!
//! The detector half of this is unit-graded in `cache_diagnostics`. This
//! grades the wiring, which is where the original defect actually bit: the
//! engine had exactly one consumer of `check_cache_health`, and everything it
//! produced went to `tracing::warn!`. With `RUST_LOG` unset only ERROR reaches
//! stderr, so the reported session ran 26 turns and 77.7M input tokens with a
//! completely dead cache and nothing on screen.
//!
//! Both directions are graded here: a route that declares a prompt cache must
//! produce the notice, and a route that does not must stay silent.

mod common;

use std::sync::{Arc, Mutex};

use serde_json::json;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

/// Records every `emit_info` line.
struct InfoSink {
    inner: Arc<TerminalSink>,
    lines: Arc<Mutex<Vec<String>>>,
}

impl InfoSink {
    fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                inner: Arc::new(TerminalSink::new(true)),
                lines: lines.clone(),
            },
            lines,
        )
    }
}

impl OutputSink for InfoSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.inner.emit_text_delta(text, msg_id);
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.inner.emit_thinking(text, msg_id);
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        self.inner.emit_tool_call(name, input);
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        self.inner.emit_tool_result(name, is_error, content);
    }
    fn emit_stream_start(&self, msg_id: &str) {
        self.inner.emit_stream_start(msg_id);
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
        finish: FinishReason,
    ) {
        self.inner.emit_stream_end(
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
        self.inner.emit_error(msg, retryable);
    }
    fn emit_info(&self, msg: &str) {
        if let Ok(mut g) = self.lines.lock() {
            g.push(msg.to_string());
        }
    }
}

/// One tool round trip reporting a large, entirely uncached input — the
/// #559 shape (`cache_read = 0`, `cache_creation = 0`, every turn).
fn uncached_tool_turn(id: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: id.into(),
            name: "mock_tool".into(),
            input: json!({ "path": "/etc/hosts" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage {
                input_tokens: 50_000,
                output_tokens: 40,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

fn uncached_final_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("done".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 50_000,
                output_tokens: 10,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

/// Drive 4 uncached round trips on `compat` and return every `emit_info` line.
async fn run_uncached_session(compat: wcore_config::compat::ProviderCompat) -> Vec<String> {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        uncached_tool_turn("tu_1"),
        uncached_tool_turn("tu_2"),
        uncached_tool_turn("tu_3"),
        uncached_final_turn(),
    ]));
    let mut config = test_config();
    config.compat = compat;
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "ok", false)));

    let (sink, lines) = InfoSink::new();
    let output: Arc<dyn OutputSink> = Arc::new(sink);
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("burn some tokens", "m-1")
        .await
        .expect("engine ok");
    assert_eq!(result.turns, 4, "harness precondition: 4 round trips");
    lines.lock().unwrap().clone()
}

fn dead_cache_lines(lines: &[String]) -> Vec<&String> {
    lines
        .iter()
        .filter(|l| l.contains("Prompt cache is not being served"))
        .collect()
}

/// A route that declares a prompt cache, served none of it across a warm
/// session, must say so — once.
#[tokio::test]
async fn dead_prompt_cache_is_reported_to_the_user() {
    let compat = wcore_config::compat::ProviderCompat::anthropic_defaults();
    assert!(
        compat.prompt_cache_expected(),
        "harness precondition: this preset declares a prompt cache"
    );
    let lines = run_uncached_session(compat).await;
    let hits = dead_cache_lines(&lines);
    assert_eq!(
        hits.len(),
        1,
        "4 round trips at 50k uncached input each on a caching route must \
         produce exactly one dead-cache notice (latched), saw {hits:?} in \
         all lines {lines:?}"
    );
    assert!(
        hits[0].contains("re-billing the full context"),
        "the notice must say what it costs, saw {:?}",
        hits[0]
    );
}

/// The other direction: identical traffic on a route with no declared prompt
/// cache must stay silent. Without this the fix would accuse every
/// OpenAI-compatible endpoint that simply has no cache.
#[tokio::test]
async fn no_dead_cache_notice_when_the_route_declares_no_cache() {
    let compat = wcore_config::compat::ProviderCompat {
        prompt_cache_expected: None,
        ..wcore_config::compat::ProviderCompat::anthropic_defaults()
    };
    assert!(
        !compat.prompt_cache_expected(),
        "harness precondition: this route declares no prompt cache"
    );
    let lines = run_uncached_session(compat).await;
    assert!(
        dead_cache_lines(&lines).is_empty(),
        "a route with no declared prompt cache must not be accused of a \
         broken one, saw {lines:?}"
    );
}
