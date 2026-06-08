//! #269 — wire-presence test proving `FleetDispatcher` is reachable from
//! the production `SpawnTool` execute path.
//!
//! Before this wiring, `FleetDispatcher` was pub-exported, fully built,
//! and thoroughly unit-tested in `wcore-swarm` — but had ZERO production
//! callers (audit `WIRING-AUDIT-SUBSTRATE-2026-05-24.md` §T3-A; same
//! "built but never wired" archetype as the v0.8.0/v0.8.1 campaign
//! targets). Every session ran single-agent regardless of topology.
//!
//! The wiring is observed via the `AgentBus` `Spawned` event's
//! `parent_call_id`: when a sub-agent spawns via the Fleet path,
//! `AgentSpawner::spawn_via_fleet` tags it with
//! `"fleet:<run_id>-shard-<i>-<j>"`. The legacy `spawn_parallel` path
//! emits `parent_call_id: None`. So observing a `"fleet:"` prefix
//! anywhere in the bus stream proves the Fleet path executed end-to-end
//! through `SpawnTool::execute`. Deleting the wiring in `spawn_tool.rs`
//! (the `if self.topology == Topology::Fleet { ... }` branch) makes this
//! test fail: every event's `parent_call_id` falls back to `None`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{MockLlmProvider, test_config};
use serde_json::json;
use wcore_agent::agents::bus::{AgentBus, AgentMessage};
use wcore_agent::spawn_tool::SpawnTool;
use wcore_agent::spawner::AgentSpawner;
use wcore_swarm::Topology;
use wcore_tools::Tool;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

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

async fn collect_events(
    rx: &mut tokio::sync::broadcast::Receiver<AgentMessage>,
    expected: usize,
    timeout: Duration,
) -> Vec<AgentMessage> {
    let mut out = Vec::new();
    while out.len() < expected {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Ok(msg)) => out.push(msg),
            _ => break,
        }
    }
    out
}

/// PRIMARY wire-presence proof: invoking `SpawnTool` with
/// `Topology::Fleet` and an 11-task input routes through
/// `FleetDispatcher` end-to-end. At least one `Spawned` event must
/// carry a `parent_call_id` beginning with `"fleet:"`.
///
/// 11 tasks is the smallest input that actually shards
/// (`DEFAULT_SHARD_SIZE = 10`), so this test exercises the cross-shard
/// boundary as well — both shards must publish events with the
/// `fleet:` prefix.
#[tokio::test]
async fn spawn_tool_routes_via_fleet_dispatcher_when_topology_is_fleet() {
    let bus = Arc::new(AgentBus::new(256));
    let mut rx = bus.subscribe();

    // 11 OK-turn responses, one per child sub-agent.
    let turns: Vec<Vec<LlmEvent>> = (0..11).map(|i| ok_turn(&format!("out-{i}"))).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus)));

    // The production wiring point: bootstrap.rs flips topology to Fleet
    // when the loaded agent registry has > DEFAULT_SHARD_SIZE entries.
    // Here we just call `with_topology(Topology::Fleet)` directly —
    // same code path the bootstrap takes.
    let tool = SpawnTool::new(spawner).with_topology(Topology::Fleet);

    // Build 11 anonymous tasks.
    let tasks_json = json!({
        "tasks": (0..11)
            .map(|i| json!({
                "name": format!("task-{i}"),
                "prompt": format!("do thing {i}"),
            }))
            .collect::<Vec<_>>(),
    });

    let result = tool.execute(tasks_json).await;
    assert!(
        !result.is_error,
        "fleet-dispatched SpawnTool must succeed; got: {}",
        result.content
    );

    // Drain the bus. Per spawned sub-agent we expect 3 events
    // (Spawned, FirstMessage, Completed) plus possibly Errored on
    // shutdown — 11 × 3 = 33 minimum.
    let events = collect_events(&mut rx, 33, Duration::from_secs(5)).await;

    // Wire-presence assertion: at least one Spawned event must carry
    // the "fleet:" parent_call_id prefix. Without the wiring, every
    // parent_call_id falls back to None and this assertion fails.
    let fleet_tagged = events
        .iter()
        .filter_map(|ev| match ev {
            AgentMessage::Spawned {
                parent_call_id: Some(pid),
                ..
            } => Some(pid.clone()),
            _ => None,
        })
        .filter(|pid| pid.starts_with("fleet:"))
        .count();

    assert!(
        fleet_tagged >= 11,
        "expected all 11 sub-agents tagged with `fleet:` parent_call_id, \
         got {fleet_tagged}. Without the FleetDispatcher wiring in \
         SpawnTool::execute, parent_call_id would be None across the board. \
         Events seen: {events:#?}"
    );

    // Sharding sanity: with shard size 10 + 11 tasks, we should see
    // both shard-0 and shard-1 represented in the parent_call_ids.
    let pids: Vec<String> = events
        .iter()
        .filter_map(|ev| match ev {
            AgentMessage::Spawned {
                parent_call_id: Some(pid),
                ..
            } => Some(pid.clone()),
            _ => None,
        })
        .collect();
    assert!(
        pids.iter().any(|p| p.contains("shard-0")),
        "expected at least one shard-0 parent_call_id, got: {pids:?}"
    );
    assert!(
        pids.iter().any(|p| p.contains("shard-1")),
        "expected at least one shard-1 parent_call_id, got: {pids:?}"
    );
}

/// NEGATIVE CONTROL: with default `Topology::Spawn`, the Fleet path
/// must NOT fire. Every `Spawned` event's `parent_call_id` should be
/// `None` (the legacy `spawn_parallel` path). This locks in that the
/// Fleet wiring is gated by topology, not unconditionally invoked.
#[tokio::test]
async fn spawn_tool_does_not_route_via_fleet_when_topology_is_spawn() {
    let bus = Arc::new(AgentBus::new(64));
    let mut rx = bus.subscribe();

    let turns: Vec<Vec<LlmEvent>> = (0..3).map(|i| ok_turn(&format!("out-{i}"))).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus)));

    // No with_topology — defaults to Topology::Spawn.
    let tool = SpawnTool::new(spawner);

    let tasks_json = json!({
        "tasks": (0..3)
            .map(|i| json!({
                "name": format!("task-{i}"),
                "prompt": format!("do thing {i}"),
            }))
            .collect::<Vec<_>>(),
    });

    let result = tool.execute(tasks_json).await;
    assert!(!result.is_error, "default-topology SpawnTool must succeed");

    let events = collect_events(&mut rx, 9, Duration::from_secs(3)).await;

    let fleet_tagged = events
        .iter()
        .filter_map(|ev| match ev {
            AgentMessage::Spawned {
                parent_call_id: Some(pid),
                ..
            } => Some(pid.clone()),
            _ => None,
        })
        .filter(|pid| pid.starts_with("fleet:"))
        .count();

    assert_eq!(
        fleet_tagged, 0,
        "default Topology::Spawn must NOT route through FleetDispatcher; \
         saw {fleet_tagged} `fleet:`-tagged events"
    );
}
