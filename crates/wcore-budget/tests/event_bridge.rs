//! M5.3 — verify the BudgetEvent → SpanSink bridge is wired correctly.
//!
//! This test lives in wcore-budget rather than wcore-observability so it
//! doesn't pull `wcore-observability` into wcore-budget's dep graph; the
//! bridge is reconstructed inline as a minimal `BudgetEventSink` that
//! captures serialized JSON. Production uses
//! `wcore_observability::sink::ObservabilityBudgetEventBridge`, exercised
//! transitively by the workspace build.

use std::sync::{Arc, Mutex};

use wcore_budget::{BudgetCap, BudgetEvent, BudgetEventSink, BudgetTracker};

#[derive(Default)]
struct JsonCapture {
    payloads: Mutex<Vec<serde_json::Value>>,
}
impl BudgetEventSink for JsonCapture {
    fn emit(&self, event: &BudgetEvent) {
        if let Ok(v) = serde_json::to_value(event) {
            self.payloads.lock().unwrap().push(v);
        }
    }
}

#[test]
fn charge_event_serializes_to_expected_json_shape() {
    let sink = Arc::new(JsonCapture::default());
    let mut t = BudgetTracker::new(BudgetCap::default());
    t.set_event_sink(sink.clone());

    t.charge("sess-1", 250, 0.0125).unwrap();

    let payloads = sink.payloads.lock().unwrap();
    assert_eq!(payloads.len(), 1);
    let v = &payloads[0];
    assert_eq!(v["kind"], "charge");
    assert_eq!(v["session_id"], "sess-1");
    assert_eq!(v["tokens"], 250);
    assert!((v["usd"].as_f64().unwrap() - 0.0125).abs() < 1e-9);
}

#[test]
fn cap_block_serializes_with_reason_payload() {
    let sink = Arc::new(JsonCapture::default());
    let cap = BudgetCap::builder().per_session_usd(0.01).build();
    let mut t = BudgetTracker::new(cap);
    t.set_event_sink(sink.clone());

    let _ = t.charge("sess-1", 0, 0.05);

    let payloads = sink.payloads.lock().unwrap();
    let block = payloads
        .iter()
        .find(|v| v["kind"] == "cap_block")
        .expect("cap_block payload must be emitted");
    assert_eq!(block["session_id"], "sess-1");
    assert_eq!(block["reason"]["CapExceeded"]["kind"], "per_session_usd");
}
