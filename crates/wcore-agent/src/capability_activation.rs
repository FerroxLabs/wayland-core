use wcore_protocol::events::{
    CapabilityActivation, CapabilityId, CapabilityReasonCode, CapabilityStage,
};

/// Production construction facts resolved during bootstrap. Configuration can
/// request a capability without its dependencies actually being available;
/// keeping those facts separate prevents configured from becoming "ready" by
/// implication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupCapabilityInputs {
    pub smart_compaction_enabled: bool,
    pub smart_handoff_enabled: bool,
    pub skills_lifecycle_enabled: bool,
    pub memory_constructed: bool,
    pub legacy_drafter_constructed: bool,
    pub midflight_monitor_constructed: bool,
    pub pricing_refresher_constructed: bool,
    pub cooldown_tracker_constructed: bool,
    /// Phase 22 (22-02 Task 3): a `LearnedPolicy` was actually constructed
    /// from an on-disk operator policy AND installed on the engine, so every
    /// child this session spawns dispatches through the narrowing pre-filter.
    /// False when no policy file exists — the pre-filter is wired but has
    /// nothing to enforce, which is `disabled_by_config`, not readiness.
    pub learned_policy_constructed: bool,
}

fn unavailable(
    events: &mut Vec<CapabilityActivation>,
    capability: CapabilityId,
    reason: CapabilityReasonCode,
) {
    events.push(CapabilityActivation::stage(
        capability,
        CapabilityStage::Declared,
    ));
    events.push(CapabilityActivation::unavailable(capability, reason));
}

fn configured_unavailable(
    events: &mut Vec<CapabilityActivation>,
    capability: CapabilityId,
    reason: CapabilityReasonCode,
) {
    events.push(CapabilityActivation::stage(
        capability,
        CapabilityStage::Declared,
    ));
    events.push(CapabilityActivation::stage(
        capability,
        CapabilityStage::Configured,
    ));
    events.push(CapabilityActivation::unavailable(capability, reason));
}

fn ready(events: &mut Vec<CapabilityActivation>, capability: CapabilityId) {
    for stage in [
        CapabilityStage::Declared,
        CapabilityStage::Configured,
        CapabilityStage::Constructed,
        CapabilityStage::Ready,
    ] {
        events.push(CapabilityActivation::stage(capability, stage));
    }
}

/// Produce the deterministic startup truth for every capability in F05's
/// audited set. Dormant assets remain unavailable; this function does not wire
/// them merely to make the report green.
pub fn startup_activations(inputs: StartupCapabilityInputs) -> Vec<CapabilityActivation> {
    let mut events = Vec::with_capacity(24);

    if inputs.pricing_refresher_constructed {
        ready(&mut events, CapabilityId::PricingRefresher);
    } else {
        unavailable(
            &mut events,
            CapabilityId::PricingRefresher,
            CapabilityReasonCode::DisabledByConfig,
        );
    }
    if inputs.midflight_monitor_constructed {
        ready(&mut events, CapabilityId::MidFlightMonitor);
    } else {
        unavailable(
            &mut events,
            CapabilityId::MidFlightMonitor,
            CapabilityReasonCode::NoProductionConstructor,
        );
    }
    if inputs.cooldown_tracker_constructed {
        ready(&mut events, CapabilityId::CooldownTracker);
    } else {
        unavailable(
            &mut events,
            CapabilityId::CooldownTracker,
            CapabilityReasonCode::NoProductionConstructor,
        );
    }
    // Phase 22 (22-02 Task 3). This row read `RuntimePathUnwired`
    // UNCONDITIONALLY — there was no input for it, so no configuration could
    // ever make it ready, and `AgentExecutorConfig::learned_policy` had zero
    // readers in the workspace. The pre-filter is now consulted at
    // `node_executor::dispatch_once` for every `CallActor::SubAgent` dispatch,
    // and `AgentSpawner` constructs that actor for every child, so the runtime
    // path exists. What remains conditional is whether the operator has a
    // policy for it to apply.
    if inputs.learned_policy_constructed {
        ready(&mut events, CapabilityId::LearnedPolicy);
    } else {
        unavailable(
            &mut events,
            CapabilityId::LearnedPolicy,
            CapabilityReasonCode::DisabledByConfig,
        );
    }

    if !inputs.smart_compaction_enabled || !inputs.smart_handoff_enabled {
        unavailable(
            &mut events,
            CapabilityId::SmartHandoff,
            CapabilityReasonCode::DisabledByConfig,
        );
    } else if !inputs.memory_constructed {
        configured_unavailable(
            &mut events,
            CapabilityId::SmartHandoff,
            CapabilityReasonCode::DependencyUnavailable,
        );
    } else {
        ready(&mut events, CapabilityId::SmartHandoff);
    }

    unavailable(
        &mut events,
        CapabilityId::DelegateIsolation,
        CapabilityReasonCode::IsolationNotEnforced,
    );

    if !inputs.skills_lifecycle_enabled {
        unavailable(
            &mut events,
            CapabilityId::ProcedureSkillDrafting,
            CapabilityReasonCode::DisabledByConfig,
        );
        unavailable(
            &mut events,
            CapabilityId::LegacyAutoSkillDrafting,
            CapabilityReasonCode::DisabledByConfig,
        );
    } else if !inputs.memory_constructed {
        configured_unavailable(
            &mut events,
            CapabilityId::ProcedureSkillDrafting,
            CapabilityReasonCode::DependencyUnavailable,
        );
        configured_unavailable(
            &mut events,
            CapabilityId::LegacyAutoSkillDrafting,
            CapabilityReasonCode::DependencyUnavailable,
        );
    } else {
        ready(&mut events, CapabilityId::ProcedureSkillDrafting);
        if inputs.legacy_drafter_constructed {
            ready(&mut events, CapabilityId::LegacyAutoSkillDrafting);
        } else {
            configured_unavailable(
                &mut events,
                CapabilityId::LegacyAutoSkillDrafting,
                CapabilityReasonCode::NoProductionConstructor,
            );
        }
    }

    events
}

/// Runtime proof emitted only after the capability's real side effect succeeds.
pub fn successful_occurrence(capability: CapabilityId) -> [CapabilityActivation; 3] {
    [
        CapabilityActivation::stage(capability, CapabilityStage::Reached),
        CapabilityActivation::stage(capability, CapabilityStage::OutcomeChanged),
        CapabilityActivation::stage(capability, CapabilityStage::Observed),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn final_statuses(
        events: &[CapabilityActivation],
    ) -> BTreeMap<CapabilityId, &CapabilityActivation> {
        let mut statuses = BTreeMap::new();
        for event in events {
            statuses.insert(event.capability, event);
        }
        statuses
    }

    fn assert_legal_chains(events: &[CapabilityActivation]) {
        let mut previous = BTreeMap::new();
        for event in events {
            assert!(event.is_well_formed(), "malformed event: {event:?}");
            if let Some(stage) = previous.insert(event.capability, event.stage) {
                assert!(
                    stage.allows(event.stage),
                    "illegal {stage:?} -> {:?} for {:?}",
                    event.stage,
                    event.capability
                );
            } else {
                assert_eq!(event.stage, CapabilityStage::Declared);
            }
        }
    }

    #[test]
    fn default_startup_reports_all_eight_capabilities_honestly() {
        let events = startup_activations(StartupCapabilityInputs {
            smart_compaction_enabled: false,
            smart_handoff_enabled: false,
            skills_lifecycle_enabled: false,
            memory_constructed: false,
            legacy_drafter_constructed: false,
            midflight_monitor_constructed: false,
            pricing_refresher_constructed: false,
            cooldown_tracker_constructed: false,
            learned_policy_constructed: false,
        });
        assert_legal_chains(&events);
        let statuses = final_statuses(&events);

        assert_eq!(statuses.len(), 8);
        assert!(statuses.values().all(|event| {
            event.stage == CapabilityStage::Unavailable && event.reason.is_some()
        }));
        assert_eq!(
            statuses[&CapabilityId::DelegateIsolation].reason,
            Some(CapabilityReasonCode::IsolationNotEnforced)
        );
    }

    #[test]
    fn live_memory_paths_become_ready_but_dormant_assets_do_not() {
        let events = startup_activations(StartupCapabilityInputs {
            smart_compaction_enabled: true,
            smart_handoff_enabled: true,
            skills_lifecycle_enabled: true,
            memory_constructed: true,
            legacy_drafter_constructed: true,
            midflight_monitor_constructed: true,
            pricing_refresher_constructed: true,
            cooldown_tracker_constructed: true,
            learned_policy_constructed: true,
        });
        assert_legal_chains(&events);
        let statuses = final_statuses(&events);

        for capability in [
            CapabilityId::SmartHandoff,
            CapabilityId::ProcedureSkillDrafting,
            CapabilityId::LegacyAutoSkillDrafting,
        ] {
            assert_eq!(statuses[&capability].stage, CapabilityStage::Ready);
        }
        // `DelegateIsolation` is still a genuinely dormant asset. `LearnedPolicy`
        // is NOT, as of Phase 22 (22-02 Task 3) — its pre-filter is consulted at
        // dispatch for every `CallActor::SubAgent` — so it is asserted below on
        // its real construction input instead of being carried here. Removing it
        // from this list is not a weakening: `learned_policy_is_unavailable_
        // without_a_constructed_policy` is the replacement, and it FAILS if the
        // row ever claims readiness without one.
        assert_eq!(
            statuses[&CapabilityId::DelegateIsolation].stage,
            CapabilityStage::Unavailable
        );
        assert_eq!(
            statuses[&CapabilityId::LearnedPolicy].stage,
            CapabilityStage::Ready
        );
        assert_eq!(
            statuses[&CapabilityId::MidFlightMonitor].stage,
            CapabilityStage::Ready
        );
        assert_eq!(
            statuses[&CapabilityId::PricingRefresher].stage,
            CapabilityStage::Ready
        );
        assert_eq!(
            statuses[&CapabilityId::CooldownTracker].stage,
            CapabilityStage::Ready
        );
    }

    /// The falsifiable half of the Phase 22 `learned_policy` wiring: readiness
    /// is bound to a CONSTRUCTED policy, not to the pre-filter existing. Every
    /// other input is held at its ready value, so `learned_policy_constructed`
    /// is the single variable.
    #[test]
    fn learned_policy_is_unavailable_without_a_constructed_policy() {
        let ready_inputs = StartupCapabilityInputs {
            smart_compaction_enabled: true,
            smart_handoff_enabled: true,
            skills_lifecycle_enabled: true,
            memory_constructed: true,
            legacy_drafter_constructed: true,
            midflight_monitor_constructed: true,
            pricing_refresher_constructed: true,
            cooldown_tracker_constructed: true,
            learned_policy_constructed: true,
        };
        let with_policy = startup_activations(ready_inputs);
        assert_legal_chains(&with_policy);
        assert_eq!(
            final_statuses(&with_policy)[&CapabilityId::LearnedPolicy].stage,
            CapabilityStage::Ready
        );

        let without_policy = startup_activations(StartupCapabilityInputs {
            learned_policy_constructed: false,
            ..ready_inputs
        });
        assert_legal_chains(&without_policy);
        let statuses = final_statuses(&without_policy);
        assert_eq!(
            statuses[&CapabilityId::LearnedPolicy].stage,
            CapabilityStage::Unavailable,
            "readiness must follow a constructed policy, not the pre-filter's existence"
        );
        assert_eq!(
            statuses[&CapabilityId::LearnedPolicy].reason,
            Some(CapabilityReasonCode::DisabledByConfig),
            "the honest reason is now 'no policy configured', NOT 'runtime path unwired' — \
             the runtime path is what Phase 22 wired"
        );
        // And the old reason must be gone: a row that still emits
        // RuntimePathUnwired anywhere would mean the wiring did not land.
        assert!(
            !without_policy.iter().any(|event| {
                event.capability == CapabilityId::LearnedPolicy
                    && event.reason == Some(CapabilityReasonCode::RuntimePathUnwired)
            }),
            "F05-TRUTH-4's reason code must no longer be reachable for learned_policy"
        );
    }

    #[test]
    fn configured_memory_failure_is_not_reported_as_disabled_or_ready() {
        let events = startup_activations(StartupCapabilityInputs {
            smart_compaction_enabled: true,
            smart_handoff_enabled: true,
            skills_lifecycle_enabled: true,
            memory_constructed: false,
            legacy_drafter_constructed: false,
            midflight_monitor_constructed: true,
            pricing_refresher_constructed: true,
            cooldown_tracker_constructed: true,
            learned_policy_constructed: true,
        });
        assert_legal_chains(&events);
        let statuses = final_statuses(&events);

        for capability in [
            CapabilityId::SmartHandoff,
            CapabilityId::ProcedureSkillDrafting,
            CapabilityId::LegacyAutoSkillDrafting,
        ] {
            assert_eq!(
                statuses[&capability].reason,
                Some(CapabilityReasonCode::DependencyUnavailable)
            );
        }
    }

    #[test]
    fn handoff_flag_cannot_claim_ready_while_smart_compaction_is_disabled() {
        let events = startup_activations(StartupCapabilityInputs {
            smart_compaction_enabled: false,
            smart_handoff_enabled: true,
            skills_lifecycle_enabled: false,
            memory_constructed: true,
            legacy_drafter_constructed: false,
            midflight_monitor_constructed: true,
            pricing_refresher_constructed: false,
            cooldown_tracker_constructed: true,
            learned_policy_constructed: true,
        });
        assert_legal_chains(&events);
        let statuses = final_statuses(&events);

        assert_eq!(
            statuses[&CapabilityId::SmartHandoff].stage,
            CapabilityStage::Unavailable
        );
        assert_eq!(
            statuses[&CapabilityId::SmartHandoff].reason,
            Some(CapabilityReasonCode::DisabledByConfig)
        );
    }

    #[test]
    fn successful_occurrences_form_a_repeatable_runtime_cycle() {
        let first = successful_occurrence(CapabilityId::SmartHandoff);
        assert!(CapabilityStage::Ready.allows(first[0].stage));
        assert!(first[0].stage.allows(first[1].stage));
        assert!(first[1].stage.allows(first[2].stage));
        let second = successful_occurrence(CapabilityId::SmartHandoff);
        assert!(first[2].stage.allows(second[0].stage));
    }
}
