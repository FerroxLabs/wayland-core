//! FerroxLabs/wayland#1266 — the IN-BAND error seam carries a category.
//!
//! #1237 typed the TERMINAL exit of a run (`AgentEngine::run` returning `Err`,
//! classified from the `AgentError` variant). It left `emit_error` — the seam
//! the engine uses ~30 times to tell the user something went wrong WITHOUT
//! returning — taking prose, so an error the engine itself had already
//! classified reached the host as `unknown`.
//!
//! GRADED THROUGH THE PRODUCTION PATH. Every category assertion below comes
//! from a real `AgentEngine::run` over a scripted provider: the engine decides
//! the category at its own call site, hands it to its own `emit_error` funnel,
//! and the sink records what actually arrived. Nothing here calls a classifier
//! helper directly — driving a helper is how this repo's last several vacuous
//! guards happened, and a category that only a helper can produce is not a
//! category the host ever sees.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_protocol::events::FailureCategory;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

// ---------------------------------------------------------------------------
// Scripted provider: one scripted outcome per `stream()` call.
// ---------------------------------------------------------------------------
struct ScriptedProvider {
    script: Mutex<std::collections::VecDeque<Result<Vec<LlmEvent>, ProviderError>>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(failing_tool_call_turn()));
        let events = next?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        ..Default::default()
    }
}

fn end_turn_text(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Capturing sink — records the (message, category) PAIR, which is the whole
// property under test. A sink that recorded only the message would be green
// against the defect.
// ---------------------------------------------------------------------------
#[derive(Default)]
struct CatSink {
    errors: Mutex<Vec<(String, FailureCategory)>>,
}

impl CatSink {
    fn categories(&self) -> Vec<FailureCategory> {
        self.errors.lock().unwrap().iter().map(|e| e.1).collect()
    }
    fn errors(&self) -> Vec<(String, FailureCategory)> {
        self.errors.lock().unwrap().clone()
    }
}

impl OutputSink for CatSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, message: &str, _: bool, category: FailureCategory) {
        self.errors
            .lock()
            .unwrap()
            .push((message.to_string(), category));
    }
    fn emit_info(&self, _: &str) {}
}

/// An ephemeral engine over a scripted provider, with `mutate` applied to the
/// config first so each test can arm the exit it is about.
fn engine_with(
    script: Vec<Result<Vec<LlmEvent>, ProviderError>>,
    mutate: impl FnOnce(&mut wcore_config::config::Config),
) -> (AgentEngine, Arc<CatSink>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(ScriptedProvider {
        script: Mutex::new(script.into_iter().collect()),
        calls,
    });
    let mut config = test_config();
    mutate(&mut config);
    let sink = Arc::new(CatSink::default());
    let engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        sink.clone() as Arc<dyn OutputSink>,
    );
    (engine, sink)
}

// ---------------------------------------------------------------------------
// c1 / c2 — an engine-classified in-band error arrives WITH its category.
// ---------------------------------------------------------------------------

/// c1 + c2 (`context_limit`).
///
/// `[compact] context_window` set below the baseline turn trips
/// `unworkable_window_refusal`, which is an IN-BAND `emit_error` — the run
/// still returns, and before #1266 the host was told about it in prose with
/// `category: unknown`.
///
/// RED ARM (recorded, re-runnable): change that call site in
/// `crates/wcore-agent/src/engine.rs` back to
/// `FailureCategory::Unknown`, `touch` the file, rebuild — this test fails on
/// the category assertion while `the_message_still_reaches_the_user` below
/// stays green, which is the point: the prose was never the missing half.
#[tokio::test]
async fn an_engine_classified_in_band_error_reaches_the_sink_as_context_limit() {
    let (mut engine, sink) = engine_with(vec![Ok(end_turn_text("unreachable"))], |config| {
        // Far below `minimum_workable_window()`: the refusal fires before any
        // provider call, so this test cannot be flaky on the network.
        config.compact.context_window = Some(1_024);
    });
    let _ = engine.run("hello", "msg-1266").await;

    let errors = sink.errors();
    assert!(
        !errors.is_empty(),
        "control: the unworkable-window refusal must reach the sink at all — if \
         this fires the harness stopped exercising the branch and every \
         category assertion below would pass vacuously"
    );
    assert!(
        errors.iter().any(|(m, _)| m.contains("Run stopped")),
        "control: the refusal is the error we captured, not some other one: {errors:?}"
    );
    assert_eq!(
        errors[0].1,
        FailureCategory::ContextLimit,
        "wayland#1266 c1: the engine KNEW this was a context ceiling — it \
         refused precisely because the window is unworkable — and the host was \
         handed `{:?}`",
        errors[0].1
    );
}

/// c2 (`tool_runtime`) — the tool-failure breaker, which is the example
/// #1266's own body gives for "an engine error the engine itself classified".
#[tokio::test]
async fn the_tool_failure_breaker_reaches_the_sink_as_tool_runtime() {
    // The consecutive-failure breaker trips at 10 by default and the
    // no-progress loop guard trips earlier on identical repeats; both are
    // `tool_runtime`. `max_turns` is set well above either so the run cannot
    // end on the turn cap before a breaker fires -- if it did, this test would
    // grade nothing, which the control below catches.
    let script: Vec<Result<Vec<LlmEvent>, ProviderError>> =
        (0..24).map(|_| Ok(failing_tool_call_turn())).collect();
    let (mut engine, sink) = engine_with(script, |config| {
        config.max_turns = Some(40);
    });
    let _ = engine.run("use the tool", "msg-1266").await;

    let errors = sink.errors();
    let breaker = errors
        .iter()
        .find(|(m, _)| m.contains("tool calls failed") || m.contains("no-progress loop"));
    let Some((message, category)) = breaker else {
        panic!(
            "control: no breaker error was captured, so this test would grade \
             nothing. Errors seen: {errors:?}"
        );
    };
    assert_eq!(
        *category,
        FailureCategory::ToolRuntime,
        "wayland#1266 c2: a tool breaker firing is #388's `tool/runtime \
         failure` and the engine knows it at the call site. Message was: \
         {message}"
    );
}

/// c2's CONTROL — a genuinely unclassifiable in-band error is still `unknown`,
/// not given a plausible-looking value.
///
/// The provider failed every attempt with an opaque non-2xx. That is exactly
/// the #1184 split core cannot decide from inside this repo, and #1237 c4
/// already forbids guessing it. If a later change starts "helpfully"
/// classifying these, this reddens.
#[tokio::test]
async fn an_opaque_provider_failure_stays_unknown_in_band() {
    // A 400, not a 503: a client error is not retried, so this exercises the
    // same opaque-upstream exit without waiting out the retry backoff. What
    // makes it the right control is not the status but that core cannot see
    // past it -- the #1184 rate-limit-versus-router split arrives exactly like
    // this, as a non-2xx from a host core has no other view of.
    let script: Vec<Result<Vec<LlmEvent>, ProviderError>> = (0..4)
        .map(|_| {
            Err(ProviderError::Api {
                status: 400,
                message: "{\"error\":{\"message\":\"upstream said no\"}}".to_string(),
            })
        })
        .collect();
    let (mut engine, sink) = engine_with(script, |_| {});
    let _ = engine.run("hello", "msg-1266").await;

    let errors = sink.errors();
    assert!(
        !errors.is_empty(),
        "control: the exhausted-provider path must reach the sink at all"
    );
    for (message, category) in &errors {
        assert_eq!(
            *category,
            FailureCategory::Unknown,
            "wayland#1266 c2 control: core cannot tell a provider rate limit \
             from a router failure (#1184) and must say so rather than pick \
             one. Got {category:?} for: {message}"
        );
    }
}

/// The prose half is UNCHANGED by all of this. Without this, a fix that
/// carried a category by replacing the message would still pass above.
#[tokio::test]
async fn the_message_still_reaches_the_user() {
    let (mut engine, sink) = engine_with(vec![Ok(end_turn_text("unreachable"))], |config| {
        config.compact.context_window = Some(1_024);
    });
    let _ = engine.run("hello", "msg-1266").await;
    assert!(
        sink.errors()
            .iter()
            .any(|(m, _)| m.contains("cannot operate in a window that small")),
        "the category is an ADDITION to the prose, not a replacement for it"
    );
}

/// The alphabet a run can actually put on the wire is not one constant.
///
/// Each test above asserts ONE value; a classifier wired to return that same
/// value everywhere would satisfy each of them in isolation. This asserts the
/// values differ ACROSS runs, which no single-constant implementation can do.
#[tokio::test]
async fn the_in_band_seam_emits_more_than_one_category() {
    let (mut ctx_engine, ctx_sink) = engine_with(vec![Ok(end_turn_text("x"))], |config| {
        config.compact.context_window = Some(1_024);
    });
    let _ = ctx_engine.run("hello", "msg-1266").await;

    let script: Vec<Result<Vec<LlmEvent>, ProviderError>> = (0..4)
        .map(|_| {
            Err(ProviderError::Api {
                status: 400,
                message: "opaque".to_string(),
            })
        })
        .collect();
    let (mut unk_engine, unk_sink) = engine_with(script, |_| {});
    let _ = unk_engine.run("hello", "msg-1266").await;

    let ctx = ctx_sink.categories();
    let unk = unk_sink.categories();
    assert!(
        !ctx.is_empty() && !unk.is_empty(),
        "control: both runs emitted"
    );
    assert_ne!(
        ctx[0], unk[0],
        "two different in-band exits produced the same category, so the seam \
         is carrying a constant rather than a classification"
    );
}

/// A tool-call turn whose tool does not exist, so the call always fails. Eight
/// of these in a row trip the consecutive-failure breaker.
fn failing_tool_call_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-1".into(),
            name: "no_such_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ]
}
