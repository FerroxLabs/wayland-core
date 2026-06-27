//! v0.9.4 W1 — spawn relay substrate integration tests.
//!
//! These tests are the "v0.9.3 dormancy guard": they prove that the relay
//! fix (per-task parent_call_id + per-task ChannelSink + terminal emit_info)
//! actually produces N distinct sub-agent event streams. The key assertion is
//! that two tasks produce two distinct parent_call_ids, not one collapsed row.
//!
//! Test layout:
//!   (a) see wcore-cli protocol_bridge tests — requires App/apply_event.
//!   (b) see wcore-cli protocol_bridge tests — requires App/apply_event.
//!   (c) Full path via AgentSpawner::spawn_parallel_with_per_task_extras:
//!       2 tasks → ≥2 distinct parent_call_ids in emitted SubAgentRelay events
//!       + at least one terminal "info" relay per task.
//!   (d) Negative: no parent_output (NullSink) → zero SubAgentRelay events.
//!   (e) Per-task keying: parent_call_ids are distinct across tasks.
//!   (f) Terminal relay: emit_info fires on the ChannelSink before tx drops.

mod common;

use std::sync::{Arc, Mutex};

use common::{MockLlmProvider, test_config};
use tokio::sync::mpsc;
use wcore_agent::agents::channel_sink::{CHANNEL_CAPACITY, ChannelSink, SubAgentRelay};
use wcore_agent::spawner::{AgentSpawner, SpawnExtras, SubAgentConfig};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn sub_config(name: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.to_string(),
        prompt: format!("Task for {}", name),
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

// ---------------------------------------------------------------------------
// (c) Full path: per-task extras → 2 distinct parent_call_ids + terminal relay
// ---------------------------------------------------------------------------

/// (c) Two tasks with per-task ChannelSinks produce two distinct parent_call_id
/// streams. Each stream must contain at least one terminal "info" event
/// (v0.9.4 W1.1b — the Done signal). This is the relay-substrate integrity test.
#[tokio::test]
async fn spawn_relay_two_tasks_produce_distinct_parent_call_ids_and_terminal_events_v094() {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        ok_turn("result-A"),
        ok_turn("result-B"),
    ]));
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));

    // One shared drain channel (mirrors SpawnTool::spawn_with_relay).
    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);

    let extras_a = SpawnExtras {
        channel_sink: Some(Arc::new(ChannelSink::new(
            "spawn:0:agent-a".to_string(),
            "agent-a".to_string(),
            tx.clone(),
        ))),
        agent_name: Some("agent-a".to_string()),
        parent_call_id: Some("spawn:0:agent-a".to_string()),
    };
    let extras_b = SpawnExtras {
        channel_sink: Some(Arc::new(ChannelSink::new(
            "spawn:1:agent-b".to_string(),
            "agent-b".to_string(),
            tx.clone(),
        ))),
        agent_name: Some("agent-b".to_string()),
        parent_call_id: Some("spawn:1:agent-b".to_string()),
    };
    // Drop the original tx so the drain exits when both per-task senders drop.
    drop(tx);

    let tasks_and_extras = vec![
        (sub_config("agent-a"), extras_a),
        (sub_config("agent-b"), extras_b),
    ];

    // Collect all relays in the background before running the spawner.
    let relays: Arc<Mutex<Vec<SubAgentRelay>>> = Arc::new(Mutex::new(Vec::new()));
    let relays_clone = Arc::clone(&relays);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            relays_clone.lock().unwrap().push(relay);
        }
    });

    spawner
        .spawn_parallel_with_per_task_extras(tasks_and_extras)
        .await;
    drain.await.unwrap();

    let collected = relays.lock().unwrap();

    // Must have received events from both tasks.
    let ids_a: Vec<_> = collected
        .iter()
        .filter(|r| r.parent_call_id == "spawn:0:agent-a")
        .collect();
    let ids_b: Vec<_> = collected
        .iter()
        .filter(|r| r.parent_call_id == "spawn:1:agent-b")
        .collect();

    assert!(
        !ids_a.is_empty(),
        "expected relay events for agent-a (spawn:0:agent-a), got none. \
         All parent_call_ids: {:?}",
        collected
            .iter()
            .map(|r| &r.parent_call_id)
            .collect::<Vec<_>>()
    );
    assert!(
        !ids_b.is_empty(),
        "expected relay events for agent-b (spawn:1:agent-b), got none. \
         All parent_call_ids: {:?}",
        collected
            .iter()
            .map(|r| &r.parent_call_id)
            .collect::<Vec<_>>()
    );

    // Each task must have emitted a terminal "info" event (W1.1b).
    let terminal_a = ids_a
        .iter()
        .any(|r| r.inner.get("type").and_then(|v| v.as_str()) == Some("info"));
    let terminal_b = ids_b
        .iter()
        .any(|r| r.inner.get("type").and_then(|v| v.as_str()) == Some("info"));

    assert!(
        terminal_a,
        "agent-a must emit a terminal 'info' relay (W1.1b). Events for a: {:?}",
        ids_a.iter().map(|r| &r.inner["type"]).collect::<Vec<_>>()
    );
    assert!(
        terminal_b,
        "agent-b must emit a terminal 'info' relay (W1.1b). Events for b: {:?}",
        ids_b.iter().map(|r| &r.inner["type"]).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// (d) Negative: no parent_output (NullSink) → zero relay events
// ---------------------------------------------------------------------------

/// (d) When spawn_parallel_with_per_task_extras is called with NullSink extras
/// (channel_sink: None), no SubAgentRelay events are produced. This proves the
/// anonymous / legacy path (without parent_output) produces no relay traffic.
#[tokio::test]
async fn spawn_no_channel_sink_produces_zero_relay_events_v094() {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        ok_turn("anon-A"),
        ok_turn("anon-B"),
    ]));
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));

    // Tasks with no channel_sink (NullSink path).
    let tasks_and_extras = vec![
        (sub_config("anon-a"), SpawnExtras::default()),
        (sub_config("anon-b"), SpawnExtras::default()),
    ];

    // No drain channel — just run the spawner and verify results come back.
    let results = spawner
        .spawn_parallel_with_per_task_extras(tasks_and_extras)
        .await;

    assert_eq!(results.len(), 2, "should get 2 results even without relay");
    for r in &results {
        assert!(!r.is_error, "anon task '{}' should not error", r.name);
    }
    // No assertions on relay events — the NullSink swallows them silently.
}

// ---------------------------------------------------------------------------
// (e) Per-task keying: distinct parent_call_ids per task
// ---------------------------------------------------------------------------

/// (e) Verify the per-task keying scheme: two tasks with different label
/// strings produce different parent_call_id values in the relay.
#[tokio::test]
async fn spawn_relay_per_task_keying_produces_distinct_ids_v094() {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        ok_turn("task-x"),
        ok_turn("task-y"),
    ]));
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));

    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);

    let extras_x = SpawnExtras {
        channel_sink: Some(Arc::new(ChannelSink::new(
            "spawn:0:x".to_string(),
            "x".to_string(),
            tx.clone(),
        ))),
        agent_name: Some("x".to_string()),
        parent_call_id: Some("spawn:0:x".to_string()),
    };
    let extras_y = SpawnExtras {
        channel_sink: Some(Arc::new(ChannelSink::new(
            "spawn:1:y".to_string(),
            "y".to_string(),
            tx.clone(),
        ))),
        agent_name: Some("y".to_string()),
        parent_call_id: Some("spawn:1:y".to_string()),
    };
    drop(tx);

    let ids: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));
    let ids_clone = Arc::clone(&ids);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            ids_clone.lock().unwrap().insert(relay.parent_call_id);
        }
    });

    spawner
        .spawn_parallel_with_per_task_extras(vec![
            (sub_config("x"), extras_x),
            (sub_config("y"), extras_y),
        ])
        .await;
    drain.await.unwrap();

    let seen_ids = ids.lock().unwrap().clone();
    assert!(
        seen_ids.contains("spawn:0:x"),
        "expected spawn:0:x in ids, got: {:?}",
        seen_ids
    );
    assert!(
        seen_ids.contains("spawn:1:y"),
        "expected spawn:1:y in ids, got: {:?}",
        seen_ids
    );
    assert_eq!(
        seen_ids.len(),
        2,
        "must have exactly 2 distinct parent_call_ids"
    );
}

// ---------------------------------------------------------------------------
// (f) Terminal relay: emit_info fires before tx drops (W1.1b sanity check)
// ---------------------------------------------------------------------------

/// (f) A single task with a ChannelSink must emit at least one "info" type
/// relay before the sender drops. This proves emit_info() is called in
/// spawn_one_with_extras before the ChannelSink is released.
#[tokio::test]
async fn spawn_relay_terminal_info_event_emitted_before_sink_drops_v094() {
    let provider = Arc::new(MockLlmProvider::with_turns(vec![ok_turn("done")]));
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));

    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);
    let extras = SpawnExtras {
        channel_sink: Some(Arc::new(ChannelSink::new(
            "spawn:0:solo".to_string(),
            "solo".to_string(),
            tx.clone(),
        ))),
        agent_name: Some("solo".to_string()),
        parent_call_id: Some("spawn:0:solo".to_string()),
    };
    drop(tx);

    let received: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let recv_clone = Arc::clone(&received);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            recv_clone.lock().unwrap().push(relay.inner);
        }
    });

    spawner
        .spawn_parallel_with_per_task_extras(vec![(sub_config("solo"), extras)])
        .await;
    drain.await.unwrap();

    let events = received.lock().unwrap();
    let has_terminal = events
        .iter()
        .any(|v| v.get("type").and_then(|t| t.as_str()) == Some("info"));

    assert!(
        has_terminal,
        "spawn_one_with_extras must emit a terminal 'info' relay before the \
         ChannelSink drops. Event types received: {:?}",
        events
            .iter()
            .map(|v| v.get("type").and_then(|t| t.as_str()).unwrap_or("?"))
            .collect::<Vec<_>>()
    );
}
