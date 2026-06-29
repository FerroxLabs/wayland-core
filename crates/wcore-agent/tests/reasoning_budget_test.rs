//! #426 / wayland#422 — the reasoning budget must never starve the visible
//! answer. These drive a real `engine.run()` with extended thinking enabled on
//! an UNKNOWN (router-aliased) model and capture the `LlmRequest` actually sent
//! to the provider, proving the engine's output-sizing + budget-separation hold
//! on the wire (not just in the `size_output_cap` / `fit_thinking_budget` unit
//! tests).

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

/// Minimum visible-output floor the engine reserves (mirrors
/// `engine::MIN_VISIBLE_OUTPUT`, which is private).
const MIN_VISIBLE_OUTPUT: u32 = 4_096;

/// Records the `max_tokens` and `thinking` of the last request streamed, so a
/// test can assert what the engine put on the wire after output sizing.
struct CapturingProvider {
    max_tokens: Mutex<Option<u32>>,
    thinking: Mutex<Option<ThinkingConfig>>,
}

impl CapturingProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            max_tokens: Mutex::new(None),
            thinking: Mutex::new(None),
        })
    }
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        *self.max_tokens.lock().unwrap() = Some(request.max_tokens);
        *self.thinking.lock().unwrap() = request.thinking.clone();
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _ = tx.send(LlmEvent::TextDelta("ok".to_string())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                    usage: TokenUsage::default(),
                })
                .await;
        });
        Ok(rx)
    }
}

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// Build an engine on an unknown/router-aliased model ("flux-auto", which has no
/// known output ceiling) with extended thinking enabled at `budget`, capping the
/// output budget at `max_tokens`.
fn reasoning_engine(max_tokens: u32, budget: u32) -> (AgentEngine, Arc<CapturingProvider>) {
    let provider = CapturingProvider::new();
    let mut config = test_config();
    config.model = "flux-auto".to_string();
    config.max_tokens = max_tokens;
    config.thinking = Some(ThinkingConfig::Enabled {
        budget_tokens: budget,
    });
    let engine = AgentEngine::new_with_provider(
        provider.clone(),
        config,
        ToolRegistry::new(),
        silent_output(),
    );
    (engine, provider)
}

fn captured_budget(provider: &CapturingProvider) -> Option<u32> {
    match provider.thinking.lock().unwrap().clone() {
        Some(ThinkingConfig::Enabled { budget_tokens }) => Some(budget_tokens),
        _ => None,
    }
}

/// The reported bug: a generous default (no host budget) on Flux Auto with heavy
/// thinking. The model must get reasoning headroom (32768, not the 8192 unknown
/// floor) AND keep room for the answer.
#[tokio::test]
async fn flux_auto_thinking_gets_headroom_and_reserves_the_answer() {
    let (mut engine, provider) = reasoning_engine(64_000, 10_000);
    engine.run("hello", "").await.expect("run should succeed");

    let max_tokens = provider
        .max_tokens
        .lock()
        .unwrap()
        .expect("a request was sent");
    let budget = captured_budget(&provider).expect("thinking stayed enabled");

    assert_eq!(
        max_tokens, 32_768,
        "unknown reasoning model must get the reasoning-aware cap, not 8192"
    );
    assert_eq!(
        budget, 10_000,
        "budget fits comfortably, so it is unchanged"
    );
    assert!(
        max_tokens - budget >= MIN_VISIBLE_OUTPUT,
        "the visible answer must keep at least the floor: {max_tokens} - {budget}"
    );
}

/// A low explicit cap must shrink the thinking budget so the answer survives,
/// instead of letting thinking consume the whole budget (the empty-answer bug).
#[tokio::test]
async fn low_cap_shrinks_thinking_to_reserve_the_answer() {
    let (mut engine, provider) = reasoning_engine(8_192, 10_000);
    engine.run("hello", "").await.expect("run should succeed");

    let max_tokens = provider
        .max_tokens
        .lock()
        .unwrap()
        .expect("a request was sent");
    let budget = captured_budget(&provider).expect("thinking stayed enabled");

    assert_eq!(max_tokens, 8_192, "explicit low cap binds");
    assert_eq!(
        budget,
        8_192 - MIN_VISIBLE_OUTPUT,
        "budget shrinks to reserve the visible floor"
    );
    assert!(max_tokens - budget >= MIN_VISIBLE_OUTPUT);
}

/// Boundary: the smallest cap that still keeps thinking on. With
/// MIN_VISIBLE_OUTPUT=4096 and MIN_THINKING_BUDGET=1024, a 5120 cap fits exactly
/// 1024 thinking + 4096 visible.
#[tokio::test]
async fn exact_minimum_cap_keeps_thinking_at_the_floor() {
    let (mut engine, provider) = reasoning_engine(5_120, 10_000);
    engine.run("hello", "").await.expect("run should succeed");

    let max_tokens = provider
        .max_tokens
        .lock()
        .unwrap()
        .expect("a request was sent");
    let budget = captured_budget(&provider).expect("thinking stayed enabled at the boundary");
    assert_eq!(max_tokens, 5_120);
    assert_eq!(budget, 1_024, "exactly the minimum thinking budget fits");
    assert_eq!(
        max_tokens - budget,
        MIN_VISIBLE_OUTPUT,
        "the visible floor is exactly preserved"
    );
}

/// Boundary: one token below the minimum viable cap. The fitted budget would be
/// 1023 (< MIN_THINKING_BUDGET), so thinking is dropped rather than sent invalid.
#[tokio::test]
async fn one_below_minimum_cap_drops_thinking() {
    let (mut engine, provider) = reasoning_engine(5_119, 10_000);
    engine.run("hello", "").await.expect("run should succeed");

    assert_eq!(provider.max_tokens.lock().unwrap().unwrap(), 5_119);
    assert!(
        matches!(
            provider.thinking.lock().unwrap().clone(),
            Some(ThinkingConfig::Disabled)
        ),
        "5119 leaves only 1023 budget after the visible floor; thinking must drop"
    );
}

/// When the cap is too small to hold any usable reasoning budget plus the floor,
/// thinking is dropped entirely so the full budget goes to the visible answer.
#[tokio::test]
async fn tiny_cap_drops_thinking_so_the_answer_is_never_empty() {
    let (mut engine, provider) = reasoning_engine(5_000, 10_000);
    engine.run("hello", "").await.expect("run should succeed");

    let max_tokens = provider
        .max_tokens
        .lock()
        .unwrap()
        .expect("a request was sent");
    assert_eq!(max_tokens, 5_000, "tiny cap binds");
    assert!(
        matches!(
            provider.thinking.lock().unwrap().clone(),
            Some(ThinkingConfig::Disabled)
        ),
        "thinking must be dropped when it cannot fit alongside the visible floor"
    );
}
