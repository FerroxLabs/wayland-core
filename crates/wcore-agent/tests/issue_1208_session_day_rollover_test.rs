//! FerroxLabs/wayland#1208 c2 — a long-lived per-channel engine must not
//! answer date-bound questions with the day the gateway started.
//!
//! `channel_dispatch.rs` keeps "one `AgentEngine` per channel session" in an
//! `Arc<Mutex<HashMap<String, Arc<Mutex<AgentEngine>>>>>` with no eviction
//! (the LRU TODO at `channel_dispatch.rs:30` is unimplemented), and every
//! message for that channel is served by calling `guard.run(&prompt, &msg_id)`
//! on the pooled engine (`channel_dispatch.rs:346`). So the pool contributes
//! exactly one thing to this defect: it makes `AgentEngine::run` outlive the
//! day the prompt was baked on. This test reproduces that shape — one engine
//! behind an `Arc<Mutex<_>>`, several turns, a system prompt baked on an
//! earlier day — and asserts the bytes the PROVIDER is handed, which is the
//! only place the model can read a date from.
//!
//! The stale date is a real one in the past rather than a sentinel, because
//! the thing under test is "the baked value is not today", and a session that
//! booted yesterday is the cheap common case the channel gateway hits every
//! midnight.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::TestSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Records the system prompt of every dispatch and answers with a clean turn.
struct RecordingProvider {
    systems: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.systems
            .lock()
            .expect("recorder mutex")
            .push(request.system.clone());
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = tx.send(LlmEvent::TextDelta("ok".to_string())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage: TokenUsage::default(),
                })
                .await;
        });
        Ok(rx)
    }
}

fn gateway_config(baked_date: &str) -> Config {
    let mut cfg = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "issue-1208-local-unlisted".into(),
        max_tokens: 256,
        max_turns: Some(4),
        // The shape `context::build_system_prompt` bakes at bootstrap: the
        // date line, and the sentence that makes it authoritative.
        system_prompt: Some(format!(
            "You are a test assistant.\n\n{}\nWhen answering, use the current \
             date given above as the authoritative \"today\". Do NOT substitute \
             a different month or year.",
            wcore_agent::context::current_date_block(baked_date)
        )),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    cfg.tools.auto_approve = true;
    cfg.session.enabled = false;
    cfg
}

/// Drive `turns` messages through ONE pooled engine, as the channel gateway
/// does, and return the system prompt each dispatch carried.
async fn pooled_session_systems(baked_date: &str, turns: usize) -> Vec<String> {
    let systems = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        systems: Arc::clone(&systems),
    });
    // The gateway's own storage shape: the engine is owned by the pool, not by
    // the turn, which is the entire reason it outlives its day.
    let pooled = Arc::new(tokio::sync::Mutex::new(AgentEngine::new_with_provider(
        provider,
        gateway_config(baked_date),
        ToolRegistry::new(),
        Arc::new(TestSink::new()) as Arc<dyn OutputSink>,
    )));
    for turn in 0..turns {
        let mut guard = pooled.lock().await;
        guard
            .run(
                &format!("turn {turn}: what is today's date?"),
                &format!("msg-{turn}"),
            )
            .await
            .expect("the recording provider answers cleanly");
    }
    let out = systems.lock().expect("recorder mutex").clone();
    out
}

#[tokio::test]
async fn a_session_that_outlives_its_day_dispatches_todays_date() {
    let today = wcore_agent::context::today_string();
    let stale = "2020-03-01";
    assert_ne!(stale, today, "the fixture date must be a stale one");

    // The gateway booted on a day that is not today and has never restarted.
    let systems = pooled_session_systems(stale, 2).await;
    assert_eq!(systems.len(), 2, "both messages must reach the provider");
    for (turn, system) in systems.iter().enumerate() {
        assert!(
            system.contains(&wcore_agent::context::current_date_block(&today)),
            "turn {turn} must carry the real date; system was: {system}"
        );
        assert!(
            !system.contains(&wcore_agent::context::current_date_block(stale)),
            "turn {turn} still asserts the day the gateway booted: {system}"
        );
        // The refresh must rewrite the value and nothing else: the authority
        // sentence the ticket quotes has to survive, or the fix would have
        // closed the criterion by deleting the instruction instead.
        assert!(
            system.contains("authoritative"),
            "turn {turn} lost the authoritative-date instruction: {system}"
        );
    }
    // Within the day the prompt is byte-stable across turns, so this costs one
    // prefix invalidation per rollover, not one per turn.
    assert_eq!(
        systems[0], systems[1],
        "the refreshed prefix must be identical turn-to-turn"
    );

    // POSITIVE CONTROL — an engine baked TODAY dispatches the same bytes it
    // was built with. Without this, an assertion that "today's date is
    // present" would also pass on a build that unconditionally rewrote the
    // whole prompt, and a stale-date failure could not be told apart from a
    // harness that never produced a date line at all.
    let fresh = pooled_session_systems(&today, 1).await;
    assert!(
        fresh[0].contains(&wcore_agent::context::current_date_block(&today)),
        "control: a same-day session carries today's date: {}",
        fresh[0]
    );
    assert_eq!(
        fresh[0].replacen(&today, stale, 1),
        systems[0].replacen(&today, stale, 1),
        "control: the refreshed and the same-day prompt differ ONLY in the date value"
    );
}
