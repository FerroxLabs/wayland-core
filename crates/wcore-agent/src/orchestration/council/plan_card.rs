//! Map an assembled [`AssemblyPlan`] to the typed [`CruciblePlan`] proposal card.
//!
//! This is the Stage 2 foundation: a pure mapping that computes the certified
//! judge-inclusive ceiling and the single-model baseline from the SAME pricing
//! path the Assembler used, so every surface renders identical numbers from one
//! struct rather than re-parsing prose. No emit/consume wiring yet (Stage 3).

use wcore_pricing::DEFAULT_CATALOG;
use wcore_types::crucible::{CouncilMemberCard, CouncilRole, CruciblePlan};

use super::assembler::{AssemblyPlan, AssemblyPolicy};
use super::gate::Stakes;
use super::resolver::family;
use super::spend::CouncilSpend;

fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((p, m)) if !m.is_empty() => (p, Some(m)),
        _ => (spec, None),
    }
}

fn stakes_str(s: Stakes) -> &'static str {
    match s {
        Stakes::Low => "low",
        Stakes::Med => "med",
        Stakes::High => "high",
    }
}

/// Build the typed proposal card from an assembled plan. `day_spent_microcents`
/// / `day_cap_microcents` are passed in (read from the live BudgetTracker at
/// card-build time) — pass `None` when the envelope is not aggregating.
pub fn plan_to_card(
    plan: &AssemblyPlan,
    policy: &AssemblyPolicy,
    focus: Option<String>,
    day_spent_microcents: Option<u64>,
    day_cap_microcents: Option<u64>,
) -> CruciblePlan {
    // Members: proposers tagged Proposer, the aggregator tagged Judge.
    let mut members: Vec<CouncilMemberCard> = plan
        .members
        .iter()
        .map(|spec| CouncilMemberCard {
            spec: spec.clone(),
            vendor: family(spec),
            role: CouncilRole::Proposer,
        })
        .collect();
    if let Some(agg) = &plan.aggregator {
        members.push(CouncilMemberCard {
            spec: agg.clone(),
            vendor: family(agg),
            role: CouncilRole::Judge,
        });
    }

    // Judge independence: the judge vendor differs from every proposer vendor.
    let judge_independent = match &plan.aggregator {
        Some(agg) => {
            let jf = family(agg);
            !plan.members.iter().any(|m| family(m) == jf)
        }
        None => true,
    };

    // Certified judge-inclusive ceiling (None ⇒ not fully priceable). An empty
    // roster (no proposers AND no aggregator — e.g. the pool resolved to zero
    // runnable candidates, so the Assembler emitted a members-less Direct plan)
    // is NOT a priced $0 council; it is an ABSENT roster. estimate_preflight
    // would certify Some(0) for it, which the card contract forbids
    // ("None ⇒ render 'price unknown', never $0"), so force None.
    let proposers: Vec<(&str, Option<&str>)> = plan.members.iter().map(|s| split_spec(s)).collect();
    let aggregator = plan.aggregator.as_deref().map(split_spec);
    let ceiling_microcents = if proposers.is_empty() && aggregator.is_none() {
        None
    } else {
        CouncilSpend::estimate_preflight_microcents(
            &DEFAULT_CATALOG,
            &proposers,
            aggregator,
            policy.proposer_max_turns,
            policy.proposer_max_tokens,
            policy.markup,
        )
        .certified_microcents()
    };

    // Single STRONG model alone, for the "one model alone ≈ $X" comparison. Use
    // the judge — the Assembler reserves the strongest family as the decoupled
    // judge — when convening; fall back to the single Direct model otherwise.
    // NOT members.first(): the Assembler orders proposers cheapest-first, so the
    // first member is the CHEAPEST SKU, and comparing the council against it
    // would inflate the council's apparent value (and contradict this field's
    // doc, "one strong model alone").
    let baseline_spec = plan
        .aggregator
        .as_deref()
        .or_else(|| plan.members.first().map(String::as_str));
    let single_model_baseline_microcents = baseline_spec.map(split_spec).and_then(|m| {
        CouncilSpend::estimate_preflight_microcents(
            &DEFAULT_CATALOG,
            &[m],
            None,
            policy.proposer_max_turns,
            policy.proposer_max_tokens,
            policy.markup,
        )
        .certified_microcents()
    });

    CruciblePlan {
        convene: plan.convene,
        members,
        stakes: stakes_str(plan.stakes).to_string(),
        focus,
        ceiling_microcents,
        single_model_baseline_microcents,
        day_spent_microcents,
        day_cap_microcents,
        judge_independent,
        reason: plan.reason.clone(),
        trims: plan.trims.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::council::gate::Stakes;

    fn policy() -> AssemblyPolicy {
        // Mirror build_policy defaults; only the fields plan_to_card uses matter.
        AssemblyPolicy {
            deny_families: vec![],
            max_proposers: 5,
            markup: 1.0,
            cap_low_usd: 0.02,
            cap_med_usd: 0.05,
            cap_high_usd: 0.15,
            price_floor_frac: 0.0,
            proposer_max_turns: 4,
            proposer_max_tokens: 4096,
        }
    }

    #[test]
    fn maps_members_roles_vendors_and_prices() {
        let plan = AssemblyPlan {
            convene: true,
            members: vec![
                "deepseek:deepseek-v4-pro".into(),
                "anthropic:claude-opus-4-7".into(),
            ],
            aggregator: Some("openai:gpt-5".into()),
            est_cost_microcents: None,
            stakes: Stakes::Med,
            reason: "diverse".into(),
            trims: vec![],
        };
        let card = plan_to_card(&plan, &policy(), Some("c-suite".into()), None, None);
        assert_eq!(card.members.len(), 3);
        assert_eq!(card.members[2].role, CouncilRole::Judge);
        assert_eq!(card.members[0].vendor, "deepseek");
        assert_eq!(card.members[2].vendor, "openai");
        assert!(
            card.judge_independent,
            "openai judge vs deepseek/anthropic proposers"
        );
        // deepseek + opus-4-7 + gpt-5 are all priced under the default catalog.
        assert!(card.ceiling_microcents.is_some());
        // The baseline is the JUDGE (the reserved strongest family) priced ALONE
        // — never the cheapest proposer. Hand-derive to lock that.
        let judge_alone = CouncilSpend::estimate_preflight_microcents(
            &DEFAULT_CATALOG,
            &[("openai", Some("gpt-5"))],
            None,
            4,
            4096,
            1.0,
        )
        .certified_microcents();
        assert_eq!(card.single_model_baseline_microcents, judge_alone);
        // The full council ceiling exceeds one strong model alone.
        assert!(card.ceiling_microcents.unwrap() > card.single_model_baseline_microcents.unwrap());
    }

    #[test]
    fn empty_roster_reports_price_unknown_not_zero() {
        // When the candidate pool resolves to zero runnable specs, the Assembler
        // emits a Direct plan with empty members. The card MUST report a None
        // ceiling ("price unknown"), never a certified Some(0) that a host would
        // render as a free "$0.00" council — the contract's no-$0-surprise rule.
        let plan = AssemblyPlan {
            convene: false,
            members: vec![],
            aggregator: None,
            est_cost_microcents: None,
            stakes: Stakes::Low,
            reason: "no priceable candidates".into(),
            trims: vec![],
        };
        let card = plan_to_card(&plan, &policy(), None, None, None);
        assert_eq!(
            card.ceiling_microcents, None,
            "empty roster must render price-unknown, never a $0 ceiling"
        );
        assert_eq!(card.single_model_baseline_microcents, None);
        assert!(card.members.is_empty());
        assert!(!card.convene);
    }

    #[test]
    fn judge_sharing_a_vendor_is_not_independent() {
        let plan = AssemblyPlan {
            convene: true,
            members: vec!["openai:gpt-5".into(), "deepseek:deepseek-v4-pro".into()],
            aggregator: Some("openai:gpt-5-mini".into()),
            est_cost_microcents: None,
            stakes: Stakes::Low,
            reason: "x".into(),
            trims: vec![],
        };
        let card = plan_to_card(&plan, &policy(), None, None, None);
        assert!(
            !card.judge_independent,
            "openai judge collides with openai proposer"
        );
    }
}
