//! M5.3 step 8 — per-user daily USD cap enforcement.

use chrono::Utc;
use wcore_budget::{BudgetCap, BudgetError, BudgetTracker};

#[test]
fn per_user_daily_cap_aggregates_across_sessions() {
    let cap = BudgetCap::builder().per_user_daily_usd(0.10).build();
    let mut t = BudgetTracker::new(cap);
    let now = Utc::now();
    // Two sessions, same user, total $0.09 — under cap.
    t.charge_for_user_at("sess-a", "user-1", 100, 0.05, now)
        .unwrap();
    t.charge_for_user_at("sess-b", "user-1", 100, 0.04, now)
        .unwrap();
    // Third charge would push the user-day total to $0.11 — blocked.
    let err = t
        .charge_for_user_at("sess-c", "user-1", 100, 0.02, now)
        .unwrap_err();
    match err {
        BudgetError::CapExceeded { kind, .. } => assert_eq!(kind, "per_user_daily_usd"),
    }
    // A different user is unaffected.
    t.charge_for_user_at("sess-d", "user-2", 100, 0.09, now)
        .unwrap();
}

#[test]
fn per_user_block_preserves_session_bucket_invariant() {
    let cap = BudgetCap::builder().per_user_daily_usd(0.01).build();
    let mut t = BudgetTracker::new(cap);
    let now = Utc::now();
    let _ = t.charge_for_user_at("session", "user", 100, 0.05, now);
    // Daily cap rejected → session bucket must be 0, not 0.05.
    assert_eq!(t.session_totals("session"), (0, 0.0));
}
