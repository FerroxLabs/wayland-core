//! M5.3 — per-session USD cap enforcement (TDD red).
//!
//! This is the first failing test that drives the `BudgetTracker` +
//! `BudgetCap` extraction. The plan body in
//! `docs/superpowers/plans/milestone-5-productize-multi-agent.md` lines
//! 215–229 is the literal source.

use wcore_budget::{BudgetCap, BudgetError, BudgetTracker};

#[test]
fn per_session_usd_cap_blocks_overrun() {
    let cap = BudgetCap::builder().per_session_usd(0.10).build();
    let mut t = BudgetTracker::new(cap);
    let sid = "test-session";
    t.charge(sid, /* tokens */ 1000, /* usd */ 0.05).unwrap();
    t.charge(sid, /* tokens */ 1000, /* usd */ 0.04).unwrap();
    let err = t.charge(sid, 1000, 0.05).unwrap_err();
    assert!(matches!(err, BudgetError::CapExceeded { .. }));
}
