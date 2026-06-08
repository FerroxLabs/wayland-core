//! M5+ — Per-turn skill router.
//!
//! The session-start [`SkillPrioritizer`] (in `prioritizer.rs`) reorders
//! the global skill list ONCE per session from procedural memory. The
//! `SkillRouter` here is the per-turn complement: given a task plus a
//! candidate list, it picks the single best skill using a Thompson
//! [`BetaScorer`] from `wcore-dispatch`. Picks update the scorer; over a
//! session the router learns which skills work for which task shapes.
//!
//! ## ROUTER HINT — F-068 (loop closed)
//!
//! The candidate `choose()` returns (via `DecisionRouter`) is stashed in
//! `engine.current_skill_router_pick` and credited to `observe()` at turn end.
//! As of v0.8.1 U1 the engine ALSO surfaces the pick to the model: when the
//! router is installed and the pick names a visible catalog skill, the engine
//! appends ONE short, non-binding line to the turn's system prompt (see
//! `Engine::skill_router_hint`) — `Skill hint: ... the "<name>" skill may help
//! ... use it only if genuinely relevant.` The hint is advisory: the model
//! still selects skills on its own and is free to ignore it. The router does
//! NOT prepend message context and does NOT automatically invoke the skill.
//!
//! The router accumulates Thompson Beta statistics across turns to learn which
//! skills succeed for which task shapes; the hint is gated on router
//! installation, so engines built without a router are behaviour-identical.
//!
//! Optionally hydrated from the prioritizer's session-start ranking via
//! [`SkillRouter::seed_from_prioritizer`] — names ranked higher in the
//! prioritizer's output get a small head-start (3 simulated successes)
//! so the per-turn scorer doesn't start cold on a fresh session.
//!
//! Manual override: `@@skill=<name>` in the task description. If the
//! named skill is in the candidate set, it's returned directly; else
//! the scorer picks.

use wcore_dispatch::{BetaScorer, DecisionRouter, RouterError, Scorer, Stats, TaskOutcome};

/// Per-turn skill router. Cheap to construct.
pub struct SkillRouter {
    scorer: BetaScorer<String>,
}

impl Default for SkillRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillRouter {
    pub fn new() -> Self {
        Self {
            scorer: BetaScorer::new(),
        }
    }

    /// Deterministic seed for tests.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            scorer: BetaScorer::with_seed(seed),
        }
    }

    /// Seed the scorer from a prioritizer-style ordering. Higher-ranked
    /// names (earlier indices) get +3 simulated successes; later names
    /// stay at default Beta(1, 1). Idempotent — calling twice is a no-op
    /// for names already seeded.
    pub fn seed_from_prioritizer(&mut self, ranked: &[String]) {
        // Walk from end-of-list to give earlier names more weight by
        // restoring them last (only the success count matters, but we
        // skip restoring keys that already exist).
        let mut already: std::collections::HashSet<String> =
            self.scorer.iter_stats().map(|(k, _)| k.clone()).collect();
        // Highest-ranked = index 0 → most successes; degrade linearly
        // but never below 1 (so the bottom of the list still has the
        // default cold-start posterior).
        let n = ranked.len();
        for (i, name) in ranked.iter().enumerate() {
            if already.contains(name) {
                continue;
            }
            // Top of list: 3 successes; mid: 2; bottom: 1; tail: 0.
            let successes = match i {
                _ if i < n / 4 => 3,
                _ if i < n / 2 => 2,
                _ if i < (3 * n) / 4 => 1,
                _ => 0,
            };
            if successes > 0 {
                self.scorer.restore(std::iter::once((
                    name.clone(),
                    Stats {
                        success: successes,
                        failure: 0,
                    },
                )));
            }
            already.insert(name.clone());
        }
    }

    /// Restore seeds from an externally-computed list of
    /// `(skill_name, success_count)` pairs. Used by callers that have
    /// access to both `wcore-skills` and a separate seeding source
    /// (e.g. GEPA's `PromptStore::seed_pairs_for`) but can't introduce a
    /// direct dep due to cycles. Idempotent on names already seeded by
    /// `seed_from_prioritizer` (or a prior call to this method) — skipped
    /// silently. Pairs with `success == 0` are also skipped. Returns the
    /// number of arms actually seeded.
    pub fn restore_seeds<I>(&mut self, pairs: I) -> usize
    where
        I: IntoIterator<Item = (String, u64)>,
    {
        let already: std::collections::HashSet<String> =
            self.scorer.iter_stats().map(|(k, _)| k.clone()).collect();
        let mut seeded = 0usize;
        for (name, success) in pairs {
            if already.contains(&name) || success == 0 {
                continue;
            }
            self.scorer.restore(std::iter::once((
                name,
                Stats {
                    success,
                    failure: 0,
                },
            )));
            seeded += 1;
        }
        seeded
    }

    /// Parse `@@skill=<name>` from input. Kebab-case allowed.
    fn parse_override(input: &str) -> Option<String> {
        let lower = input.to_ascii_lowercase();
        let needle = "@@skill=";
        let idx = lower.find(needle)?;
        let tail = &input[idx + needle.len()..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(tail.len());
        if end == 0 {
            None
        } else {
            Some(tail[..end].to_string())
        }
    }
}

/// (task description, candidate skills) → chosen skill name.
pub struct SkillRouterInput<'a> {
    pub task: &'a str,
    pub candidates: &'a [String],
}

impl<'a> DecisionRouter<String, SkillRouterInput<'a>> for SkillRouter {
    fn choose(&mut self, input: SkillRouterInput<'a>) -> Result<String, RouterError> {
        if let Some(name) = Self::parse_override(input.task)
            && input.candidates.iter().any(|s| s == &name)
        {
            return Ok(name);
        }
        self.scorer.thompson_pick(input.candidates)
    }

    fn observe(&mut self, choice: &String, outcome: TaskOutcome) {
        self.scorer.record(choice, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_candidates_returns_no_candidates() {
        let mut r = SkillRouter::with_seed(7);
        let pick = r.choose(SkillRouterInput {
            task: "foo",
            candidates: &[],
        });
        assert!(matches!(pick, Err(RouterError::NoCandidates)));
    }

    #[test]
    fn override_honored_when_in_candidates() {
        let mut r = SkillRouter::with_seed(7);
        let cands = s(&["alpha", "beta", "gamma"]);
        let pick = r
            .choose(SkillRouterInput {
                task: "use @@skill=beta please",
                candidates: &cands,
            })
            .unwrap();
        assert_eq!(pick, "beta");
    }

    #[test]
    fn override_outside_candidates_falls_back() {
        let mut r = SkillRouter::with_seed(7);
        let cands = s(&["alpha", "beta"]);
        let pick = r
            .choose(SkillRouterInput {
                task: "@@skill=gamma here",
                candidates: &cands,
            })
            .unwrap();
        assert!(pick == "alpha" || pick == "beta");
    }

    #[test]
    fn seed_from_prioritizer_gives_top_names_head_start() {
        let ranked = s(&["best", "good", "okay", "meh", "bad"]);
        let cands = ranked.clone();
        let mut best_picks = 0;
        for i in 0..200u64 {
            // Each iteration gets an independent RNG seed so the picks
            // sample the posterior space rather than re-running one draw.
            let mut r2 = SkillRouter::with_seed(101 + i);
            r2.seed_from_prioritizer(&ranked);
            if r2
                .choose(SkillRouterInput {
                    task: "task",
                    candidates: &cands,
                })
                .unwrap()
                == "best"
            {
                best_picks += 1;
            }
        }
        // Statistical: with a 3:0 head-start vs cold (0:0 or 1:0) the
        // top arm should win > random share. Be lenient: >= 50/200.
        assert!(
            best_picks >= 50,
            "expected best >= 50/200, got {best_picks}"
        );
    }

    #[test]
    fn online_observation_updates_scorer() {
        let mut r = SkillRouter::with_seed(2026);
        let cands = s(&["alpha", "beta"]);

        // Train alpha as strong arm.
        for _ in 0..50 {
            r.observe(&"alpha".to_string(), TaskOutcome::Success);
        }
        for _ in 0..50 {
            r.observe(&"beta".to_string(), TaskOutcome::Failure);
        }

        let mut alpha = 0;
        let mut beta = 0;
        for _ in 0..500 {
            match r
                .choose(SkillRouterInput {
                    task: "task",
                    candidates: &cands,
                })
                .unwrap()
                .as_str()
            {
                "alpha" => alpha += 1,
                "beta" => beta += 1,
                _ => unreachable!(),
            }
        }
        assert!(alpha > beta, "alpha={alpha} beta={beta}");
    }

    #[test]
    fn restore_seeds_empty_input_is_noop() {
        let mut r = SkillRouter::with_seed(7);
        let seeded = r.restore_seeds(std::iter::empty::<(String, u64)>());
        assert_eq!(seeded, 0);
        assert_eq!(r.scorer.iter_stats().count(), 0);
    }

    #[test]
    fn restore_seeds_skips_zero_success() {
        let mut r = SkillRouter::with_seed(7);
        let seeded = r.restore_seeds(vec![
            ("a".to_string(), 0u64),
            ("b".to_string(), 3u64),
            ("c".to_string(), 0u64),
        ]);
        assert_eq!(seeded, 1);
        let stats: Vec<(String, u64)> = r
            .scorer
            .iter_stats()
            .map(|(k, v)| (k.clone(), v.success))
            .collect();
        assert_eq!(stats, vec![("b".to_string(), 3)]);
    }

    #[test]
    fn restore_seeds_seeds_high_score_arm() {
        let mut r = SkillRouter::with_seed(7);
        // Simulates a top score of 0.9 → round(0.9*5) = 5.
        let seeded = r.restore_seeds(vec![("alpha".to_string(), 5u64)]);
        assert_eq!(seeded, 1);
        let alpha_success = r
            .scorer
            .iter_stats()
            .find(|(k, _)| k.as_str() == "alpha")
            .map(|(_, v)| v.success)
            .expect("alpha should be present after restore_seeds");
        assert!(
            alpha_success >= 4,
            "expected alpha.success >= 4, got {alpha_success}"
        );
    }

    #[test]
    fn restore_seeds_skips_names_already_seeded_by_prioritizer() {
        let mut r = SkillRouter::with_seed(7);
        // Prioritizer seeds "alpha" first (head-start = 3 for top of a
        // 1-element list, since i < n/4 is i < 0 → falls to else branch
        // → 0). Use a list where "alpha" lands in the top quartile.
        let ranked = s(&["alpha", "x", "y", "z", "w", "u", "v", "t"]);
        r.seed_from_prioritizer(&ranked);
        let alpha_before = r
            .scorer
            .iter_stats()
            .find(|(k, _)| k.as_str() == "alpha")
            .map(|(_, v)| v.success)
            .unwrap_or(0);
        // Now try to re-seed alpha via restore_seeds with a higher value.
        let seeded = r.restore_seeds(vec![("alpha".to_string(), 5u64)]);
        let alpha_after = r
            .scorer
            .iter_stats()
            .find(|(k, _)| k.as_str() == "alpha")
            .map(|(_, v)| v.success)
            .unwrap_or(0);
        assert_eq!(seeded, 0, "should not re-seed an existing arm");
        assert_eq!(
            alpha_before, alpha_after,
            "existing alpha must remain untouched"
        );
    }

    #[test]
    fn restore_seeds_biases_picks_toward_higher_scores() {
        // Three arms: weak (0.1 → 0 → no seed), mid (0.5 → 2), strong
        // (0.95 → 5). Across 200 independent draws, strong should win
        // more than weak.
        let cands = s(&["weak", "mid", "strong"]);
        let mut strong_picks = 0;
        let mut weak_picks = 0;
        for i in 0..200u64 {
            let mut r = SkillRouter::with_seed(101 + i);
            let seeded = r.restore_seeds(vec![
                ("weak".to_string(), 0u64),
                ("mid".to_string(), 2u64),
                ("strong".to_string(), 5u64),
            ]);
            // weak's 0 is skipped so only 2 arms seeded.
            assert_eq!(seeded, 2);
            let pick = r
                .choose(SkillRouterInput {
                    task: "task",
                    candidates: &cands,
                })
                .unwrap();
            match pick.as_str() {
                "strong" => strong_picks += 1,
                "weak" => weak_picks += 1,
                _ => {}
            }
        }
        assert!(
            strong_picks > weak_picks,
            "expected strong>weak, got strong={strong_picks} weak={weak_picks}"
        );
    }

    #[test]
    fn seed_idempotent_on_repeat_call() {
        let mut r = SkillRouter::with_seed(7);
        let ranked = s(&["a", "b", "c"]);
        r.seed_from_prioritizer(&ranked);
        let before: Vec<(String, u64)> = r
            .scorer
            .iter_stats()
            .map(|(k, v)| (k.clone(), v.success))
            .collect();
        r.seed_from_prioritizer(&ranked); // second call should not change anything
        let after: Vec<(String, u64)> = r
            .scorer
            .iter_stats()
            .map(|(k, v)| (k.clone(), v.success))
            .collect();
        // Sort by name for comparison stability.
        let mut b = before.clone();
        let mut a = after.clone();
        b.sort();
        a.sort();
        assert_eq!(b, a);
    }
}
