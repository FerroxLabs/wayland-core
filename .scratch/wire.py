import re, sys
p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()
orig = s

def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1 occurrence, found {s.count(old)}"
    s = s.replace(old, new, 1)

# ── 1. ProviderBudgetReservation carries the task audit ─────────────────────
sub1("""impl ProviderBudgetReservation {
    fn new(
        owner: ProviderBudgetOwner,
        execution_budget: crate::budget::ExecutionBudgetView,
        reservation: wcore_budget::BudgetReservation,
        conservative_input_tokens: u64,
        conservative_output_tokens: u64,
        conservative_cost_usd: f64,
    ) -> Self {
        Self {
            owner,
            execution_budget,
            reservation: Some(reservation),
            conservative_input_tokens,
            conservative_output_tokens,
            conservative_cost_usd,
        }
    }
""",
"""impl ProviderBudgetReservation {
    fn new(
        owner: ProviderBudgetOwner,
        execution_budget: crate::budget::ExecutionBudgetView,
        reservation: wcore_budget::BudgetReservation,
        conservative_input_tokens: u64,
        conservative_output_tokens: u64,
        conservative_cost_usd: f64,
        audit: SettledDispatchAudit,
    ) -> Self {
        Self {
            owner,
            execution_budget,
            reservation: Some(reservation),
            conservative_input_tokens,
            conservative_output_tokens,
            conservative_cost_usd,
            audit,
        }
    }
""", "reservation-new")

sub1("""    fn settle(
        mut self,
        actual_input_tokens: u64,
        actual_output_tokens: u64,
        actual_cost_usd: f64,
    ) -> Result<(), ProviderBudgetMutationError> {
        let reservation = self
            .reservation
            .take()
            .expect("provider budget reservation settles exactly once");
""",
"""    fn settle(
        mut self,
        actual_input_tokens: u64,
        actual_output_tokens: u64,
        actual_cost_usd: f64,
    ) -> Result<(), ProviderBudgetMutationError> {
        let reservation = self
            .reservation
            .take()
            .expect("provider budget reservation settles exactly once");
        // #174 c2 — every settled provider dispatch is charged to the task's
        // spend audit here, at the ONE place all five settle call sites funnel
        // through, rather than at each of them.
        self.audit.charge(actual_input_tokens, actual_output_tokens, actual_cost_usd);
""", "reservation-settle")

open(p, 'w').write(s)
print("phase 1 ok")
