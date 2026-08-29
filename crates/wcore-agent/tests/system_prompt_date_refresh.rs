//! #1168 — the `Current date:` line the engine dispatches must be TODAY'S, on
//! every turn, not the day the engine was constructed.
//!
//! `bootstrap` renders the system prefix once into a plain `String` and hands
//! it to the engine. Nothing re-renders it, so a long-lived engine keeps
//! asserting a date that has since gone stale — while the very next sentence of
//! the same prompt tells the model to treat that date as the authoritative
//! "today" and NOT to substitute a different month or year.
//!
//! Written against the WIRE, not against `context::refresh_current_date`: the
//! unit tests already grade that function, and this project has shipped a
//! fully-tested guard through ungraded call sites before. What is asserted here
//! is what the provider was actually handed.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

/// Captures the `system` string of every request the engine dispatches.
struct SystemCapturingProvider {
    seen: Mutex<Vec<String>>,
}

impl SystemCapturingProvider {
    fn new() -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
        }
    }
    fn systems(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for SystemCapturingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.seen.lock().unwrap().push(request.system.clone());
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _ = tx.send(LlmEvent::TextDelta("ok".into())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                    usage: TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        ..Default::default()
                    },
                })
                .await;
        });
        Ok(rx)
    }
}

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// A prefix built on an earlier day — the state a week-old channel-gateway
/// engine is in — must not be dispatched as-is.
#[tokio::test]
async fn a_stale_baked_date_is_refreshed_before_the_request_goes_out() {
    let stale = "2020-01-01";
    let today = wcore_agent::context::today_string();
    assert_ne!(stale, today, "the fixture date must actually be stale");

    let provider = Arc::new(SystemCapturingProvider::new());
    let mut config = test_config();
    // The shape `build_system_prompt` produces, with the date frozen on a day
    // that has long passed.
    config.system_prompt = Some(format!(
        "You are an AI assistant that can use tools to help with tasks.\n\
         {}\n\
         When constructing time-bound queries, use the current date given above \
         as the authoritative \"today\". Do NOT substitute a different month or \
         year.",
        wcore_agent::context::current_date_block(stale)
    ));

    let mut engine = AgentEngine::new_with_provider(
        provider.clone(),
        config,
        ToolRegistry::new(),
        silent_output(),
    );
    engine.run("what day is it", "m").await.expect("run");

    let systems = provider.systems();
    assert_eq!(systems.len(), 1, "one round-trip was scripted");
    assert!(
        systems[0].contains(&wcore_agent::context::current_date_block(&today)),
        "the dispatched system prompt must carry TODAY'S date; it carried: {}",
        systems[0]
    );
    assert!(
        !systems[0].contains(&wcore_agent::context::current_date_block(stale)),
        "the stale date must be gone from the wire, not merely joined: {}",
        systems[0]
    );
    assert!(
        !engine.system_prompt().contains(stale),
        "the refresh must be persisted on the engine, or it re-renders every turn"
    );
}

/// Control: a prefix already carrying today's date must go out BYTE-IDENTICAL
/// on every turn. If the refresher rewrote it each time it would bust the
/// prompt cache on every round-trip — the failure #559 moved the date into the
/// prefix to avoid.
#[tokio::test]
async fn a_current_date_is_not_rewritten_and_the_prefix_stays_byte_stable() {
    let today = wcore_agent::context::today_string();
    let provider = Arc::new(SystemCapturingProvider::new());
    let mut config = test_config();
    config.system_prompt = Some(format!(
        "You are an AI assistant.\n{}\nDo NOT substitute a different month or year.",
        wcore_agent::context::current_date_block(&today)
    ));
    let before = config.system_prompt.clone().unwrap();

    let mut engine = AgentEngine::new_with_provider(
        provider.clone(),
        config,
        ToolRegistry::new(),
        silent_output(),
    );
    engine.run("turn one", "m").await.expect("run 1");
    engine.run("turn two", "m").await.expect("run 2");

    let systems = provider.systems();
    assert_eq!(systems.len(), 2);
    assert_eq!(
        systems[0], systems[1],
        "the cached system prefix must be byte-identical across turns"
    );
    assert_eq!(
        engine.system_prompt(),
        before,
        "an unchanged day must leave the engine's prefix untouched"
    );
}
