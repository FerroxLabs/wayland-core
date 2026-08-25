//! FerroxLabs/wayland#863 F2 — the runtime collision detector, driven through
//! `AgentEngine::run()`.
//!
//! The wire half of this contract is covered in
//! `wcore-providers/tests/flux_loop_provenance.rs`. This file covers the half
//! that has to CHANGE BEHAVIOUR: when Core declared it owns the climb and the
//! router reports it ran its own server-side Elevation ladder anyway, both
//! ladders climbed the same task, and the candidate that came back is
//! contaminated mid-loop material. It must be dropped, not accepted.
//!
//! Why this matters more than it looks: #247 shipped a "nested-ladder guard"
//! that pushes a warning string onto a `Vec<String>` and blocks nothing. A
//! receipt produced under a doubled ladder is byte-identical in shape to a
//! clean one — the exact "one receipt lying about the other" the contract was
//! written to prevent. A detector that merely logs would repeat that mistake,
//! so every test here asserts on the RESULT of the turn.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{ANVIL_LOOP_OWNER, FluxLoopIntent, LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// Replays a `ProviderMeta` carrying a given `loop_engaged`, then a clean turn.
struct EngagedProvider {
    loop_engaged: Option<String>,
}

#[async_trait]
impl LlmProvider for EngagedProvider {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let events = vec![
            LlmEvent::ProviderMeta {
                routed_model: Some("some-arm".to_string()),
                model_window: None,
                context_pressure: None,
                tokens_counted: None,
                loop_engaged: self.loop_engaged.clone(),
            },
            LlmEvent::TextDelta("candidate".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
            },
        ];
        let (tx, rx) = mpsc::channel(events.len());
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

/// Build an engine over a provider that echoes `engaged`, optionally declaring
/// that Core owns the loop.
fn engine_for(engaged: Option<&str>, owns_loop: bool) -> AgentEngine {
    let provider = Arc::new(EngagedProvider {
        loop_engaged: engaged.map(str::to_string),
    });
    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        silent_output(),
    );
    if owns_loop {
        engine.set_flux_loop_intent(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string()));
    }
    engine
}

/// THE deliverable. Core owns the loop; the router says it ran Elevation.
/// The turn must FAIL — the candidate is dropped, not returned.
#[tokio::test]
async fn elevation_on_a_loop_owned_turn_is_a_hard_fault() {
    let mut engine = engine_for(Some("elevation"), true);
    let result = engine.run("build it", "m1").await;

    let err = result.expect_err(
        "a turn whose candidate was produced under two ladders must not succeed; \
         accepting it is how a receipt ends up lying about the other ladder",
    );
    let text = err.to_string();
    assert!(
        text.contains("loop-ownership collision"),
        "the fault must name the contract it broke, got: {text}"
    );
    assert!(
        text.contains("elevation"),
        "and must report what the router actually ran, got: {text}"
    );
    assert_eq!(
        engine.flux_loop_collisions(),
        1,
        "the collision must be counted, so a receipt can report it"
    );
}

/// Control: the SAME echo on a turn Core does not own is Flux doing its job on
/// its own traffic. It must not fault.
///
/// Without this control the test above would also pass against a detector that
/// simply failed every turn carrying a `ProviderMeta`.
#[tokio::test]
async fn elevation_without_loop_ownership_is_not_a_fault() {
    let mut engine = engine_for(Some("elevation"), false);
    engine
        .run("build it", "m1")
        .await
        .expect("Elevation on unowned traffic is the router's own ladder, not a collision");
    assert_eq!(engine.flux_loop_collisions(), 0);
}

/// Control: `cascade` is explicitly permitted by F1 — a single-tier
/// climb-on-failure, per-request and origin-tier billed. It is NOT a second
/// ladder and must not fault a loop-owned turn.
#[tokio::test]
async fn cascade_on_a_loop_owned_turn_is_not_a_fault() {
    let mut engine = engine_for(Some("cascade"), true);
    engine
        .run("build it", "m1")
        .await
        .expect("F1 permits Cascade on loop-owned traffic");
    assert_eq!(engine.flux_loop_collisions(), 0);
}

/// Control: an absent echo. Every non-Flux endpoint in the workspace sends no
/// `x-flux-loop-engaged` at all, so treating silence as a collision would fail
/// every Anthropic turn an Anvil builder takes.
#[tokio::test]
async fn absent_echo_on_a_loop_owned_turn_is_not_a_fault() {
    let mut engine = engine_for(None, true);
    engine
        .run("build it", "m1")
        .await
        .expect("silence from a non-Flux endpoint must never fault a turn");
    assert_eq!(engine.flux_loop_collisions(), 0);
}

/// An ordinary session engine claims no loop. Only the durable-spawn seam sets
/// the intent, and only for an `Anvil`-origin child — so a plain session turn
/// can never be marked, and can never collide.
#[tokio::test]
async fn a_plain_session_engine_claims_no_loop() {
    let mut engine = engine_for(Some("none"), false);
    engine.run("hello", "m1").await.expect("ordinary turn");
    assert_eq!(engine.flux_loop_collisions(), 0);
}
