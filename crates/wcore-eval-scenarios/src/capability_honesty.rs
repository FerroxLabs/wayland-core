use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use wcore_protocol::events::{CapabilityActivation, CapabilityId, CapabilityStage};

use crate::scenario::CapabilityExpectation;

const ALL_CAPABILITIES: [CapabilityId; 8] = [
    CapabilityId::PricingRefresher,
    CapabilityId::MidFlightMonitor,
    CapabilityId::CooldownTracker,
    CapabilityId::LearnedPolicy,
    CapabilityId::SmartHandoff,
    CapabilityId::DelegateIsolation,
    CapabilityId::ProcedureSkillDrafting,
    CapabilityId::LegacyAutoSkillDrafting,
];

#[derive(Debug, Default)]
pub(crate) struct CapabilityEvidence {
    activations: BTreeMap<CapabilityId, Vec<CapabilityActivation>>,
    malformed: Vec<String>,
}

impl CapabilityEvidence {
    pub(crate) fn capture(&mut self, event: &Value) {
        if event.get("type").and_then(Value::as_str) != Some("capability_activation") {
            return;
        }

        match serde_json::from_value::<CapabilityActivation>(event.clone()) {
            Ok(activation) => self
                .activations
                .entry(activation.capability)
                .or_default()
                .push(activation),
            Err(error) => self.malformed.push(error.to_string()),
        }
    }

    pub(crate) fn enforce_frozen_thresholds(&self) -> Result<(), String> {
        let thresholds: FrontierThresholds =
            toml::from_str(include_str!("../frontier-thresholds-v1.toml"))
                .map_err(|error| format!("invalid frozen Frontier thresholds: {error}"))?;

        let mut honest = 0usize;
        let mut startup_proved = 0usize;
        let mut issues = self
            .malformed
            .iter()
            .map(|error| format!("malformed activation event: {error}"))
            .collect::<Vec<_>>();

        for capability in ALL_CAPABILITIES {
            let events = self
                .activations
                .get(&capability)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if has_startup_proof(events) {
                startup_proved += 1;
            }
            match validate_chain(events) {
                Ok(()) => honest += 1,
                Err(error) => issues.push(format!("{capability:?}: {error}")),
            }
        }

        let denominator = ALL_CAPABILITIES.len() as f64;
        let honesty_rate = honest as f64 / denominator;
        let activation_proof_rate = startup_proved as f64 / denominator;
        if honesty_rate < thresholds.deterministic.capability_honesty_pass_rate_min {
            issues.push(format!(
                "capability honesty rate {honesty_rate:.3} is below frozen threshold {:.3}",
                thresholds.deterministic.capability_honesty_pass_rate_min
            ));
        }
        if activation_proof_rate < thresholds.capability.advertised_activation_proof_rate_min {
            issues.push(format!(
                "advertised activation proof rate {activation_proof_rate:.3} is below frozen threshold {:.3}",
                thresholds
                    .capability
                    .advertised_activation_proof_rate_min
            ));
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues.join("; "))
        }
    }

    pub(crate) fn enforce_expectations(
        &self,
        expectations: &[CapabilityExpectation],
    ) -> Result<(), String> {
        let mut issues = Vec::new();
        for expectation in expectations {
            match *expectation {
                CapabilityExpectation::Unavailable { capability, reason } => {
                    let events = self
                        .activations
                        .get(&capability)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    if let Err(error) = validate_chain(events) {
                        issues.push(format!("{capability:?}: {error}"));
                        continue;
                    }
                    if events
                        .iter()
                        .any(|event| event.stage == CapabilityStage::Ready)
                    {
                        issues.push(format!(
                            "{capability:?}: advertised ready before required unavailability"
                        ));
                        continue;
                    }
                    match events.last() {
                        Some(event)
                            if event.stage == CapabilityStage::Unavailable
                                && event.reason == Some(reason) => {}
                        Some(event) => issues.push(format!(
                            "{capability:?}: expected Unavailable({reason:?}), got {:?}({:?})",
                            event.stage, event.reason
                        )),
                        None => issues.push(format!("{capability:?}: missing activation chain")),
                    }
                }
                CapabilityExpectation::OutcomeObserved { capability } => {
                    let events = self
                        .activations
                        .get(&capability)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    if let Err(error) = validate_chain(events) {
                        issues.push(format!("{capability:?}: {error}"));
                        continue;
                    }
                    let expected = [
                        CapabilityStage::Ready,
                        CapabilityStage::Reached,
                        CapabilityStage::OutcomeChanged,
                        CapabilityStage::Observed,
                    ];
                    let mut cursor = 0usize;
                    for event in events {
                        if expected.get(cursor) == Some(&event.stage) {
                            cursor += 1;
                        }
                    }
                    if cursor != expected.len()
                        || events.last().map(|event| event.stage) != Some(CapabilityStage::Observed)
                    {
                        issues.push(format!(
                            "{capability:?}: missing complete Ready -> Reached -> OutcomeChanged -> Observed cycle"
                        ));
                    }
                }
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues.join("; "))
        }
    }
}

fn validate_chain(events: &[CapabilityActivation]) -> Result<(), String> {
    let Some(first) = events.first() else {
        return Err("missing declared activation chain".to_string());
    };
    if first.stage != CapabilityStage::Declared {
        return Err(format!(
            "chain starts at {:?}, expected Declared",
            first.stage
        ));
    }

    for (index, event) in events.iter().enumerate() {
        if !event.is_well_formed() {
            return Err(format!(
                "event {index} has an invalid stage/reason combination"
            ));
        }
        if let Some(previous) = index.checked_sub(1).and_then(|i| events.get(i))
            && !previous.stage.allows(event.stage)
        {
            return Err(format!(
                "illegal transition {:?} -> {:?} at event {index}",
                previous.stage, event.stage
            ));
        }
    }

    if !has_startup_proof(events) {
        return Err("startup chain never reached Ready or Unavailable".to_string());
    }
    let final_stage = events
        .last()
        .map(|event| event.stage)
        .ok_or_else(|| "missing declared activation chain".to_string())?;
    if !matches!(
        final_stage,
        CapabilityStage::Ready | CapabilityStage::Observed | CapabilityStage::Unavailable
    ) {
        return Err(format!(
            "runtime chain ends at incomplete stage {final_stage:?}"
        ));
    }

    Ok(())
}

fn has_startup_proof(events: &[CapabilityActivation]) -> bool {
    let Some(first) = events.first() else {
        return false;
    };
    if first.stage != CapabilityStage::Declared || !first.is_well_formed() {
        return false;
    }

    for (previous, current) in events.iter().zip(events.iter().skip(1)) {
        if !current.is_well_formed() || !previous.stage.allows(current.stage) {
            return false;
        }
        if matches!(
            current.stage,
            CapabilityStage::Ready | CapabilityStage::Unavailable
        ) {
            return true;
        }
    }
    false
}

#[derive(Debug, Deserialize)]
struct FrontierThresholds {
    deterministic: DeterministicThresholds,
    capability: CapabilityThresholds,
}

#[derive(Debug, Deserialize)]
struct DeterministicThresholds {
    capability_honesty_pass_rate_min: f64,
}

#[derive(Debug, Deserialize)]
struct CapabilityThresholds {
    advertised_activation_proof_rate_min: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_protocol::events::CapabilityReasonCode;

    fn capture(
        evidence: &mut CapabilityEvidence,
        capability: CapabilityId,
        stage: CapabilityStage,
        reason: Option<CapabilityReasonCode>,
    ) {
        evidence.capture(&serde_json::json!({
            "type": "capability_activation",
            "capability": capability,
            "stage": stage,
            "reason": reason,
        }));
    }

    fn complete_startup_evidence() -> CapabilityEvidence {
        let mut evidence = CapabilityEvidence::default();
        for capability in ALL_CAPABILITIES {
            capture(&mut evidence, capability, CapabilityStage::Declared, None);
            capture(
                &mut evidence,
                capability,
                CapabilityStage::Unavailable,
                Some(CapabilityReasonCode::DisabledByConfig),
            );
        }
        evidence
    }

    #[test]
    fn accepts_all_startup_proofs_and_complete_runtime_cycles() {
        let mut evidence = complete_startup_evidence();
        let capability = CapabilityId::SmartHandoff;
        evidence.activations.insert(
            capability,
            vec![
                CapabilityActivation::stage(capability, CapabilityStage::Declared),
                CapabilityActivation::stage(capability, CapabilityStage::Configured),
                CapabilityActivation::stage(capability, CapabilityStage::Constructed),
                CapabilityActivation::stage(capability, CapabilityStage::Ready),
                CapabilityActivation::stage(capability, CapabilityStage::Reached),
                CapabilityActivation::stage(capability, CapabilityStage::OutcomeChanged),
                CapabilityActivation::stage(capability, CapabilityStage::Observed),
                CapabilityActivation::stage(capability, CapabilityStage::Reached),
                CapabilityActivation::stage(capability, CapabilityStage::OutcomeChanged),
                CapabilityActivation::stage(capability, CapabilityStage::Observed),
            ],
        );

        assert_eq!(evidence.enforce_frozen_thresholds(), Ok(()));
    }

    #[test]
    fn rejects_missing_advertised_capability_proof() {
        let mut evidence = complete_startup_evidence();
        evidence
            .activations
            .remove(&CapabilityId::DelegateIsolation);

        let error = evidence.enforce_frozen_thresholds().unwrap_err();

        assert!(
            error.contains("DelegateIsolation: missing declared"),
            "{error}"
        );
        assert!(error.contains("honesty rate 0.875"), "{error}");
        assert!(error.contains("activation proof rate 0.875"), "{error}");
    }

    #[test]
    fn rejects_out_of_order_runtime_evidence() {
        let mut evidence = complete_startup_evidence();
        let capability = CapabilityId::ProcedureSkillDrafting;
        evidence.activations.insert(
            capability,
            vec![
                CapabilityActivation::stage(capability, CapabilityStage::Declared),
                CapabilityActivation::stage(capability, CapabilityStage::Configured),
                CapabilityActivation::stage(capability, CapabilityStage::Constructed),
                CapabilityActivation::stage(capability, CapabilityStage::Ready),
                CapabilityActivation::stage(capability, CapabilityStage::OutcomeChanged),
            ],
        );

        let error = evidence.enforce_frozen_thresholds().unwrap_err();

        assert!(
            error.contains("illegal transition Ready -> OutcomeChanged"),
            "{error}"
        );
    }

    #[test]
    fn rejects_unreasoned_unavailability() {
        let mut evidence = complete_startup_evidence();
        let capability = CapabilityId::PricingRefresher;
        evidence.activations.insert(
            capability,
            vec![
                CapabilityActivation::stage(capability, CapabilityStage::Declared),
                CapabilityActivation::stage(capability, CapabilityStage::Unavailable),
            ],
        );

        let error = evidence.enforce_frozen_thresholds().unwrap_err();

        assert!(
            error.contains("invalid stage/reason combination"),
            "{error}"
        );
    }

    #[test]
    fn exact_unavailable_expectation_rejects_ready_or_wrong_reason() {
        let capability = CapabilityId::PricingRefresher;
        let expectation = [CapabilityExpectation::Unavailable {
            capability,
            reason: CapabilityReasonCode::NoProductionConstructor,
        }];

        let mut wrong_reason = complete_startup_evidence();
        assert!(
            wrong_reason
                .enforce_expectations(&expectation)
                .unwrap_err()
                .contains("expected Unavailable(NoProductionConstructor)")
        );

        wrong_reason.activations.insert(
            capability,
            vec![
                CapabilityActivation::stage(capability, CapabilityStage::Declared),
                CapabilityActivation::stage(capability, CapabilityStage::Configured),
                CapabilityActivation::stage(capability, CapabilityStage::Constructed),
                CapabilityActivation::stage(capability, CapabilityStage::Ready),
            ],
        );
        assert!(
            wrong_reason
                .enforce_expectations(&expectation)
                .unwrap_err()
                .contains("advertised ready")
        );
    }

    #[test]
    fn outcome_expectation_requires_complete_runtime_cycle() {
        let capability = CapabilityId::SmartHandoff;
        let expectation = [CapabilityExpectation::OutcomeObserved { capability }];
        let mut evidence = complete_startup_evidence();
        evidence.activations.insert(
            capability,
            vec![
                CapabilityActivation::stage(capability, CapabilityStage::Declared),
                CapabilityActivation::stage(capability, CapabilityStage::Configured),
                CapabilityActivation::stage(capability, CapabilityStage::Constructed),
                CapabilityActivation::stage(capability, CapabilityStage::Ready),
            ],
        );

        assert!(
            evidence
                .enforce_expectations(&expectation)
                .unwrap_err()
                .contains("missing complete Ready")
        );

        evidence.activations.get_mut(&capability).unwrap().extend([
            CapabilityActivation::stage(capability, CapabilityStage::Reached),
            CapabilityActivation::stage(capability, CapabilityStage::OutcomeChanged),
            CapabilityActivation::stage(capability, CapabilityStage::Observed),
        ]);
        assert_eq!(evidence.enforce_expectations(&expectation), Ok(()));
    }
}
