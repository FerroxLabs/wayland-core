//! v0.8.0 Task J — integration tests for AgentBus production wiring.
//!
//! These tests use the same MockLlmProvider as `spawn_test.rs` so we can
//! drive a real `AgentSpawner` through `spawn_one` / `spawn_parallel` /
//! `spawn_fork` and observe `AgentMessage` events on a subscriber.
//!
//! Specifically we verify:
//!   - `Spawned` fires immediately after the spawner is invoked.
//!   - `FirstMessage` carries a UTF-8-safe preview of the prompt.
//!   - `Completed` carries the final turn / token counts on success.
//!   - `Errored` fires when the engine errors instead of `Completed`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{MockLlmProvider, test_config};
use wcore_agent::agents::bus::{AgentBus, AgentMessage};
use wcore_agent::spawner::{AgentSpawner, SubAgentConfig};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wcore_types::spawner::{ForkOverrides, Spawner};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sub_config(name: &str, prompt: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.to_string(),
        prompt: prompt.to_string(),
        max_turns: 5,
        max_tokens: 1024,
        system_prompt: None,
        provider: None,
        model: None,
        temperature: None,
    }
}

fn ok_turn(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

/// Drain a broadcast receiver into a Vec until either `expected` events
/// land or `timeout` expires. Each `recv().await` is wrapped in its own
/// timeout to avoid hanging the test if a publish never fires.
async fn collect_events(
    rx: &mut tokio::sync::broadcast::Receiver<AgentMessage>,
    expected: usize,
    timeout: Duration,
) -> Vec<AgentMessage> {
    let mut out = Vec::new();
    while out.len() < expected {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Ok(msg)) => out.push(msg),
            // Channel closed or timed out — return what we have so
            // assertions can produce useful error messages.
            _ => break,
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Subscribe BEFORE spawn — Spawned + FirstMessage + Completed all
/// arrive on the bus when the sub-agent finishes cleanly.
#[tokio::test]
async fn spawn_one_emits_spawned_first_message_and_completed() {
    let bus = Arc::new(AgentBus::new(64));
    let mut rx = bus.subscribe();

    let provider = Arc::new(MockLlmProvider::with_text_response("done"));
    let spawner = AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus));

    let result = spawner
        .spawn_one(sub_config("alpha", "find all the foos"))
        .await;
    assert!(!result.is_error, "spawn failed: {}", result.text);

    let events = collect_events(&mut rx, 3, Duration::from_secs(2)).await;
    assert_eq!(events.len(), 3, "expected 3 events, got: {:?}", events);

    // Spawned.
    match &events[0] {
        AgentMessage::Spawned {
            agent,
            parent_call_id,
            timestamp_ms,
        } => {
            assert_eq!(agent, "alpha");
            assert!(
                parent_call_id.is_none(),
                "spawn_one path has no parent_call_id"
            );
            assert!(*timestamp_ms > 0, "expected a non-zero timestamp");
        }
        other => panic!("expected Spawned, got {other:?}"),
    }

    // FirstMessage with preview of the prompt.
    match &events[1] {
        AgentMessage::FirstMessage {
            agent,
            content_preview,
        } => {
            assert_eq!(agent, "alpha");
            assert_eq!(content_preview, "find all the foos");
        }
        other => panic!("expected FirstMessage, got {other:?}"),
    }

    // Completed with turn + output token counts. We don't assert on
    // an exact token count — MockLlmProvider's `with_text_response`
    // helper picks its own Done.usage shape, so we just verify the
    // counts came from the engine (turns >= 1, output_tokens > 0).
    match &events[2] {
        AgentMessage::Completed {
            agent,
            turns,
            output_tokens,
        } => {
            assert_eq!(agent, "alpha");
            assert!(*turns >= 1, "expected at least 1 turn, got {turns}");
            assert!(*output_tokens > 0, "expected non-zero output_tokens");
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// Engine-level error -> Errored event instead of Completed.
#[tokio::test]
async fn spawn_one_emits_errored_on_provider_failure() {
    let bus = Arc::new(AgentBus::new(64));
    let mut rx = bus.subscribe();

    // AUDIT E-C2 — a mid-stream error is retried up to the bounded
    // budget (1 + 2); error on every attempt so the run fails hard.
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        vec![LlmEvent::Error("provider blew up".to_string())],
        vec![LlmEvent::Error("provider blew up".to_string())],
        vec![LlmEvent::Error("provider blew up".to_string())],
    ]));
    let spawner = AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus));

    let result = spawner.spawn_one(sub_config("beta", "go boom")).await;
    assert!(result.is_error, "expected is_error, got {}", result.text);

    let events = collect_events(&mut rx, 3, Duration::from_secs(2)).await;
    assert_eq!(events.len(), 3, "expected 3 events, got {events:?}");

    assert!(matches!(events[0], AgentMessage::Spawned { .. }));
    assert!(matches!(events[1], AgentMessage::FirstMessage { .. }));
    match &events[2] {
        AgentMessage::Errored { agent, error } => {
            assert_eq!(agent, "beta");
            assert!(!error.is_empty(), "expected non-empty error message");
        }
        other => panic!("expected Errored, got {other:?}"),
    }
}

/// Subscriber missing (None bus) — production safety regression: bus
/// publishes must not panic / leak even when no bus is attached.
#[tokio::test]
async fn spawn_one_without_bus_does_not_panic() {
    let provider = Arc::new(MockLlmProvider::with_text_response("ok"));
    // Note: no .with_bus(...) — bus is None.
    let spawner = AgentSpawner::new(provider, test_config());

    let result = spawner.spawn_one(sub_config("no-bus", "ignored")).await;
    assert!(!result.is_error);
}

/// Parallel spawn — every sub-agent gets its own lifecycle trio.
#[tokio::test]
async fn spawn_parallel_emits_lifecycle_for_each_child() {
    let bus = Arc::new(AgentBus::new(64));
    let mut rx = bus.subscribe();

    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        ok_turn("a"),
        ok_turn("b"),
        ok_turn("c"),
    ]));
    let spawner = AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus));

    let results = spawner
        .spawn_parallel(vec![
            sub_config("alpha", "x"),
            sub_config("beta", "y"),
            sub_config("gamma", "z"),
        ])
        .await;
    assert_eq!(results.len(), 3);

    // 3 children × 3 lifecycle events = 9 events.
    let events = collect_events(&mut rx, 9, Duration::from_secs(3)).await;
    assert_eq!(events.len(), 9, "expected 9 events, got {events:?}");

    // For each agent name, verify we observed the trio (order across
    // agents is concurrent / nondeterministic, so bucket per agent
    // first then assert on each bucket).
    use std::collections::HashMap;
    let mut by_agent: HashMap<String, Vec<&AgentMessage>> = HashMap::new();
    for ev in &events {
        let agent = match ev {
            AgentMessage::Spawned { agent, .. }
            | AgentMessage::FirstMessage { agent, .. }
            | AgentMessage::Completed { agent, .. }
            | AgentMessage::Errored { agent, .. } => agent.clone(),
            _ => continue,
        };
        by_agent.entry(agent).or_default().push(ev);
    }

    for name in &["alpha", "beta", "gamma"] {
        let trio = by_agent
            .get(*name)
            .unwrap_or_else(|| panic!("no events for {name}"));
        assert_eq!(
            trio.len(),
            3,
            "agent {name} should have 3 events, got: {:?}",
            trio
        );
        assert!(matches!(trio[0], AgentMessage::Spawned { .. }));
        assert!(matches!(trio[1], AgentMessage::FirstMessage { .. }));
        assert!(matches!(trio[2], AgentMessage::Completed { .. }));
    }
}

/// `spawn_fork` (the `Spawner` trait impl) also publishes lifecycle.
#[tokio::test]
async fn spawn_fork_emits_lifecycle() {
    let bus = Arc::new(AgentBus::new(64));
    let mut rx = bus.subscribe();

    let provider = Arc::new(MockLlmProvider::with_text_response("forked"));
    let spawner = AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus));

    let result = spawner
        .spawn_fork(sub_config("forky", "forky task"), ForkOverrides::default())
        .await;
    assert!(!result.is_error);

    let events = collect_events(&mut rx, 3, Duration::from_secs(2)).await;
    assert_eq!(events.len(), 3);
    assert!(matches!(events[0], AgentMessage::Spawned { .. }));
    assert!(matches!(events[1], AgentMessage::FirstMessage { .. }));
    assert!(matches!(events[2], AgentMessage::Completed { .. }));
}

/// FirstMessage preview is char-truncated, not byte-truncated — multi-
/// byte UTF-8 prompts must not panic / split a codepoint.
#[tokio::test]
async fn first_message_preview_is_char_safe() {
    let bus = Arc::new(AgentBus::new(64));
    let mut rx = bus.subscribe();

    let provider = Arc::new(MockLlmProvider::with_text_response("ok"));
    let spawner = AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus));

    // 300 emoji = ~1200 bytes — well past the 200-char preview cap.
    let prompt = "🦀".repeat(300);
    let _ = spawner.spawn_one(sub_config("multibyte", &prompt)).await;

    let events = collect_events(&mut rx, 3, Duration::from_secs(2)).await;
    let first_msg = events
        .iter()
        .find_map(|e| match e {
            AgentMessage::FirstMessage {
                content_preview, ..
            } => Some(content_preview.clone()),
            _ => None,
        })
        .expect("FirstMessage missing from events");
    // 200 chars + ellipsis = 201 chars.
    assert_eq!(first_msg.chars().count(), 201);
    assert!(first_msg.ends_with('…'));
}
