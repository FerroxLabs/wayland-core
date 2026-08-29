p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()

def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

sub1("""struct ProviderBudgetReservation {
    owner: ProviderBudgetOwner,
    execution_budget: crate::budget::ExecutionBudgetView,
    reservation: Option<wcore_budget::BudgetReservation>,
    conservative_input_tokens: u64,
    conservative_output_tokens: u64,
    conservative_cost_usd: f64,
}
""",
"""struct ProviderBudgetReservation {
    owner: ProviderBudgetOwner,
    execution_budget: crate::budget::ExecutionBudgetView,
    reservation: Option<wcore_budget::BudgetReservation>,
    conservative_input_tokens: u64,
    conservative_output_tokens: u64,
    conservative_cost_usd: f64,
    audit: SettledDispatchAudit,
}

/// #174 c2 — the task-audit half of a provider reservation.
///
/// Carried on the reservation rather than read from the engine at settle time
/// because two of the four reservation sites live inside the configured-
/// fallback admitter closure, which does not hold `&self`. Binding the audit
/// identity at RESERVE time also means the record names the provider/model the
/// reservation was actually taken against, not whatever the engine moved to
/// afterwards.
struct SettledDispatchAudit {
    auditor: Arc<wcore_budget::SpendAuditor>,
    provider: String,
    model: String,
    /// `conversation` or `configured_fallback`.
    purpose: &'static str,
    /// Whether the reserved cost was a real price. An unpriced dispatch is
    /// recorded as unpriced, never as $0 — the record's `unpriced_dispatches`
    /// count is what tells a reader the total is a floor.
    priced: bool,
}

impl SettledDispatchAudit {
    fn charge(&self, input_tokens: u64, output_tokens: u64, cost_usd: f64) {
        self.auditor.charge(wcore_budget::SpendAuditDispatch {
            provider: self.provider.clone(),
            model: self.model.clone(),
            purpose: self.purpose.to_owned(),
            tokens_in: input_tokens,
            tokens_out: output_tokens,
            cost_usd: self.priced.then_some(cost_usd),
        });
    }
}
""", "reservation-struct")

open(p, 'w').write(s)
print("ok")
