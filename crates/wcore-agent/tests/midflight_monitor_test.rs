//! W8b.2.B Task C.4 — `MidFlightMonitor` tests.
//!
//! The monitor watches two signals: budget exhaustion (via
//! `ExecutionBudgetView`) and a sliding window of recent
//! "tool-call errors." It exposes a `MonitorAction` enum that the
//! graph walker (or a future replan loop) consumes.

use wcore_agent::budget::ExecutionBudget;
use wcore_agent::orchestration::monitor::{MidFlightMonitor, MonitorAction};

#[test]
fn monitor_returns_continue_on_clean_state() {
    let view = ExecutionBudget::default().start_root();
    let mut mon = MidFlightMonitor::new(view.clone());
    assert_eq!(mon.tick(), MonitorAction::Continue);
}

#[test]
fn monitor_returns_cancel_on_budget_exceeded() {
    let budget = ExecutionBudget {
        max_tokens_in: Some(10),
        ..ExecutionBudget::default()
    };
    let view = budget.start_root();
    // Push past the cap.
    view.record_tokens(1000, 0);

    let mut mon = MidFlightMonitor::new(view.clone());
    let action = mon.tick();
    assert!(
        matches!(action, MonitorAction::CancelBudget { .. }),
        "expected CancelBudget, got {action:?}"
    );
}

#[test]
fn monitor_requests_replan_after_three_repeated_errors() {
    let view = ExecutionBudget::default().start_root();
    let mut mon = MidFlightMonitor::new(view);
    mon.record_error("Bash exited with code 127");
    mon.record_error("Bash exited with code 127");
    // Two errors so far — not yet replan.
    assert_eq!(mon.tick(), MonitorAction::Continue);
    mon.record_error("Bash exited with code 127");
    assert_eq!(mon.tick(), MonitorAction::ReplanRepeatedError);
}

#[test]
fn monitor_does_not_replan_on_three_distinct_errors() {
    let view = ExecutionBudget::default().start_root();
    let mut mon = MidFlightMonitor::new(view);
    mon.record_error("connection refused");
    mon.record_error("file not found");
    mon.record_error("Bash exited 127");
    assert_eq!(mon.tick(), MonitorAction::Continue);
}

#[test]
fn monitor_window_slides_distinct_signatures() {
    // 3 of same error, then 3 distinct → distinct ones must NOT count
    // toward the repeated-error window.
    let view = ExecutionBudget::default().start_root();
    let mut mon = MidFlightMonitor::new(view);
    mon.record_error("A");
    mon.record_error("B");
    mon.record_error("C");
    // Now another A — only 1 in the recent window of A's.
    mon.record_error("A");
    assert_eq!(mon.tick(), MonitorAction::Continue);
}

#[test]
fn monitor_root_cause_signature_strips_volatile_fields() {
    // Two errors that differ only by a path/byte offset must collapse
    // to the same signature. This is the "same root cause" notion.
    let sig1 =
        MidFlightMonitor::root_cause_signature("ENOENT: /tmp/x/abc-12345.tmp at byte 8192 line 42");
    let sig2 =
        MidFlightMonitor::root_cause_signature("ENOENT: /tmp/x/def-67890.tmp at byte 4096 line 17");
    assert_eq!(sig1, sig2);
}
