//! W8a A.6 — Bootstrap surfaces a session-root ExecutionBudgetView +
//! cancellation token derived from `Config.budget`.
//!
//! Test scope: validates the conversion path config → ExecutionBudget →
//! BudgetView + linked cancel token, plus that exceeding the cap fires
//! the cancel token. Full end-to-end Bootstrap::build() requires a
//! provider + sink and is out of scope here; that's covered by the
//! existing e2e tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use wcore_agent::budget::ExecutionBudget;
use wcore_agent::cancel::{
    BudgetTripPayload, CancellationToken, budget_linked, budget_linked_with_callback,
};
use wcore_config::budget::BudgetConfig;

#[tokio::test]
async fn bootstrap_root_budget_with_wall_time_trips_cancel() {
    // Same code path the bootstrap.rs A.6 wiring takes.
    let cfg = BudgetConfig {
        max_wall_time_secs: Some(0), // any elapsed time exceeds 0s
        ..Default::default()
    };
    let exec: ExecutionBudget = (&cfg).into();
    let view = exec.start_root();
    let cancel = budget_linked(CancellationToken::new(), view.clone());

    // Wait for the watcher (50ms poll) to observe the trip.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(view.is_exceeded(), "wall-time cap=0 must trip immediately");
    assert!(cancel.is_cancelled(), "linked cancel must fire on trip");
}

#[tokio::test]
async fn bootstrap_root_budget_with_default_config_never_trips() {
    let cfg = BudgetConfig::default();
    let exec: ExecutionBudget = (&cfg).into();
    let view = exec.start_root();
    let cancel = budget_linked(CancellationToken::new(), view.clone());

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!view.is_exceeded(), "no caps = never exceeded");
    assert!(!cancel.is_cancelled(), "linked cancel must not fire");
    // Cancel manually to terminate the watcher.
    cancel.cancel();
}

/// W8a A.7 — `budget_linked_with_callback` fires its callback exactly
/// once with the formatted `(reason, observed, limit)` payload the
/// instant the first cap trips. Bootstrap wires this callback to
/// `OutputSink::emit_budget_exceeded` so the protocol sink emits
/// `BudgetExceeded { reason, observed, limit }` per session.
#[tokio::test]
async fn budget_linked_callback_fires_once_with_payload_on_trip() {
    let cfg = BudgetConfig {
        max_wall_time_secs: Some(0),
        ..Default::default()
    };
    let exec: ExecutionBudget = (&cfg).into();
    let view = exec.start_root();
    let fired = Arc::new(AtomicBool::new(false));
    let payload = Arc::new(Mutex::new(None::<BudgetTripPayload>));
    let fired2 = fired.clone();
    let payload2 = payload.clone();
    let cancel = budget_linked_with_callback(
        CancellationToken::new(),
        view.clone(),
        move |p: BudgetTripPayload| {
            fired2.store(true, Ordering::SeqCst);
            *payload2.lock() = Some(p);
        },
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(cancel.is_cancelled());
    assert!(fired.load(Ordering::SeqCst), "callback must have fired");
    let p = payload.lock().clone().expect("payload captured");
    assert_eq!(p.reason, "max_wall_time");
    // observed is "X.Ys" formatted; limit is "0.0s" from secs=0.
    assert!(p.observed.ends_with('s'));
    assert_eq!(p.limit, "0.0s");
}
