//! Crucible (Mixture-of-Providers) typed proposal card + decision.
//!
//! `CruciblePlan` is the ONE source of truth for the council proposal card —
//! TUI, desktop, and chat all render the same numbers from it instead of
//! re-parsing prose. It rides the approval rail (`ApprovalRequired.plan`); the
//! host returns a [`CrucibleDecision`] via the approval outcome's `modifications`.

use serde::{Deserialize, Serialize};

/// A member's role in the council.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouncilRole {
    Proposer,
    Judge,
}

/// One council member as shown on the proposal card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouncilMemberCard {
    /// The `provider` / `provider:model` spec.
    pub spec: String,
    /// The vendor family (so the same model via two routes shows once).
    pub vendor: String,
    pub role: CouncilRole,
}

/// The typed proposal card. Built before any spend; carried on the approval rail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CruciblePlan {
    /// Whether the gate convened a council (false ⇒ a single Direct model).
    pub convene: bool,
    /// Proposers + the judge (role-tagged). For a Direct plan, the single model.
    pub members: Vec<CouncilMemberCard>,
    /// Stakes tier: "low" | "med" | "high".
    pub stakes: String,
    /// Optional persona/lens applied to proposers + judge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    /// Certified judge-inclusive worst-case ceiling (microcents). `None` when the
    /// roster is not fully priceable — render "price unknown", never $0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling_microcents: Option<u64>,
    /// One strong model alone, for comparison (microcents). `None` if unpriceable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub single_model_baseline_microcents: Option<u64>,
    /// Running daily spend + cap (microcents). Both `None` unless the envelope
    /// genuinely aggregates — omit the "today" line rather than show a zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_spent_microcents: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_cap_microcents: Option<u64>,
    /// True when the judge's vendor differs from every proposer's vendor.
    pub judge_independent: bool,
    /// The Assembler's decision trace.
    pub reason: String,
    /// Budget downshift steps applied to fit the cap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trims: Vec<String>,
}

/// The canonical USD↔microcents conversion factor: 1 USD = 100¢ = 100_000_000 µ¢.
/// The single source of truth for this constant across the workspace — every
/// crate that prices microcents into USD (or USD into a microcents cap) references
/// this instead of redeclaring it, so a one-sided edit can never desync them.
pub const MICROCENTS_PER_USD: f64 = 100_000_000.0;

impl CruciblePlan {
    /// The certified ceiling in USD, if priceable.
    pub fn ceiling_usd(&self) -> Option<f64> {
        self.ceiling_microcents
            .map(|m| m as f64 / MICROCENTS_PER_USD)
    }
    /// The single-model baseline in USD, if priceable.
    pub fn baseline_usd(&self) -> Option<f64> {
        self.single_model_baseline_microcents
            .map(|m| m as f64 / MICROCENTS_PER_USD)
    }
}

/// The host's typed response to a proposal card, returned via the approval
/// outcome's `modifications` (a `serde_json::Value`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum CrucibleDecision {
    /// Run the plan as proposed.
    Approve,
    /// Approve at a higher certified ceiling (the `[p]` premium upgrade).
    ApprovePremium { ceiling_usd: f64 },
    /// Re-assemble with an edited roster and/or budget.
    Edit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roster: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        budget_usd: Option<f64>,
    },
    /// Abort — no spend.
    Cancel,
}

impl CrucibleDecision {
    /// Parse a host decision from the approval outcome's `modifications` value.
    /// `None`/absent ⇒ no typed decision was supplied (caller decides the default).
    pub fn from_modifications(modifications: Option<&serde_json::Value>) -> Option<Self> {
        modifications.and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_round_trips_and_usd_helpers() {
        let plan = CruciblePlan {
            convene: true,
            members: vec![
                CouncilMemberCard {
                    spec: "deepseek:deepseek-v4-pro".into(),
                    vendor: "deepseek".into(),
                    role: CouncilRole::Proposer,
                },
                CouncilMemberCard {
                    spec: "anthropic:claude-opus-4-8".into(),
                    vendor: "anthropic".into(),
                    role: CouncilRole::Judge,
                },
            ],
            stakes: "med".into(),
            focus: Some("c-suite".into()),
            ceiling_microcents: Some(210_000_000),
            single_model_baseline_microcents: Some(45_000_000),
            day_spent_microcents: None,
            day_cap_microcents: Some(2_000_000_000),
            judge_independent: true,
            reason: "diverse cross-vendor".into(),
            trims: vec![],
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: CruciblePlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
        assert!((plan.ceiling_usd().unwrap() - 2.10).abs() < 1e-9);
        assert!((plan.baseline_usd().unwrap() - 0.45).abs() < 1e-9);
        // Omitted (None) fields do not serialize.
        assert!(!json.contains("day_spent_microcents"));
    }

    #[test]
    fn decision_round_trips_all_variants() {
        for d in [
            CrucibleDecision::Approve,
            CrucibleDecision::ApprovePremium { ceiling_usd: 4.5 },
            CrucibleDecision::Edit {
                roster: Some(vec!["openai:gpt-5".into()]),
                budget_usd: Some(3.0),
            },
            CrucibleDecision::Cancel,
        ] {
            let json = serde_json::to_value(&d).unwrap();
            let back: CrucibleDecision = serde_json::from_value(json).unwrap();
            assert_eq!(d, back);
        }
        // Tag shape is stable for hosts.
        let v =
            serde_json::to_value(CrucibleDecision::ApprovePremium { ceiling_usd: 4.5 }).unwrap();
        assert_eq!(v["decision"], "approve_premium");
    }

    #[test]
    fn from_modifications_parses_valid_and_rejects_garbage() {
        // A valid bare-approve.
        let v = serde_json::json!({ "decision": "approve" });
        assert_eq!(
            CrucibleDecision::from_modifications(Some(&v)),
            Some(CrucibleDecision::Approve)
        );
        // approve_premium carries the accepted ceiling.
        let v = serde_json::json!({ "decision": "approve_premium", "ceiling_usd": 4.5 });
        assert_eq!(
            CrucibleDecision::from_modifications(Some(&v)),
            Some(CrucibleDecision::ApprovePremium { ceiling_usd: 4.5 })
        );
        // Absent modifications ⇒ no typed decision.
        assert_eq!(CrucibleDecision::from_modifications(None), None);
        // A malformed value (unknown tag) ⇒ None, not a panic.
        let bad = serde_json::json!({ "decision": "explode" });
        assert_eq!(CrucibleDecision::from_modifications(Some(&bad)), None);
    }
}
