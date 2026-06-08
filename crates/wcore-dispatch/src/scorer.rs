//! Thompson-sampling Beta scorer for `DecisionRouter` arms.
//!
//! Posterior per arm is `Beta(success + 1, failure + 1)`. A pick draws
//! one sample per arm and returns the argmax — exploration emerges
//! naturally from the posterior spread on cold-start arms.
//!
//! This is a thin, standalone scorer. It deliberately does NOT depend
//! on `wcore-memory::partition::thompson` so that `wcore-dispatch` can
//! be embedded by lightweight callers (e.g. CLI subcommands) without
//! pulling in SQLite. The two implementations share the same posterior
//! shape, so a future merge is straightforward.

use std::collections::HashMap;
use std::hash::Hash;

use rand::{SeedableRng, rngs::StdRng};
use rand_distr::{Beta, Distribution};
use serde::{Deserialize, Serialize};

use crate::{RouterError, TaskOutcome};

/// Per-arm success/failure counts. Cold-start arms have `(0, 0)` which
/// posteriors as Beta(1, 1) = Uniform[0, 1].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub success: u64,
    pub failure: u64,
}

impl Stats {
    pub fn total(&self) -> u64 {
        self.success + self.failure
    }
}

/// Generic Thompson scorer. `TKey` is the arm identifier; usually a
/// `String` or a small `Clone + Hash + Eq` enum.
///
/// Construction:
/// - [`BetaScorer::new()`] — production: OS-RNG seeded.
/// - [`BetaScorer::with_seed(seed)`] — deterministic tests.
pub struct BetaScorer<TKey: Clone + Hash + Eq> {
    rng: StdRng,
    stats: HashMap<TKey, Stats>,
}

impl<TKey: Clone + Hash + Eq> Default for BetaScorer<TKey> {
    fn default() -> Self {
        Self::new()
    }
}

impl<TKey: Clone + Hash + Eq> BetaScorer<TKey> {
    /// OS-RNG-seeded scorer. Use in production.
    pub fn new() -> Self {
        Self {
            rng: StdRng::from_os_rng(),
            stats: HashMap::new(),
        }
    }

    /// Deterministic scorer seeded from `seed`. Use in tests.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            stats: HashMap::new(),
        }
    }

    /// Read-only access to stats — handy for debugging and persistence.
    pub fn stats(&self, key: &TKey) -> Stats {
        self.stats.get(key).cloned().unwrap_or_default()
    }

    /// Iterate every recorded arm's stats. Order is HashMap-defined
    /// (unspecified) — callers that need ordering should sort.
    pub fn iter_stats(&self) -> impl Iterator<Item = (&TKey, &Stats)> {
        self.stats.iter()
    }

    /// Hydrate stats from a previously persisted map (e.g. on startup).
    pub fn restore<I: IntoIterator<Item = (TKey, Stats)>>(&mut self, items: I) {
        self.stats.extend(items);
    }

    fn sample_beta(&mut self, alpha: f64, beta: f64) -> f64 {
        // Beta::new returns Err on non-finite or non-positive params;
        // fall back to a coin flip (matches wcore-memory thompson).
        match Beta::new(alpha, beta) {
            Ok(d) => d.sample(&mut self.rng),
            Err(_) => 0.5,
        }
    }
}

/// Public scorer surface so routers can swap implementations in tests.
pub trait Scorer<TKey: Clone + Hash + Eq> {
    /// Draw one Beta(α, β) sample per candidate and return a clone of
    /// the winner. `candidates` MUST be non-empty; returns
    /// [`RouterError::NoCandidates`] otherwise.
    fn thompson_pick(&mut self, candidates: &[TKey]) -> Result<TKey, RouterError>;

    /// Update the posterior for `key`. `Neutral` outcomes are ignored.
    fn record(&mut self, key: &TKey, outcome: TaskOutcome);
}

impl<TKey: Clone + Hash + Eq> Scorer<TKey> for BetaScorer<TKey> {
    fn thompson_pick(&mut self, candidates: &[TKey]) -> Result<TKey, RouterError> {
        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }
        // Pre-compute all samples so we don't re-sample inside the
        // argmax (would re-randomize and corrupt the comparison).
        let mut best_idx = 0usize;
        let mut best_score = f64::NEG_INFINITY;
        for (i, k) in candidates.iter().enumerate() {
            let s = self.stats.get(k).cloned().unwrap_or_default();
            let alpha = (s.success + 1) as f64;
            let beta = (s.failure + 1) as f64;
            let sample = self.sample_beta(alpha, beta);
            if sample > best_score {
                best_score = sample;
                best_idx = i;
            }
        }
        Ok(candidates[best_idx].clone())
    }

    fn record(&mut self, key: &TKey, outcome: TaskOutcome) {
        match outcome {
            TaskOutcome::Success => {
                self.stats.entry(key.clone()).or_default().success += 1;
            }
            TaskOutcome::Failure => {
                self.stats.entry(key.clone()).or_default().failure += 1;
            }
            TaskOutcome::Neutral => {
                // No-op by design.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_candidates_returns_no_candidates_err() {
        let mut s: BetaScorer<String> = BetaScorer::with_seed(7);
        let pick: Vec<String> = vec![];
        assert!(matches!(
            s.thompson_pick(&pick),
            Err(RouterError::NoCandidates)
        ));
    }

    #[test]
    fn neutral_outcome_does_not_update_stats() {
        let mut s: BetaScorer<&'static str> = BetaScorer::with_seed(7);
        s.record(&"a", TaskOutcome::Neutral);
        let st = s.stats(&"a");
        assert_eq!(st.success, 0);
        assert_eq!(st.failure, 0);
    }

    #[test]
    fn record_increments_success_and_failure_independently() {
        let mut s: BetaScorer<&'static str> = BetaScorer::with_seed(7);
        s.record(&"x", TaskOutcome::Success);
        s.record(&"x", TaskOutcome::Success);
        s.record(&"x", TaskOutcome::Failure);
        let st = s.stats(&"x");
        assert_eq!(st.success, 2);
        assert_eq!(st.failure, 1);
        assert_eq!(st.total(), 3);
    }

    #[test]
    fn restore_round_trip() {
        let mut s: BetaScorer<String> = BetaScorer::with_seed(7);
        s.restore(vec![
            (
                "a".to_string(),
                Stats {
                    success: 10,
                    failure: 2,
                },
            ),
            (
                "b".to_string(),
                Stats {
                    success: 1,
                    failure: 5,
                },
            ),
        ]);
        assert_eq!(s.stats(&"a".to_string()).success, 10);
        assert_eq!(s.stats(&"b".to_string()).failure, 5);
    }

    /// Convergence: with 1000 reseeded trials between two arms — a
    /// strong arm Beta(51, 2) (mean ≈ 0.962) and a weak arm Beta(2, 51)
    /// (mean ≈ 0.038) — Thompson should pick the strong arm ≥ 950 / 1000
    /// times. Mirrors the wcore-memory `strong_arm_wins_top_slot` golden.
    #[test]
    fn dominant_arm_wins_at_least_95pct() {
        let candidates: Vec<&'static str> = vec!["strong", "weak"];
        let n: u32 = 1000;
        let mut strong_wins: u32 = 0;
        for i in 0..n {
            let mut s: BetaScorer<&'static str> = BetaScorer::with_seed(42 + i as u64);
            // Strong: 50 successes, 1 failure (α=51, β=2). Weak: inverse.
            s.restore(vec![
                (
                    "strong",
                    Stats {
                        success: 50,
                        failure: 1,
                    },
                ),
                (
                    "weak",
                    Stats {
                        success: 1,
                        failure: 50,
                    },
                ),
            ]);
            let pick = s.thompson_pick(&candidates).unwrap();
            if pick == "strong" {
                strong_wins += 1;
            }
        }
        assert!(
            strong_wins >= 950,
            "Thompson should converge: strong wins {strong_wins}/{n} (expected >=950)"
        );
    }

    /// Cold-start fairness: with two completely unseen arms, Thompson
    /// should pick each ~50% of the time (Beta(1,1) = Uniform[0,1] each).
    /// We just require both arms get picked at least once across 200 trials.
    #[test]
    fn cold_start_explores_both_arms() {
        let candidates: Vec<&'static str> = vec!["fresh_a", "fresh_b"];
        let mut a_seen = false;
        let mut b_seen = false;
        for i in 0..200 {
            let mut s: BetaScorer<&'static str> = BetaScorer::with_seed(100 + i);
            let pick = s.thompson_pick(&candidates).unwrap();
            if pick == "fresh_a" {
                a_seen = true;
            } else if pick == "fresh_b" {
                b_seen = true;
            }
            if a_seen && b_seen {
                break;
            }
        }
        assert!(a_seen && b_seen, "cold-start should explore both arms");
    }

    /// Convergence from cold start: feed real outcomes and watch the
    /// scorer learn. 200 simulated rounds where arm "good" succeeds
    /// 80% of the time and arm "bad" 20%. After training, in 500 picks
    /// we want "good" chosen > 400 (well over the 50% baseline).
    #[test]
    fn online_convergence_from_cold_start() {
        let candidates: Vec<&'static str> = vec!["good", "bad"];
        let mut s: BetaScorer<&'static str> = BetaScorer::with_seed(2026);

        // Simulated environment. Outcomes are deterministic for
        // reproducibility — we cycle a known pattern hitting the
        // target Bernoulli rates over the training window.
        let train_rounds = 200u32;
        for t in 0..train_rounds {
            let pick = s.thompson_pick(&candidates).unwrap();
            let success_pattern_good = (t % 5) != 0; // 80% success
            let success_pattern_bad = (t % 5) == 0; // 20% success
            let outcome = match pick {
                "good" if success_pattern_good => TaskOutcome::Success,
                "good" => TaskOutcome::Failure,
                "bad" if success_pattern_bad => TaskOutcome::Success,
                "bad" => TaskOutcome::Failure,
                _ => unreachable!(),
            };
            s.record(&pick, outcome);
        }

        // Evaluation pass: 500 picks, "good" should dominate strongly.
        let mut good_picks = 0u32;
        for _ in 0..500 {
            if s.thompson_pick(&candidates).unwrap() == "good" {
                good_picks += 1;
            }
        }
        assert!(
            good_picks > 400,
            "online Thompson should converge; got {good_picks}/500 good picks"
        );
    }
}
