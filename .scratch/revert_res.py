p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()
def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

sub1("""    conservative_cost_usd: f64,
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
""", """    conservative_cost_usd: f64,
}
""", "struct")

sub1("""        conservative_cost_usd: f64,
        audit: SettledDispatchAudit,
    ) -> Self {""", """        conservative_cost_usd: f64,
    ) -> Self {""", "new-sig")

sub1("""            conservative_cost_usd,
            audit,
        }
    }""", """            conservative_cost_usd,
        }
    }""", "new-body")

sub1("""            .expect("provider budget reservation settles exactly once");
        // #174 c2 — every settled provider dispatch is charged to the task's
        // spend audit here, at the ONE place all five settle call sites funnel
        // through, rather than at each of them.
        self.audit.charge(actual_input_tokens, actual_output_tokens, actual_cost_usd);
""", """            .expect("provider budget reservation settles exactly once");
""", "settle")

for label, block in [
    ("primary-durable", """                            reserved_cost,
                            SettledDispatchAudit {
                                auditor: dispatch_auditor.clone(),
                                provider: reservation_provider.to_string(),
                                model: effective_model.clone(),
                                purpose: "conversation",
                                priced: reserved_cost_priced,
                            },
                        )),"""),
]:
    assert s.count(block) == 2, f"{label}: found {s.count(block)}"
    s = s.replace(block, """                            reserved_cost,
                        )),""")

fb = """                                        next_cost_usd,
                                        SettledDispatchAudit {
                                            auditor: auditor_for_fallback.clone(),
                                            provider: next_provider.to_string(),
                                            model: next_model.to_string(),
                                            purpose: "configured_fallback",
                                            priced: next_cost.priced,
                                        },
                                    )),"""
assert s.count(fb) == 2, f"fallback: found {s.count(fb)}"
s = s.replace(fb, """                                        next_cost_usd,
                                    )),""")

sub1("""                // #174 c2 — keep the priced/unpriced verdict alongside the
                // number. `reserved_cost.usd` is a conservative CEILING even
                // when nothing is priced, and recording that ceiling as a cost
                // would report an unpriced call as a known bill.
                let reserved_cost_priced = reserved_cost.priced;
                let reserved_cost = reserved_cost.usd;
                let dispatch_auditor = self.spend_guard.auditor();
""", """                let reserved_cost = reserved_cost.usd;
""", "priced-binding")

sub1("""                let auditor_for_fallback = self.spend_guard.auditor();
                let guard_for_fallback""", """                let guard_for_fallback""", "fallback-auditor")

open(p,'w').write(s)
print('reverted')
