//! W7 F2-3.5 integration test: sub-agent event relay through ChannelSink.
//!
//! Constructs a `ChannelSink`, simulates a sub-agent emitting a few events,
//! and asserts they arrive at the parent's `OutputSink::emit_sub_agent_event`
//! after passing through the mpsc drain task — mimicking the SpawnTool
//! wiring that lands in F2-3.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use wcore_agent::agents::channel_sink::{ChannelSink, SubAgentRelay};
use wcore_agent::output::OutputSink;
use wcore_types::message::FinishReason;

#[derive(Default)]
struct Rec {
    sub_events: Mutex<Vec<(String, String, serde_json::Value)>>,
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

#[tokio::test]
async fn sub_agent_text_deltas_arrive_via_sub_agent_event() {
    let parent = Arc::new(Rec::default());
    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(256);
    let sink = ChannelSink::new("c-1".into(), "reviewer".into(), tx);

    // Drain task: route relays through the parent OutputSink, mimicking
    // the SpawnTool wiring that lands in W7.
    let parent_clone = Arc::clone(&parent);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            parent_clone.emit_sub_agent_event(
                &relay.parent_call_id,
                &relay.agent_name,
                &relay.inner,
            );
        }
    });

    sink.emit_text_delta("step 1", "m-sub-1");
    sink.emit_text_delta("step 2", "m-sub-1");
    drop(sink); // closes channel so drain task exits
    drain.await.unwrap();

    let events = parent.sub_events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "c-1");
    assert_eq!(events[0].1, "reviewer");
    assert_eq!(events[0].2["type"], "text_delta");
    assert_eq!(events[0].2["text"], "step 1");
    assert_eq!(events[1].2["text"], "step 2");
}

#[tokio::test]
async fn sub_agent_thinking_relays_with_tag() {
    let parent = Arc::new(Rec::default());
    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(256);
    let sink = ChannelSink::new("c-2".into(), "researcher".into(), tx);

    let parent_clone = Arc::clone(&parent);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            parent_clone.emit_sub_agent_event(
                &relay.parent_call_id,
                &relay.agent_name,
                &relay.inner,
            );
        }
    });

    sink.emit_thinking("hmm", "m-sub-2");
    drop(sink);
    drain.await.unwrap();

    let events = parent.sub_events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].1, "researcher");
    assert_eq!(events[0].2["type"], "thinking");
    assert_eq!(events[0].2["text"], "hmm");
}
