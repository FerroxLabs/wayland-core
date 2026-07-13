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

    unavailable(
        &mut events,
        CapabilityId::PricingRefresher,
        CapabilityReasonCode::NoProductionConstructor,
    );
    unavailable(
        &mut events,
        CapabilityId::MidFlightMonitor,
        CapabilityReasonCode::RuntimePathUnwired,
    );
    unavailable(
        &mut events,
        CapabilityId::CooldownTracker,
        CapabilityReasonCode::NoProductionConstructor,
    );
    unavailable(
        &mut events,
        CapabilityId::LearnedPolicy,
        CapabilityReasonCode::RuntimePathUnwired,
    );

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
        for capability in [
            CapabilityId::PricingRefresher,
            CapabilityId::MidFlightMonitor,
            CapabilityId::CooldownTracker,
            CapabilityId::LearnedPolicy,
            CapabilityId::DelegateIsolation,
        ] {
            assert_eq!(statuses[&capability].stage, CapabilityStage::Unavailable);
        }
    }

    #[test]
    fn configured_memory_failure_is_not_reported_as_disabled_or_ready() {
        let events = startup_activations(StartupCapabilityInputs {
            smart_compaction_enabled: true,
            smart_handoff_enabled: true,
            skills_lifecycle_enabled: true,
            memory_constructed: false,
            legacy_drafter_constructed: false,
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
