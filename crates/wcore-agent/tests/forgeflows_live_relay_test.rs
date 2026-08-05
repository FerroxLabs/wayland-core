//! ForgeFlows-Live Phase 1 — regression test for `WorkflowRunner` sub-agent
//! event relay.
//!
//! Before this wiring, `WorkflowRunner::dispatch_agents_via_relay` passed
//! `SpawnExtras::default()`, so every workflow sub-agent ran with a `NullSink`
//! and its events were silently dropped. The runner now accepts a parent
//! `OutputSink` via `with_parent_output`; each dispatched sub-agent gets a
//! per-task `ChannelSink` whose events are wrapped as `SubAgentEvent` and
//! emitted up to the parent — exactly like `SpawnTool::spawn_with_relay`.
//!
//! This test runs a 2-node fan-out workflow against a recording parent sink and
//! asserts BOTH child agents produced `emit_sub_agent_event` calls with DISTINCT
//! `parent_call_id`s. A second test asserts the legacy (no parent) path stays
//! silent so nothing regresses for callers that never wire a parent.

mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{bound_test_spawner, test_config};
use serde_json::Value;
use tokio::sync::mpsc;
use wcore_agent::orchestration::workflow::runner::{WorkflowPlan, WorkflowRunner};
use wcore_agent::output::OutputSink;
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Recording parent `OutputSink` — captures every `emit_sub_agent_event` call
/// as `(parent_call_id, agent_name, inner)` so the test can assert which
/// sub-agents relayed events back. Mirrors `sub_agent_event_emission.rs`'s
/// `Rec`.
#[derive(Default)]
struct Rec {
    sub_events: Mutex<Vec<(String, String, Value)>>,
}

impl OutputSink for Rec {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, _: &str) {}
    fn emit_sub_agent_event(&self, parent_call_id: &str, agent_name: &str, inner: &Value) {
        self.sub_events.lock().unwrap().push((
            parent_call_id.into(),
            agent_name.into(),
            inner.clone(),
        ));
    }
}

/// A provider that emits a short text turn then ends — enough that each
/// sub-agent streams at least one `text_delta` (a relayed stream event) plus a
/// terminal lifecycle `info` event.
struct ChattyProvider;

#[async_trait]
impl LlmProvider for ChattyProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let events = vec![
                LlmEvent::TextDelta("branch output".to_string()),
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
            ];
            for ev in events {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A 2-branch fan-out: each branch is a distinct `AgentCall` node dispatched in
/// one wave through `dispatch_agents_via_relay`. The branch node ids
/// (`judge_a`, `judge_b`) become the relay `parent_call_id`s as
/// `workflow:<node_id>`.
const FANOUT_SRC: &str = r#"
Workflow(
    meta: (name: "relay-fanout", est_agents: 2),
    phases: [Phase(title: "vote", steps: [
        Parallel(id: "tally", branches: [
            (id: "judge_a", prompt: "judge a"),
            (id: "judge_b", prompt: "judge b"),
        ], join: Collect),
    ])],
)
"#;

#[tokio::test]
async fn fanout_relays_both_children_with_distinct_parent_call_ids() {
    let parent = Arc::new(Rec::default());
    let provider = Arc::new(ChattyProvider);
    let (spawner, _session_root) = bound_test_spawner(AgentSpawner::new(provider, test_config()));

    let plan = WorkflowPlan::parse(FANOUT_SRC).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner)
        .with_parent_output(Arc::clone(&parent) as Arc<dyn OutputSink>);
    runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("fan-out workflow should run to completion");

    let events = parent.sub_events.lock().unwrap();

    // Both children relayed at least one event each.
    let ids: HashSet<&str> = events.iter().map(|(id, _, _)| id.as_str()).collect();
    assert!(
        ids.contains("workflow:judge_a"),
        "judge_a must relay sub-agent events; got ids: {ids:?}"
    );
    assert!(
        ids.contains("workflow:judge_b"),
        "judge_b must relay sub-agent events; got ids: {ids:?}"
    );

    // The two children used DISTINCT parent_call_ids (one SubAgentView per child).
    assert_eq!(
        ids.len(),
        2,
        "exactly two distinct parent_call_ids expected; got: {ids:?}"
    );

    // Each child relayed a terminal lifecycle `info` event (the Done signal),
    // proving the lifecycle flush ran, not just the best-effort stream drain.
    for child in ["workflow:judge_a", "workflow:judge_b"] {
        assert!(
            events
                .iter()
                .any(|(id, _, inner)| id == child && inner["type"] == "info"),
            "child {child} must relay a terminal lifecycle info event"
        );
    }
}

/// Regression guard: with NO parent wired, the runner dispatches via the legacy
/// `SpawnExtras::default()` (NullSink) path — the recording sink must see zero
/// `emit_sub_agent_event` calls. This proves the `parent_output: None` branch is
/// behaviourally unchanged.
#[tokio::test]
async fn fanout_without_parent_emits_no_sub_agent_events() {
    let parent = Arc::new(Rec::default());
    let provider = Arc::new(ChattyProvider);
    let (spawner, _session_root) = bound_test_spawner(AgentSpawner::new(provider, test_config()));

    let plan = WorkflowPlan::parse(FANOUT_SRC).expect("workflow should parse");
    // No `with_parent_output` — legacy path.
    let runner = WorkflowRunner::new(&spawner);
    runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("fan-out workflow should run to completion");

    assert!(
        parent.sub_events.lock().unwrap().is_empty(),
        "without a wired parent, no sub-agent events should relay (NullSink path)"
    );
}
