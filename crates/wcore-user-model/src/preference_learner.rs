//! `PreferenceLearner` — fold observations into running
//! preference + (model, skill) -> score tables.
//!
//! v0.7.0 2.B.3: this is the data-side learner that consumes
//! [`crate::observation::Observation`] and produces refined
//! [`crate::preferences::Preferences`]. It is the dual of
//! [`crate::local::LocalBackend`]'s light-touch update: where
//! the backend records the *most recent* outcome per domain,
//! the learner aggregates *all* outcomes into a probability /
//! score that can be queried by routers (e.g. "which model
//! works best for the rust domain?").

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::observation::{Observation, Outcome};

/// Aggregated success/failure counts keyed by `(model, skill, domain)`.
/// Stored sparse — only triples observed at least once are present.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LearnerState {
    /// Map key is `"{model}|{skill}|{domain}"` (empty segment for
    /// `None` field). Stable string-keying avoids the need for a
    /// custom map-key serde impl.
    #[serde(default)]
    pub buckets: BTreeMap<String, OutcomeCounts>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeCounts {
    pub accepted: u64,
    pub praised: u64,
    pub rejected: u64,
    pub corrected: u64,
    pub ignored: u64,
}

impl OutcomeCounts {
    /// Total positive (accepted + praised).
    pub fn positives(&self) -> u64 {
        self.accepted.saturating_add(self.praised)
    }
    /// Total negative (rejected + corrected). `ignored` is neutral.
    pub fn negatives(&self) -> u64 {
        self.rejected.saturating_add(self.corrected)
    }
    /// Total non-neutral observations.
    pub fn judged(&self) -> u64 {
        self.positives().saturating_add(self.negatives())
    }
    /// Wilson 95% lower confidence bound on positive ratio. Returns
    /// `0.0` when no observations exist. Useful for ranking with a
    /// small-sample penalty (a 5/5 wins over 1/1).
    pub fn wilson_lower_bound(&self) -> f32 {
        let pos = self.positives() as f32;
        let n = self.judged() as f32;
        if n <= 0.0 {
            return 0.0;
        }
        let z: f32 = 1.96; // 95% CI
        let p = pos / n;
        let denom = 1.0 + z * z / n;
        let centre = p + z * z / (2.0 * n);
        let half = z * ((p * (1.0 - p) + z * z / (4.0 * n)) / n).sqrt();
        ((centre - half) / denom).clamp(0.0, 1.0)
    }
}

/// The learner itself. Stateless behaviour over a `LearnerState`
/// that the caller owns + persists.
pub struct PreferenceLearner;

impl PreferenceLearner {
    /// Fold one observation into the running state. Returns true if
    /// any bucket was updated. `Outcome::Ignored` is recorded in the
    /// `ignored` field but does not move the positive/negative tally.
    pub fn observe(state: &mut LearnerState, obs: &Observation) -> bool {
        let Some(outcome) = obs.outcome else {
            return false;
        };
        let key = bucket_key(
            obs.hint.model.as_deref().unwrap_or(""),
            obs.hint.skill.as_deref().unwrap_or(""),
            obs.hint.domain.as_deref().unwrap_or(""),
        );
        let bucket = state.buckets.entry(key).or_default();
        match outcome {
            Outcome::Accepted => bucket.accepted = bucket.accepted.saturating_add(1),
            Outcome::Praised => bucket.praised = bucket.praised.saturating_add(1),
            Outcome::Rejected => bucket.rejected = bucket.rejected.saturating_add(1),
            Outcome::Corrected => bucket.corrected = bucket.corrected.saturating_add(1),
            Outcome::Ignored => bucket.ignored = bucket.ignored.saturating_add(1),
        }
        true
    }

    /// Best (model, skill) for a given domain, ranked by Wilson lower
    /// bound. Returns `None` when no observations exist for the
    /// domain. Empty string in either field means "unspecified".
    pub fn best_for_domain(state: &LearnerState, domain: &str) -> Option<DomainRecommendation> {
        let mut best: Option<DomainRecommendation> = None;
        for (key, counts) in &state.buckets {
            let (model, skill, key_domain) = split_bucket_key(key);
            if key_domain != domain {
                continue;
            }
            if counts.judged() == 0 {
                continue;
            }
            let score = counts.wilson_lower_bound();
            let rec = DomainRecommendation {
                model: opt_from_str(model),
                skill: opt_from_str(skill),
                domain: domain.to_string(),
                score,
                positives: counts.positives(),
                negatives: counts.negatives(),
            };
            match &best {
                None => best = Some(rec),
                Some(b) if rec.score > b.score => best = Some(rec),
                _ => {}
            }
        }
        best
    }
}

/// One ranked recommendation produced by [`PreferenceLearner::best_for_domain`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DomainRecommendation {
    pub model: Option<String>,
    pub skill: Option<String>,
    pub domain: String,
    pub score: f32,
    pub positives: u64,
    pub negatives: u64,
}

fn bucket_key(model: &str, skill: &str, domain: &str) -> String {
    format!("{model}|{skill}|{domain}")
}

fn split_bucket_key(key: &str) -> (&str, &str, &str) {
    let mut it = key.splitn(3, '|');
    let m = it.next().unwrap_or("");
    let s = it.next().unwrap_or("");
    let d = it.next().unwrap_or("");
    (m, s, d)
}

fn opt_from_str(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::{Observation, Outcome, ToolHint};

    fn obs(model: &str, domain: &str, outcome: Outcome) -> Observation {
        Observation {
            outcome: Some(outcome),
            hint: ToolHint {
                model: Some(model.to_string()),
                skill: None,
                domain: Some(domain.to_string()),
            },
            style_fingerprint: None,
            ts_secs: 0,
        }
    }

    #[test]
    fn observe_increments_correct_counter() {
        let mut state = LearnerState::default();
        PreferenceLearner::observe(&mut state, &obs("opus", "rust", Outcome::Accepted));
        let counts = state.buckets.values().next().unwrap();
        assert_eq!(counts.accepted, 1);
        assert_eq!(counts.judged(), 1);
    }

    #[test]
    fn observe_without_outcome_is_noop() {
        let mut state = LearnerState::default();
        let obs = Observation::default();
        assert!(!PreferenceLearner::observe(&mut state, &obs));
        assert!(state.buckets.is_empty());
    }

    #[test]
    fn ignored_does_not_move_judged_count() {
        let mut state = LearnerState::default();
        PreferenceLearner::observe(&mut state, &obs("opus", "rust", Outcome::Ignored));
        let counts = state.buckets.values().next().unwrap();
        assert_eq!(counts.ignored, 1);
        assert_eq!(counts.judged(), 0);
    }

    #[test]
    fn wilson_zero_for_empty() {
        assert_eq!(OutcomeCounts::default().wilson_lower_bound(), 0.0);
    }

    #[test]
    fn wilson_penalises_small_samples() {
        let small = OutcomeCounts {
            accepted: 1,
            ..Default::default()
        };
        let large = OutcomeCounts {
            accepted: 20,
            ..Default::default()
        };
        assert!(small.wilson_lower_bound() < large.wilson_lower_bound());
    }

    #[test]
    fn best_for_domain_picks_highest_wilson() {
        let mut state = LearnerState::default();
        // opus on rust: 9 accept / 1 reject
        for _ in 0..9 {
            PreferenceLearner::observe(&mut state, &obs("opus", "rust", Outcome::Accepted));
        }
        PreferenceLearner::observe(&mut state, &obs("opus", "rust", Outcome::Rejected));
        // sonnet on rust: 1 accept / 0 reject
        PreferenceLearner::observe(&mut state, &obs("sonnet", "rust", Outcome::Accepted));

        let best = PreferenceLearner::best_for_domain(&state, "rust").expect("a rec");
        assert_eq!(best.model.as_deref(), Some("opus"));
    }

    #[test]
    fn best_for_unknown_domain_returns_none() {
        let state = LearnerState::default();
        assert!(PreferenceLearner::best_for_domain(&state, "haskell").is_none());
    }
}
