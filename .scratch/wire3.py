p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()

def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

# ── primary dispatch, durable authority ─────────────────────────────────────
sub1("""                            run_budget.clone(),
                            reservation,
                            reserved_input,
                            reserved_output,
                            reserved_cost,
                        )),
                        Err(wcore_budget::BudgetError::CapExceeded {
                            kind,
                            limit,
                            observed,
                        }) => {
                            self.output.emit_budget_exceeded(&kind, &observed, &limit);
                            self.emit_error(
                                &format!(
                                    "Provider call not started: budget cap '{kind}' would be exceeded \\
                                     (limit {limit}, reserved total {observed}). Continue with \\
                                     additional budget to authorize more work."
                                ),
                                false,
                            );
                            return self
                                .finish_run_terminated(user_input, turn, FinishReason::Length)
                                .await;
                        }
                    }
                } else if let Some(tracker) = self.budget_tracker.clone() {""",
"""                            run_budget.clone(),
                            reservation,
                            reserved_input,
                            reserved_output,
                            reserved_cost,
                            SettledDispatchAudit {
                                auditor: dispatch_auditor.clone(),
                                provider: reservation_provider.to_string(),
                                model: effective_model.clone(),
                                purpose: "conversation",
                                priced: reserved_cost_priced,
                            },
                        )),
                        Err(wcore_budget::BudgetError::CapExceeded {
                            kind,
                            limit,
                            observed,
                        }) => {
                            self.output.emit_budget_exceeded(&kind, &observed, &limit);
                            self.emit_error(
                                &format!(
                                    "Provider call not started: budget cap '{kind}' would be exceeded \\
                                     (limit {limit}, reserved total {observed}). Continue with \\
                                     additional budget to authorize more work."
                                ),
                                false,
                            );
                            return self
                                .finish_run_terminated(user_input, turn, FinishReason::Length)
                                .await;
                        }
                    }
                } else if let Some(tracker) = self.budget_tracker.clone() {""",
     "primary-durable")

# ── primary dispatch, legacy tracker ────────────────────────────────────────
sub1("""                        Ok(reservation) => Some(ProviderBudgetReservation::new(
                            ProviderBudgetOwner::Legacy(tracker),
                            run_budget.clone(),
                            reservation,
                            reserved_input,
                            reserved_output,
                            reserved_cost,
                        )),""",
"""                        Ok(reservation) => Some(ProviderBudgetReservation::new(
                            ProviderBudgetOwner::Legacy(tracker),
                            run_budget.clone(),
                            reservation,
                            reserved_input,
                            reserved_output,
                            reserved_cost,
                            SettledDispatchAudit {
                                auditor: dispatch_auditor.clone(),
                                provider: reservation_provider.to_string(),
                                model: effective_model.clone(),
                                purpose: "conversation",
                                priced: reserved_cost_priced,
                            },
                        )),""",
     "primary-legacy")

# ── configured fallback, durable authority ──────────────────────────────────
sub1("""                                        execution_for_fallback.clone(),
                                        reservation,
                                        reserved_input,
                                        reserved_output,
                                        next_cost_usd,
                                    )),
                                    Ok(Err(error)) => {""",
"""                                        execution_for_fallback.clone(),
                                        reservation,
                                        reserved_input,
                                        reserved_output,
                                        next_cost_usd,
                                        SettledDispatchAudit {
                                            auditor: auditor_for_fallback.clone(),
                                            provider: next_provider.to_string(),
                                            model: next_model.to_string(),
                                            purpose: "configured_fallback",
                                            priced: next_cost.priced,
                                        },
                                    )),
                                    Ok(Err(error)) => {""",
     "fallback-durable")

# ── configured fallback, legacy tracker ─────────────────────────────────────
sub1("""                                        execution_for_fallback.clone(),
                                        reservation,
                                        reserved_input,
                                        reserved_output,
                                        next_cost_usd,
                                    )),
                                    Err(error) => {""",
"""                                        execution_for_fallback.clone(),
                                        reservation,
                                        reserved_input,
                                        reserved_output,
                                        next_cost_usd,
                                        SettledDispatchAudit {
                                            auditor: auditor_for_fallback.clone(),
                                            provider: next_provider.to_string(),
                                            model: next_model.to_string(),
                                            purpose: "configured_fallback",
                                            priced: next_cost.priced,
                                        },
                                    )),
                                    Err(error) => {""",
     "fallback-legacy")

# ── bind `reserved_cost_priced` and `dispatch_auditor` before the reservations ──
sub1("""                let reserved_cost = reserved_cost.usd;
                let budget_dispatch_id = provider_dispatch_id.clone().unwrap_or_else(|| {""",
"""                // #174 c2 — keep the priced/unpriced verdict alongside the
                // number. `reserved_cost.usd` is a conservative CEILING even
                // when nothing is priced, and recording that ceiling as a cost
                // would report an unpriced call as a known bill.
                let reserved_cost_priced = reserved_cost.priced;
                let reserved_cost = reserved_cost.usd;
                let dispatch_auditor = self.spend_guard.auditor();
                let budget_dispatch_id = provider_dispatch_id.clone().unwrap_or_else(|| {""",
     "reserved-cost-priced")

open(p, 'w').write(s)
print("ok")
