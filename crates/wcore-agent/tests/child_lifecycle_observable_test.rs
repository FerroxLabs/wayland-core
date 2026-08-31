//! F21-04-01 regression — per-child lifecycle observables on the host protocol.
//!
//! Phase 21's attribution corpus measured that a host driving Core over the
//! JSON-stream protocol cannot attribute a NESTED lifecycle event to the child
//! that raised it. The cause is not the wire format: `ProtocolEvent::SubAgentEvent`
//! already carries `parent_call_id` + `agent_name` and an opaque `inner`, and the
//! parent already wraps every `SubAgentRelay` in it (`spawn_tool.rs`
//! `spawn_with_relay`). The cause is that `ChannelSink` — the `OutputSink`
//! installed into EVERY spawned child engine (`spawner.rs::execute_resolved_launch`)
//! — implements only a subset of the `OutputSink` surface. Every other method
//! keeps the trait's empty default body, so a child's structured lifecycle event
//! is discarded at the child boundary and never enters the relay at all.
//!
//! Two of those methods are on Success Criterion 2's named lifecycle set and are
//! demonstrably reached by a child engine:
//!
//! * `emit_budget_exceeded` — ten call sites in `engine.rs`, every one on
//!   `self.output`, which for a spawned child IS the `ChannelSink`. It is the
//!   reservation-cap / escalation signal.
//! * `emit_midflight_monitor_decision` — the mid-flight monitor is constructed
//!   unconditionally in `AgentEngine::new_with_provider`, so a child reaches it.
//!   `MonitorDirective::Stop` is the per-child cancellation decision.
//!
//! These tests fail before the repair (zero relays arrive) and pass after it,
//! with each event landing under the sibling that raised it and under no other.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use wcore_agent::agents::channel_sink::{CHANNEL_CAPACITY, ChannelSink, SubAgentRelay};
use wcore_agent::output::OutputSink;
use wcore_protocol::events::{MonitorDirective, MonitorReason};
use wcore_types::message::FinishReason;

/// Minimal parent sink: records exactly what `spawn_with_relay`'s drain task
/// hands to the real `ProtocolSink::emit_sub_agent_event`.
#[derive(Default)]
struct ParentSink {
    sub_events: Mutex<Vec<(String, String, serde_json::Value)>>,
}

impl OutputSink for ParentSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool, _: wcore_protocol::events::FailureCategory) {}
    fn emit_info(&self, _: &str) {}
    fn emit_sub_agent_event(
        &self,
        parent_call_id: &str,
        agent_name: &str,
        inner: &serde_json::Value,
    ) {
        self.sub_events.lock().unwrap().push((
            parent_call_id.into(),
            agent_name.into(),
            inner.clone(),
        ));
    }
}

/// Build the exact parent-side topology `spawn_with_relay` builds: one shared
/// drain channel, one `ChannelSink` per sibling with a distinct
/// `parent_call_id`, and a drain task that wraps each relay via the parent's
/// `emit_sub_agent_event`.
fn siblings(
    names: &[(&str, &str)],
) -> (
    Arc<ParentSink>,
    Vec<ChannelSink>,
    tokio::task::JoinHandle<()>,
) {
    let parent = Arc::new(ParentSink::default());
    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);
    let sinks = names
        .iter()
        .map(|(call_id, agent)| {
            ChannelSink::new((*call_id).to_owned(), (*agent).to_owned(), tx.clone())
        })
        .collect();
    drop(tx);

    let drain_parent = Arc::clone(&parent);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            drain_parent.emit_sub_agent_event(
                &relay.parent_call_id,
                &relay.agent_name,
                &relay.inner,
            );
        }
    });
    (parent, sinks, drain)
}

/// The nested budget-cap event must reach the host tagged with the sibling that
/// raised it — and must never appear under its sibling.
#[tokio::test]
async fn child_budget_exceeded_reaches_the_host_tagged_with_the_raising_sibling() {
    let (parent, sinks, drain) = siblings(&[("spawn:0:alpha", "alpha"), ("spawn:1:beta", "beta")]);

    sinks[0].emit_budget_exceeded("max_tokens_out", "4196", "4150");
    sinks[1].emit_budget_exceeded("max_cost_usd", "0.51", "0.50");

    drop(sinks);
    drain.await.unwrap();

    let events = parent.sub_events.lock().unwrap();
    let budget: Vec<_> = events
        .iter()
        .filter(|(_, _, inner)| inner["type"] == "budget_exceeded")
        .collect();
    assert_eq!(
        budget.len(),
        2,
        "both siblings' budget events must reach the host; got {events:?}"
    );

    let alpha = budget
        .iter()
        .find(|(call_id, _, _)| call_id == "spawn:0:alpha")
        .expect("alpha's budget event must be attributable to alpha");
    assert_eq!(alpha.1, "alpha");
    assert_eq!(alpha.2["reason"], "max_tokens_out");
    assert_eq!(alpha.2["observed"], "4196");
    assert_eq!(alpha.2["limit"], "4150");

    let beta = budget
        .iter()
        .find(|(call_id, _, _)| call_id == "spawn:1:beta")
        .expect("beta's budget event must be attributable to beta");
    assert_eq!(beta.1, "beta");
    assert_eq!(beta.2["reason"], "max_cost_usd");

    // Attribution, not merely presence: neither sibling's cap may be reported
    // under the other's identity.
    assert_ne!(alpha.2["reason"], beta.2["reason"]);
}

/// The mid-flight monitor's stop decision is the per-child cancellation signal.
/// It must be attributable to the child it cancelled.
#[tokio::test]
async fn child_midflight_stop_decision_reaches_the_host_tagged_with_the_child() {
    let (parent, sinks, drain) = siblings(&[("spawn:0:worker", "worker")]);

    sinks[0].emit_midflight_monitor_decision(MonitorDirective::Stop, MonitorReason::BudgetExceeded);

    drop(sinks);
    drain.await.unwrap();

    let events = parent.sub_events.lock().unwrap();
    let decision = events
        .iter()
        .find(|(_, _, inner)| inner["type"] == "mid_flight_monitor_decision")
        .unwrap_or_else(|| {
            panic!("the child's cancellation decision must reach the host; got {events:?}")
        });
    assert_eq!(decision.0, "spawn:0:worker");
    assert_eq!(decision.1, "worker");
    assert_eq!(decision.2["directive"], "stop");
    assert_eq!(decision.2["reason"], "budget_exceeded");
}

/// A chatty child must not be able to crowd its own lifecycle events out in a
/// way that silently rewrites attribution: relays are best-effort by design
/// (`try_send`), so what this pins is that a dropped relay is a dropped relay —
/// it never lands under a different sibling.
#[tokio::test]
async fn relayed_lifecycle_events_keep_their_own_identity_under_backpressure() {
    let parent = Arc::new(ParentSink::default());
    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(2);
    let alpha = ChannelSink::new("spawn:0:alpha".into(), "alpha".into(), tx.clone());
    let beta = ChannelSink::new("spawn:1:beta".into(), "beta".into(), tx);

    alpha.emit_budget_exceeded("max_tokens_out", "1", "0");
    beta.emit_budget_exceeded("max_cost_usd", "1.0", "0.5");
    // Channel is full from here; further relays are shed, never re-tagged.
    alpha.emit_budget_exceeded("max_tokens_in", "9", "8");

    drop(alpha);
    drop(beta);

    let drain_parent = Arc::clone(&parent);
    tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            drain_parent.emit_sub_agent_event(
                &relay.parent_call_id,
                &relay.agent_name,
                &relay.inner,
            );
        }
    })
    .await
    .unwrap();

    let events = parent.sub_events.lock().unwrap();
    for (call_id, agent_name, inner) in events.iter() {
        let expected_agent = match call_id.as_str() {
            "spawn:0:alpha" => "alpha",
            "spawn:1:beta" => "beta",
            other => panic!("unexpected parent_call_id {other}"),
        };
        assert_eq!(agent_name, expected_agent);
        let reason = inner["reason"].as_str().unwrap_or_default();
        let owner_of_reason = match reason {
            "max_tokens_out" | "max_tokens_in" => "spawn:0:alpha",
            "max_cost_usd" => "spawn:1:beta",
            other => panic!("unexpected budget reason {other}"),
        };
        assert_eq!(
            call_id, owner_of_reason,
            "a shed relay must not resurface under the other sibling"
        );
    }
}

/// The repair rides the EXISTING `sub_agent_event` wire shape rather than adding
/// an event type, so it must not require a contract regeneration. This asserts
/// the property that makes that true at the pinned revision: the checked-in
/// `sub_agent_event` branches constrain `inner` only as an open object.
///
/// If a future contract revision closes `inner` (adds `required`, or sets
/// `additionalProperties: false`), this test fails and tells the author that the
/// per-child lifecycle relay now needs a coordinated contract bump.
#[test]
fn pinned_sub_agent_event_contract_admits_a_lifecycle_inner_without_regeneration() {
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../wcore-protocol/contracts/desktop/v1/schema/core-event.schema.json");
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&schema_path).expect("contract schema"))
            .expect("contract schema parses");

    let branches: Vec<&serde_json::Value> = schema["oneOf"]
        .as_array()
        .expect("core-event schema is a oneOf")
        .iter()
        .filter(|branch| branch["properties"]["type"]["const"] == "sub_agent_event")
        .collect();
    assert!(
        !branches.is_empty(),
        "the pinned contract must carry a sub_agent_event branch"
    );

    for branch in branches {
        let inner = &branch["properties"]["inner"];
        assert_eq!(
            inner["additionalProperties"], true,
            "sub_agent_event.inner must stay open for additive child event types"
        );
        assert!(
            inner.get("required").is_none(),
            "sub_agent_event.inner must not pin a required field set"
        );
        assert_eq!(
            inner["properties"]["type"]["type"], "string",
            "sub_agent_event.inner.type must stay an unconstrained string"
        );
    }
}
