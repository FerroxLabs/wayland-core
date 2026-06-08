// v0.6.4 Task 6.4 — ThompsonSampler (port of Forge `ThompsonSampler.ts`).
//
// Beta(α, β) sampler used by `PartitionDispatcher::top_procedures` to score
// stored procedures with Thompson-sampling exploration/exploitation. The
// procedural-partition writer (`procedural.rs::record_use`) maintains the
// `thompson_alpha` / `thompson_beta` columns on each tool use; this module
// only consumes them.
//
// Design notes:
//   - We rely on `rand_distr::Beta` (a tested Marsaglia–Tsang implementation)
//     over a seedable `StdRng`. No hand-rolled gamma sampler.
//   - `alpha = success + 1`, `beta = failure + 1` — Beta(1, 1) cold start =
//     Uniform[0, 1] matches Forge.
//   - PRNG output depends on rand-crate version and platform, so tests assert
//     statistical properties (empirical means, win counts) within tolerances,
//     not literal f64 outputs (see `v0.6.4-memory-depth-design.md`, §6.4).
use rand::{SeedableRng, rngs::StdRng};
use rand_distr::{Beta, Distribution};

/// Candidate procedure / tool to be scored by Thompson sampling.
#[derive(Debug, Clone)]
pub struct ToolCandidate {
    pub tool_name: String,
    pub success_count: u64,
    pub failure_count: u64,
}

/// Result of a Thompson-sampling pick (kept around for callers that want the
/// raw score in addition to the chosen tool name).
#[derive(Debug, Clone)]
pub struct ToolSelectionResult {
    pub tool_name: String,
    pub score: f64,
}

/// Thompson sampler with an injectable PRNG seed for deterministic tests.
pub struct ThompsonSampler {
    rng: StdRng,
}

impl Default for ThompsonSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl ThompsonSampler {
    /// Fresh sampler seeded from the OS RNG. Production callers use this.
    pub fn new() -> Self {
        Self {
            rng: StdRng::from_os_rng(),
        }
    }

    /// Deterministic sampler seeded from `seed`. Tests use this.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// Draw a single Beta(α, β) sample. Returns `0.5` if α/β are degenerate
    /// (≤0 or non-finite) — matches Forge's "fall back to coin flip" behavior.
    pub fn sample_beta(&mut self, alpha: f64, beta: f64) -> f64 {
        Beta::new(alpha, beta)
            .map(|d| d.sample(&mut self.rng))
            .unwrap_or(0.5)
    }

    /// Thompson-sample each candidate's posterior Beta(success+1, failure+1)
    /// and return a reference to the winner's `tool_name`.
    ///
    /// # Panics
    /// Panics if `candidates` is empty.
    pub fn select_tool<'a>(&mut self, candidates: &'a [ToolCandidate]) -> &'a str {
        assert!(
            !candidates.is_empty(),
            "ThompsonSampler::select_tool requires at least one candidate"
        );
        let mut best_name: &str = candidates[0].tool_name.as_str();
        let mut best_score: f64 = f64::NEG_INFINITY;
        for c in candidates {
            let s = self.sample_beta((c.success_count + 1) as f64, (c.failure_count + 1) as f64);
            if s > best_score {
                best_score = s;
                best_name = c.tool_name.as_str();
            }
        }
        best_name
    }

    /// Thompson-sample each candidate and return the full
    /// `ToolSelectionResult` (name + sampled score) for the winner.
    pub fn select_tool_with_score(&mut self, candidates: &[ToolCandidate]) -> ToolSelectionResult {
        assert!(
            !candidates.is_empty(),
            "ThompsonSampler::select_tool_with_score requires at least one candidate"
        );
        let mut best = ToolSelectionResult {
            tool_name: candidates[0].tool_name.clone(),
            score: f64::NEG_INFINITY,
        };
        for c in candidates {
            let s = self.sample_beta((c.success_count + 1) as f64, (c.failure_count + 1) as f64);
            if s > best.score {
                best = ToolSelectionResult {
                    tool_name: c.tool_name.clone(),
                    score: s,
                };
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Run `n` independent selections, reseeding the sampler with
    /// `seed_base + i` for the i-th trial so the same `seed_base` is
    /// reproducible across runs.
    fn beta_trials(seed_base: u64, candidates: &[ToolCandidate], n: u32) -> HashMap<String, u32> {
        let mut counts: HashMap<String, u32> = HashMap::new();
        for c in candidates {
            counts.insert(c.tool_name.clone(), 0);
        }
        for i in 0..n {
            let mut s = ThompsonSampler::with_seed(seed_base + i as u64);
            let winner = s.select_tool(candidates).to_string();
            *counts.entry(winner).or_insert(0) += 1;
        }
        counts
    }

    /// Beta(1,1) on both arms = two Uniform[0,1] draws. Over 10_000 trials
    /// each tool should win between 4500 and 5500 times (~50/50 within ±5σ).
    #[test]
    fn cold_start_uniform() {
        let candidates = vec![
            ToolCandidate {
                tool_name: "a".into(),
                success_count: 0,
                failure_count: 0,
            },
            ToolCandidate {
                tool_name: "b".into(),
                success_count: 0,
                failure_count: 0,
            },
        ];
        let counts = beta_trials(42, &candidates, 10_000);
        let a = counts["a"];
        let b = counts["b"];
        assert!(
            (4500..=5500).contains(&a),
            "a wins out of range: {} (expected 4500..=5500)",
            a
        );
        assert!(
            (4500..=5500).contains(&b),
            "b wins out of range: {} (expected 4500..=5500)",
            b
        );
        assert_eq!(a + b, 10_000);
    }

    /// Beta(51,2) mean≈0.962 vs Beta(2,51) mean≈0.038 — `a` should dominate.
    #[test]
    fn dominant_arm_wins() {
        let candidates = vec![
            ToolCandidate {
                tool_name: "a".into(),
                success_count: 50,
                failure_count: 1,
            },
            ToolCandidate {
                tool_name: "b".into(),
                success_count: 1,
                failure_count: 50,
            },
        ];
        let counts = beta_trials(42, &candidates, 1_000);
        let a = counts["a"];
        assert!(a >= 950, "dominant arm a should win >=950 times, got {}", a);
    }

    /// Cold-start arm (Beta(1,1)) should still beat an established arm
    /// (Beta(9,3) mean=0.75) ~20–25% of the time — i.e. between 100 and 400
    /// wins over 1_000 trials.
    #[test]
    fn cold_vs_observed_exploration() {
        let candidates = vec![
            ToolCandidate {
                tool_name: "observed".into(),
                success_count: 8,
                failure_count: 2,
            },
            ToolCandidate {
                tool_name: "new".into(),
                success_count: 0,
                failure_count: 0,
            },
        ];
        let counts = beta_trials(42, &candidates, 1_000);
        let new_wins = counts["new"];
        assert!(
            (100..=400).contains(&new_wins),
            "cold-start arm exploration out of range: {} wins (expected 100..=400)",
            new_wins
        );
    }

    /// Beta(2, 5) theoretical mean = 2/7 ≈ 0.2857. Empirical mean over 10_000
    /// samples from a `with_seed(42)` sampler should land in [0.27, 0.30].
    #[test]
    fn sample_beta_mean() {
        let mut s = ThompsonSampler::with_seed(42);
        let mut total = 0.0_f64;
        let n = 10_000;
        for _ in 0..n {
            total += s.sample_beta(2.0, 5.0);
        }
        let mean = total / n as f64;
        assert!(
            (0.27..=0.30).contains(&mean),
            "Beta(2,5) empirical mean out of range: {} (expected [0.27, 0.30])",
            mean
        );
    }

    /// Beta(1, 1) = Uniform[0, 1]; empirical mean over 10_000 samples should
    /// be ≈0.5 within [0.49, 0.51].
    #[test]
    fn sample_beta_uniform() {
        let mut s = ThompsonSampler::with_seed(42);
        let mut total = 0.0_f64;
        let n = 10_000;
        for _ in 0..n {
            total += s.sample_beta(1.0, 1.0);
        }
        let mean = total / n as f64;
        assert!(
            (0.49..=0.51).contains(&mean),
            "Beta(1,1) empirical mean out of range: {} (expected [0.49, 0.51])",
            mean
        );
    }

    /// Two consecutive samplers with the same seed must produce identical
    /// draws — the seedable RNG is the only thing the tests rely on for
    /// reproducibility.
    #[test]
    fn determinism_same_seed_same_samples() {
        let mut a = ThompsonSampler::with_seed(42);
        let mut b = ThompsonSampler::with_seed(42);
        for _ in 0..32 {
            let xa = a.sample_beta(3.0, 7.0);
            let xb = b.sample_beta(3.0, 7.0);
            assert_eq!(xa.to_bits(), xb.to_bits(), "seeded samples diverged");
        }
    }

    /// `select_tool_with_score` should agree with `select_tool` when both
    /// samplers start from the same seed and see the same candidates.
    #[test]
    fn select_with_score_agrees_with_select_tool() {
        let candidates = vec![
            ToolCandidate {
                tool_name: "a".into(),
                success_count: 5,
                failure_count: 1,
            },
            ToolCandidate {
                tool_name: "b".into(),
                success_count: 2,
                failure_count: 2,
            },
        ];
        let mut s1 = ThompsonSampler::with_seed(42);
        let mut s2 = ThompsonSampler::with_seed(42);
        let pick = s1.select_tool(&candidates).to_string();
        let pick_with_score = s2.select_tool_with_score(&candidates);
        assert_eq!(pick, pick_with_score.tool_name);
        assert!(pick_with_score.score.is_finite());
    }
}
