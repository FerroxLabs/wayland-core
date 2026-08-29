p = 'crates/wcore-agent/src/engine.rs'
s = open(p).read()

def sub1(old, new, label):
    global s
    assert s.count(old) == 1, f"{label}: expected 1, found {s.count(old)}"
    s = s.replace(old, new, 1)

# ── A. failure variant for a spend-guard refusal ────────────────────────────
sub1("""enum ConfiguredFallbackAdmissionFailure {
    Budget(ProviderBudgetMutationError),
    Unpriced { provider: String, model: String },
}""",
"""enum ConfiguredFallbackAdmissionFailure {
    Budget(ProviderBudgetMutationError),
    Unpriced { provider: String, model: String },
    /// #174 c3-c5 — the fallback's provider/model is refused by the session's
    /// spend mode, or is an un-authorized model escalation.
    ///
    /// This site needs its own gate: a configured fallback swaps the model
    /// INSIDE `ResilientProvider::stream`, below the engine's guarded provider
    /// handle, so the decorator that covers every other dispatch never sees it.
    SpendGuard(wcore_budget::SpendRefusal),
}""", "failure-variant")

sub1("""                        ConfiguredFallbackAdmissionFailure::Unpriced { provider, model } => {
                            self.output.emit_budget_exceeded(
                                "unpriced_provider",
                                &format!("{provider}/{model}"),
                                "a provider/model with known pricing",
                            );
                            self.emit_error(
                                &format!(
                                    "Configured provider fallback not started: pricing is \\
                                     unavailable for {provider}/{model}, so the explicit or \\
                                     managed USD cap cannot be enforced."
                                ),
                                false,
                            );
                        }""",
"""                        ConfiguredFallbackAdmissionFailure::Unpriced { provider, model } => {
                            self.output.emit_budget_exceeded(
                                "unpriced_provider",
                                &format!("{provider}/{model}"),
                                "a provider/model with known pricing",
                            );
                            self.emit_error(
                                &format!(
                                    "Configured provider fallback not started: pricing is \\
                                     unavailable for {provider}/{model}, so the explicit or \\
                                     managed USD cap cannot be enforced."
                                ),
                                false,
                            );
                        }
                        ConfiguredFallbackAdmissionFailure::SpendGuard(refusal) => {
                            self.output.emit_budget_exceeded(
                                refusal.kind(),
                                &format!("{fallback_provider}/{fallback_model}"),
                                "a model this session is permitted to use",
                            );
                            self.emit_error(&refusal.to_string(), false);
                        }""", "failure-render")

# ── B. gate the fallback admitter ───────────────────────────────────────────
sub1("""                let fallback_compat = self.compat.clone();
                let fallback_admitter: wcore_providers::retry::ConfiguredFallbackAdmitter =""",
"""                let fallback_compat = self.compat.clone();
                let auditor_for_fallback = self.spend_guard.auditor();
                let guard_for_fallback = Arc::clone(&self.spend_guard);
                let fallback_admitter: wcore_providers::retry::ConfiguredFallbackAdmitter =""",
     "fallback-captures")

sub1("""                        let next_cost = resolve_conservative_reservation_cost(
                            next_provider,
                            next_model,
                            reserved_input,
                            reserved_output,
                            &fallback_compat,
                        );""",
"""                        // #174 c3-c5 — the spend guard binds here, before any
                        // reservation is taken, because this is the one
                        // dispatch path that changes provider AND model below
                        // the guarded provider handle.
                        let next_profile = crate::spend_guard::classify_model(
                            next_provider,
                            next_model,
                            &fallback_compat,
                        );
                        if let Err(refusal) = guard_for_fallback.admit(&next_profile) {
                            state.failure =
                                Some(ConfiguredFallbackAdmissionFailure::SpendGuard(refusal));
                            return Err(ProviderError::NotAttempted {
                                reason: "configured fallback refused by the spend guard"
                                    .to_string(),
                                failure_code: Some("spend_guard_refused".to_string()),
                            });
                        }
                        let next_cost = resolve_conservative_reservation_cost(
                            next_provider,
                            next_model,
                            reserved_input,
                            reserved_output,
                            &fallback_compat,
                        );""", "fallback-gate")

# ── C. gate the smart-routing tier swap ─────────────────────────────────────
sub1("""                        if let Some(tier_model) =
                            select_tier_model(&decision, requires_vision, &self.compat)
                        {
                            tracing::debug!(
                                target: "wcore_agent::routing",
                                from = %self.model,
                                to = %tier_model,
                                hint = %decision.to_hint().0,
                                "smart-routing tier swap"
                            );
                            request.model = tier_model.clone();
                            effective_model = tier_model;
                        }""",
"""                        if let Some(tier_model) =
                            select_tier_model(&decision, requires_vision, &self.compat)
                        {
                            // #174 c3-c5 — a tier "downgrade" is only a
                            // downgrade if the tier model is actually cheaper.
                            // A `[compat.tier_models]` entry naming a pricier
                            // (or unpriced, or hosted-under-local-only) model
                            // is an escalation wearing a cheap tier's name, so
                            // it is checked, not trusted. Refusing here DECLINES
                            // THE SWAP rather than failing the turn: the
                            // configured model is still perfectly runnable, and
                            // the decorator remains the backstop if it is not.
                            let tier_profile = crate::spend_guard::classify_model(
                                self.compat.provider_type(),
                                &tier_model,
                                &self.compat,
                            );
                            match self.spend_guard.admit(&tier_profile) {
                                Ok(()) => {
                                    tracing::debug!(
                                        target: "wcore_agent::routing",
                                        from = %self.model,
                                        to = %tier_model,
                                        hint = %decision.to_hint().0,
                                        "smart-routing tier swap"
                                    );
                                    request.model = tier_model.clone();
                                    effective_model = tier_model;
                                }
                                Err(refusal) => {
                                    tracing::warn!(
                                        target: "wcore_agent::routing",
                                        from = %self.model,
                                        to = %tier_model,
                                        %refusal,
                                        "smart-routing tier swap refused by the spend guard"
                                    );
                                }
                            }
                        }""", "tier-swap-gate")

open(p, 'w').write(s)
print("ok")
