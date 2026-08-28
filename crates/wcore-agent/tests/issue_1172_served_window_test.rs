//! FerroxLabs/wayland#1172 — a self-hosted endpoint that DISCARDS part of the
//! prompt must not be invisible.
//!
//! Measured on hetzner against a real `qwen3:8b` on stock Ollama, driven
//! through a logging reverse proxy (`/root/w3/proxylog4`). The served slot was
//! 4,096 (`ollama ps`; the journal logged `n_ctx_slot = 4096, n_keep = 4`)
//! while the model ADVERTISES 40,960. On the final turn Core sent ~10,466
//! estimated tokens, Ollama answered `"usage":{"prompt_tokens":4095}` and
//! logged `truncated = 1` — it kept 4 tokens of the head and threw away the
//! system prompt and the user's task first. Core said nothing, and its own
//! window gauge reported single-digit pressure.
//!
//! #1150 (in this base) lowered the unknown-model assumption from 200,000 to
//! 32,768. That is still 8x the served 4,096, so the truncation survives it.
//!
//! WHY THIS IS NOT A PROBE. Two earlier attempts reached for the served figure
//! by asking the endpoint (`/api/ps`) and both were backed out: probing means
//! deciding WHICH endpoints to probe, and every mock server in this workspace
//! binds `127.0.0.1`, so "the endpoint is loopback" cannot separate a real
//! self-hosted server from a test fixture. The signal used here is already in
//! the response we receive — `usage.prompt_tokens` against what we sent — so
//! there is no extra I/O and nothing to gate on an address.
//!
//! HOW THIS FAILS IF THE DEFECT RETURNS. Delete the
//! `ServedWindowTracker::observe` call in `AgentEngine`'s post-turn usage
//! accounting and `truncating_endpoint_is_named_to_the_user` goes red on the
//! notice; delete the served-window clamp in `active_window_percent_now` and
//! it goes red on the gauge. `a_full_service_endpoint_is_left_alone` is the
//! control: without it, a detector that simply fired every turn would pass.

mod common;

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockTool, test_config};

/// The served slot measured on the reproduction box.
const SERVED_SLOT: u64 = 4_096;

/// The ratio between what the endpoint reports and what Core estimates it
/// sent, on turns the endpoint served IN FULL. Measured across the 24
/// full-service turns of the #1172 corpus: 0.839..0.902 — Core's `char/4`
/// estimator runs consistently ~15% high. 0.87 is the middle of that band.
const HEALTHY_REPORT_RATIO: f64 = 0.87;

/// A tool result big enough that the next turn's prompt clearly exceeds a
/// 4,096-token slot, and small enough to stay under the #255 pre-flight
/// ceiling so this test measures the window gauge and not the overflow guard.
const BIG_RESULT_CHARS: usize = 32_000;

/// An endpoint with a fixed serving slot.
///
/// Reports usage the way a real OpenAI-compatible server does: `prompt_tokens`
/// is what it ACTUALLY processed. With `slot: None` it processes everything it
/// is given (the control). With `slot: Some(n)` it silently discards the head
/// down to `n` — stock Ollama's behaviour, and the whole defect.
///
/// It calibrates off `client_context_tokens`, the engine's own assembled-prompt
/// estimate which is already threaded onto every request, so the arms stay
/// faithful to the measured ratio without this test having to re-derive Core's
/// estimator.
struct SlotProvider {
    slot: Option<u64>,
    calls: Mutex<usize>,
}

impl SlotProvider {
    fn new(slot: Option<u64>) -> Self {
        Self {
            slot,
            calls: Mutex::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for SlotProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let call = {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
            *c
        };
        let sent = request.client_context_tokens.unwrap_or(0);
        let served = (sent as f64 * HEALTHY_REPORT_RATIO) as u64;
        let reported = match self.slot {
            Some(slot) => served.min(slot),
            None => served,
        };
        let usage = TokenUsage {
            input_tokens: reported,
            output_tokens: 32,
            ..Default::default()
        };
        // Turn 1 calls the tool, so turn 2's prompt carries its output; turn 2
        // answers and ends the run.
        let events = if call == 1 {
            vec![
                LlmEvent::ToolUse {
                    id: "call-1".to_string(),
                    name: "bigread".to_string(),
                    input: serde_json::json!({}),
                    extra: None,
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
                    usage,
                },
            ]
        } else {
            vec![
                LlmEvent::TextDelta("done".to_string()),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                    usage,
                },
            ]
        };
        let (tx, rx) = mpsc::channel(events.len().max(1));
        tokio::spawn(async move {
            for e in events {
                if tx.send(e).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

/// Records what the user is actually TOLD. `tracing::warn!` is not a surface:
/// with `RUST_LOG` unset only `ERROR` reaches stderr, so a log line here would
/// reach nobody (#1130).
#[derive(Default)]
struct NoticeSink {
    infos: Mutex<Vec<String>>,
}

impl NoticeSink {
    fn matching(&self, needle: &str) -> Vec<String> {
        self.infos
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.to_lowercase().contains(needle))
            .cloned()
            .collect()
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
        finish: FinishReason,
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

fn engine_over(slot: Option<u64>) -> (AgentEngine, Arc<NoticeSink>) {
    let sink = Arc::new(NoticeSink::default());
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(MockTool::new(
        "bigread",
        &"x".repeat(BIG_RESULT_CHARS),
        false,
    )));
    let engine = AgentEngine::new_with_provider(
        Arc::new(SlotProvider::new(slot)),
        test_config(),
        tools,
        sink.clone() as Arc<dyn OutputSink>,
    );
    (engine, sink)
}

/// THE deliverable, half one. The endpoint served 4,096 of the ~8,000 tokens
/// it was given and said so in `usage`. Core must tell the user, on the user's
/// surface, naming the served figure it discarded down to.
///
/// `tracing::warn!` cannot close this: with `RUST_LOG` unset only `ERROR`
/// reaches stderr, so a log line here reaches nobody.
#[tokio::test]
async fn truncating_endpoint_is_named_to_the_user() {
    let (mut engine, sink) = engine_over(Some(SERVED_SLOT));
    engine
        .run("read the file and summarize it", "m1")
        .await
        .expect(
            "the run itself completes - the endpoint answers, it just answers from a \
             context it no longer has, which is exactly why this is invisible today",
        );

    let infos = sink.infos.lock().unwrap().clone();
    assert!(
        infos
            .iter()
            .any(|m| m.contains("4096") || m.contains("4,096")),
        "the user must be told the endpoint discarded part of the prompt, and told the \
         served figure it discarded down to; got infos: {infos:?}"
    );
}

/// THE deliverable, half two. Once the served window has been OBSERVED it is
/// knowledge, not a guess - stronger evidence about what this endpoint will
/// accept than any table or config default, because it is what the endpoint
/// actually did. The gauge must use it.
///
/// #1172's headline number is this one: a turn that had overflowed the served
/// slot ~2.5x reported `active_window_percent: 6`.
#[tokio::test]
async fn an_observed_served_window_becomes_the_gauge_denominator() {
    let (mut engine, _sink) = engine_over(Some(SERVED_SLOT));
    let result = engine
        .run("read the file and summarize it", "m1")
        .await
        .expect("the endpoint answers");

    let percent = result.active_window_percent.expect(
        "the model is unlisted, so the kernel has no window - but the endpoint has now \
         DEMONSTRATED one, and a demonstrated window is not a fabricated denominator",
    );
    assert!(
        percent > 100,
        "the prompt was ~2x the served slot; a gauge reporting {percent}% is the 48x \
         over-estimate #1172 reports, in miniature"
    );
}

/// CONTROL. The same conversation, the same sizes, against an endpoint that
/// serves the whole prompt. Nothing may be said and the gauge must stay as the
/// kernel resolved it.
///
/// Without this, a detector that fired on every turn would pass the test above.
#[tokio::test]
async fn a_full_service_endpoint_is_left_alone() {
    let (mut engine, sink) = engine_over(None);
    let result = engine
        .run("read the file and summarize it", "m1")
        .await
        .expect("full-service endpoint");

    assert!(
        sink.matching("discard").is_empty() && sink.matching("truncat").is_empty(),
        "an endpoint that processed everything it was sent must not be accused of \
         truncating; got infos: {:?}",
        sink.infos.lock().unwrap()
    );
    assert_eq!(
        result.active_window_percent, None,
        "with no observation and an unlisted model the window is genuinely unknown; \
         fabricating a denominator here is the #1150 defect"
    );
}
