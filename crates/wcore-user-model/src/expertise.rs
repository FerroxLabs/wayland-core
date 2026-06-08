//! `ExpertiseEstimator` — infer per-domain expertise from observation
//! history.
//!
//! v0.7.0 2.B.3: simple heuristic. Per-domain interaction count + the
//! Wilson lower bound of acceptance ratio drive a 3-bucket level. The
//! middleware in 2.B.4 reads these via the backend and surfaces
//! `{{user.expertise.<domain>}}` into the system prompt.

use std::collections::BTreeMap;

use crate::preference_learner::LearnerState;
use crate::preferences::ExpertiseLevel;

/// Thresholds for the 3-bucket classifier. Tuned for the default
/// model + skill axes — the consumer can override by constructing
/// an `ExpertiseEstimator { ... }` directly.
#[derive(Debug, Clone, Copy)]
pub struct ExpertiseEstimator {
    /// Minimum total interactions in a domain to lift past `Novice`.
    pub intermediate_interactions: u64,
    /// Minimum Wilson lower bound + minimum interactions to reach
    /// `Expert`. Both gates must trip.
    pub expert_interactions: u64,
    pub expert_score: f32,
}

impl Default for ExpertiseEstimator {
    fn default() -> Self {
        Self {
            intermediate_interactions: 3,
            expert_interactions: 12,
            expert_score: 0.7,
        }
    }
}

impl ExpertiseEstimator {
    /// Score one domain. Aggregates every `(model, skill)` bucket
    /// for the given domain.
    pub fn level_for(&self, state: &LearnerState, domain: &str) -> ExpertiseLevel {
        let mut judged = 0u64;
        let mut positives = 0u64;
        for (key, counts) in &state.buckets {
            let key_domain = key.splitn(3, '|').nth(2).unwrap_or("");
            if key_domain != domain {
                continue;
            }
            judged = judged.saturating_add(counts.judged());
            positives = positives.saturating_add(counts.positives());
        }
        if judged < self.intermediate_interactions {
            return ExpertiseLevel::Novice;
        }
        if judged < self.expert_interactions {
            return ExpertiseLevel::Intermediate;
        }
        // Wilson over the aggregate.
        let n = judged as f32;
        let p = positives as f32 / n;
        let z = 1.96f32;
        let denom = 1.0 + z * z / n;
        let centre = p + z * z / (2.0 * n);
        let half = z * ((p * (1.0 - p) + z * z / (4.0 * n)) / n).sqrt();
        let lb = ((centre - half) / denom).clamp(0.0, 1.0);
        if lb >= self.expert_score {
            ExpertiseLevel::Expert
        } else {
            ExpertiseLevel::Intermediate
        }
    }

    /// Score every domain present in the state. Useful for batch
    /// brief refresh.
    pub fn all_levels(&self, state: &LearnerState) -> BTreeMap<String, ExpertiseLevel> {
        let mut out: BTreeMap<String, ExpertiseLevel> = BTreeMap::new();
        let mut domains: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for key in state.buckets.keys() {
            let d = key.splitn(3, '|').nth(2).unwrap_or("");
            if !d.is_empty() {
                domains.insert(d.to_string());
            }
        }
        for d in domains {
            let lvl = self.level_for(state, &d);
            out.insert(d, lvl);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{Observation, Outcome, ToolHint};
    use crate::preference_learner::PreferenceLearner;

    fn obs(domain: &str, outcome: Outcome) -> Observation {
        Observation {
            outcome: Some(outcome),
            hint: ToolHint {
                model: Some("opus".to_string()),
                skill: None,
                domain: Some(domain.to_string()),
            },
            style_fingerprint: None,
            ts_secs: 0,
        }
    }

    #[test]
    fn no_interactions_is_novice() {
        let state = LearnerState::default();
        let est = ExpertiseEstimator::default();
        assert_eq!(est.level_for(&state, "rust"), ExpertiseLevel::Novice);
    }

    #[test]
    fn few_interactions_stays_novice() {
        let mut state = LearnerState::default();
        for _ in 0..2 {
            PreferenceLearner::observe(&mut state, &obs("rust", Outcome::Accepted));
        }
        let est = ExpertiseEstimator::default();
        assert_eq!(est.level_for(&state, "rust"), ExpertiseLevel::Novice);
    }

    #[test]
    fn moderate_lifts_to_intermediate() {
        let mut state = LearnerState::default();
        for _ in 0..6 {
            PreferenceLearner::observe(&mut state, &obs("rust", Outcome::Accepted));
        }
        let est = ExpertiseEstimator::default();
        assert_eq!(est.level_for(&state, "rust"), ExpertiseLevel::Intermediate);
    }

    #[test]
    fn high_volume_high_score_is_expert() {
        let mut state = LearnerState::default();
        for _ in 0..25 {
            PreferenceLearner::observe(&mut state, &obs("rust", Outcome::Accepted));
        }
        for _ in 0..2 {
            PreferenceLearner::observe(&mut state, &obs("rust", Outcome::Rejected));
        }
        let est = ExpertiseEstimator::default();
        assert_eq!(est.level_for(&state, "rust"), ExpertiseLevel::Expert);
    }

    #[test]
    fn high_volume_low_score_stays_intermediate() {
        let mut state = LearnerState::default();
        for _ in 0..15 {
            PreferenceLearner::observe(&mut state, &obs("rust", Outcome::Accepted));
        }
        for _ in 0..15 {
            PreferenceLearner::observe(&mut state, &obs("rust", Outcome::Rejected));
        }
        let est = ExpertiseEstimator::default();
        assert_eq!(est.level_for(&state, "rust"), ExpertiseLevel::Intermediate);
    }

    #[test]
    fn all_levels_returns_one_entry_per_domain() {
        let mut state = LearnerState::default();
        for _ in 0..6 {
            PreferenceLearner::observe(&mut state, &obs("rust", Outcome::Accepted));
        }
        for _ in 0..6 {
            PreferenceLearner::observe(&mut state, &obs("react", Outcome::Accepted));
        }
        let est = ExpertiseEstimator::default();
        let map = est.all_levels(&state);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("rust"), Some(&ExpertiseLevel::Intermediate));
    }
}
