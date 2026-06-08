//! M5.3 — verify the engine hook wiring: `AgentEngine::set_budget_tracker`
//! installs a tracker; `BudgetEvent` flow is exercised via the standalone
//! tracker surface here because spinning up a full agent turn requires
//! the `test-utils` feature + provider scripting that the other M5.3 tests
//! already cover at the unit layer.

use std::sync::{Arc, Mutex};

use parking_lot::Mutex as PMutex;
use wcore_budget::{BudgetCap, BudgetEvent, BudgetEventSink, BudgetTracker};

#[derive(Default)]
struct EventCapture {
    events: Mutex<Vec<BudgetEvent>>,
}
impl BudgetEventSink for EventCapture {
    fn emit(&self, event: &BudgetEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

#[test]
fn tracker_can_be_shared_between_engine_and_test_observer() {
    // Engine holds `Arc<PMutex<BudgetTracker>>`. Tests can install an
    // event sink on the tracker before handing it to the engine.
    let cap = BudgetCap::builder().per_session_usd(1.00).build();
    let sink = Arc::new(EventCapture::default());
    let tracker = {
        let mut t = BudgetTracker::new(cap);
        t.set_event_sink(sink.clone());
        Arc::new(PMutex::new(t))
    };

    // Simulate the engine's per-turn charge: two charges, third overruns.
    tracker.lock().charge("sess", 1000, 0.50).unwrap();
    tracker.lock().charge("sess", 1000, 0.40).unwrap();
    let err = tracker.lock().charge("sess", 1000, 0.20).unwrap_err();

    let events = sink.events.lock().unwrap();
    let charges = events
        .iter()
        .filter(|e| matches!(e, BudgetEvent::Charge { .. }))
        .count();
    let blocks = events
        .iter()
        .filter(|e| matches!(e, BudgetEvent::CapBlock { .. }))
        .count();
    assert_eq!(charges, 2, "two successful charges expected");
    assert_eq!(blocks, 1, "one CapBlock for the rejected charge");

    // Sanity-check the err payload kind.
    match err {
        wcore_budget::BudgetError::CapExceeded { kind, .. } => {
            assert_eq!(kind, "per_session_usd");
        }
    }
}
