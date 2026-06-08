//! M5.3 — smoke test that BudgetTracker stays useful with no caps and
//! that totals are observable for diagnostics even when nothing blocks.

use wcore_budget::{BudgetCap, BudgetTracker};

#[test]
fn no_caps_accumulates_totals_for_observability() {
    let mut t = BudgetTracker::new(BudgetCap::default());
    for i in 0..5 {
        t.charge("session", 100, 0.01 * (i as f64 + 1.0)).unwrap();
    }
    let (tokens, usd) = t.session_totals("session");
    assert_eq!(tokens, 500);
    // 0.01 + 0.02 + 0.03 + 0.04 + 0.05 = 0.15
    assert!((usd - 0.15).abs() < 1e-9, "expected $0.15, got ${usd}");
}
