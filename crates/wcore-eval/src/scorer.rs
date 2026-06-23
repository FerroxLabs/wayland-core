//! Deterministic scoring for the W10A eval harness.
//!
//! `Scorer` trait + `DefaultScorer` implementation. Three weighted
//! components combine into [0.0, 1.0]; a binary `Verdict` is predicted
//! by comparing the combined score to a fixed acceptance cutoff:
//!
//! - **outcome correctness** (weight 0.7): 9 structural checks against
//!   the skill body and frontmatter (see `score_outcome`).
//! - **cost penalty** (weight 0.2): normalized blend of `cost_usd` and
//!   `output_tokens` against fixed W10A saturation constants.
//!   Inversely contributes: `0.2 * (1 - cost_penalty)`.
//! - **size penalty** (weight 0.1): `content_length` against a 2KB
//!   reference. Inversely contributes: `0.1 * (1 - size_penalty)`.
//!
//! **Determinism:** NO LLM calls, NO randomness, NO time/clock reads,
//! NO file I/O. Given an identical `Candidate` the function returns
//! bit-identical `ScoreDimensions` values. Asserted via
//! `f64::to_bits` equality in `tests/scoring_determinism.rs`.
//!
//! **Constants LOCKED at end of Task 3.** Per the plan, post-hoc
//! tuning of saturation constants or the acceptance cutoff after
//! observing gate failures is forbidden — remedies are additive
//! structural checks or case re-authoring (see plan Task 5).
//!
//! Wave RC (audit MAJOR #10) — constants live in the private
//! [`DefaultScorerConstants`] struct and are exposed only through the
//! const `LOCKED`. The fields are NOT individually `pub`, so callers
//! cannot mutate or shadow them; any drift is caught by the SHA-256
//! pinning test in `tests/locked_constants_test.rs`.

use serde::Serialize;

use wcore_observability::trace::TurnTrace;
use wcore_skills::types::SkillMetadata;

use crate::corpus::{Candidate, Verdict};

/// Three named scoring axes plus their weighted combination.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScoreDimensions {
    pub outcome: f64,
    pub cost_penalty: f64,
    pub size_penalty: f64,
    pub combined: f64,
}

/// What a `Scorer` returns for one `Candidate`.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScoreOutcome {
    pub dimensions: ScoreDimensions,
    pub predicted: Verdict,
}

/// The W10A scoring contract. `DefaultScorer` is the W10A
/// implementation; W10B's GEPA loop may swap in alternatives behind
/// a feature flag.
pub trait Scorer {
    fn score(&self, candidate: &Candidate) -> ScoreOutcome;
}

/// Immutable bundle of LOCKED scoring constants (audit MAJOR #10).
///
/// Fields are crate-private; outside callers read them through
/// [`DefaultScorer`] accessors only. The SHA-256 of these fields is
/// hard-pinned by `tests/locked_constants_test.rs`; any change here
/// MUST be intentional and MUST update the pinned hash AFTER
/// re-running the acceptance gate.
#[derive(Debug, Clone)]
pub struct DefaultScorerConstants {
    w_outcome: f64,
    w_cost: f64,
    w_size: f64,
    cost_saturate_usd: f64,
    tokens_saturate: u64,
    size_saturate_bytes: usize,
    acceptance_cutoff: f64,
    model_allowlist: &'static [&'static str],
}

impl DefaultScorerConstants {
    /// Read-only accessors. Kept `pub` so tests + the pinning hash
    /// computation can read constant values without going through a
    /// constructed `DefaultScorer`.
    pub const fn w_outcome(&self) -> f64 {
        self.w_outcome
    }
    pub const fn w_cost(&self) -> f64 {
        self.w_cost
    }
    pub const fn w_size(&self) -> f64 {
        self.w_size
    }
    pub const fn cost_saturate_usd(&self) -> f64 {
        self.cost_saturate_usd
    }
    pub const fn tokens_saturate(&self) -> u64 {
        self.tokens_saturate
    }
    pub const fn size_saturate_bytes(&self) -> usize {
        self.size_saturate_bytes
    }
    pub const fn acceptance_cutoff(&self) -> f64 {
        self.acceptance_cutoff
    }
    pub const fn model_allowlist(&self) -> &'static [&'static str] {
        self.model_allowlist
    }
}

/// The W10A LOCKED scoring constants. Any drift is caught by
/// `tests/locked_constants_test.rs`.
pub const LOCKED: DefaultScorerConstants = DefaultScorerConstants {
    w_outcome: 0.7,
    w_cost: 0.2,
    w_size: 0.1,
    cost_saturate_usd: 0.05,
    tokens_saturate: 2_000,
    size_saturate_bytes: 2_048,
    acceptance_cutoff: 0.65,
    model_allowlist: &["claude-sonnet-4-7", "claude-opus-4-7", "claude-haiku-4-5"],
};

/// Deterministic W10A scorer. All constants LOCKED at end of Task 3.
///
/// Wave RC: the struct itself carries no mutable scoring state; every
/// scoring call reads from the module-level [`LOCKED`] bundle so
/// constants cannot be tweaked post-construction.
#[derive(Debug, Clone, Default)]
pub struct DefaultScorer {
    _seal: (),
}

impl DefaultScorer {
    /// Construct a `DefaultScorer`. Equivalent to `DefaultScorer::default()`
    /// but reads cleaner at call sites.
    pub const fn new() -> Self {
        Self { _seal: () }
    }

    /// Read-only view onto the LOCKED constants. Hosts that need to
    /// surface acceptance-gate thresholds or model allowlists (e.g.
    /// for diagnostics) read them through this accessor rather than a
    /// `pub` field, so the pinning test catches any drift centrally.
    pub const fn constants(&self) -> &'static DefaultScorerConstants {
        &LOCKED
    }
}

impl Scorer for DefaultScorer {
    fn score(&self, candidate: &Candidate) -> ScoreOutcome {
        let outcome = self.score_outcome(&candidate.skill, &candidate.source_filename);
        let cost_penalty = match candidate.trace.as_ref() {
            Some(t) => self.score_cost(t),
            None => 0.0, // no trace = no cost penalty
        };
        let size_penalty = self.score_size(&candidate.skill);
        let combined = LOCKED.w_outcome * outcome
            + LOCKED.w_cost * (1.0 - cost_penalty)
            + LOCKED.w_size * (1.0 - size_penalty);
        // Clamp belt-and-suspenders.
        let combined = combined.clamp(0.0, 1.0);

        let predicted = if combined >= LOCKED.acceptance_cutoff {
            Verdict::Good
        } else {
            Verdict::Bad
        };

        ScoreOutcome {
            dimensions: ScoreDimensions {
                outcome,
                cost_penalty,
                size_penalty,
                combined,
            },
            predicted,
        }
    }
}

impl DefaultScorer {
    /// 9 structural checks. Each failure trims an equal share off the
    /// outcome score; 9 checks => 9 stops of 1/9 each.
    ///
    /// (Audit F6 added checks 7, 8, 9 to cover corruption families
    /// 3 / 4 / 9 that previously had no deterministic signal.)
    fn score_outcome(&self, skill: &SkillMetadata, source_filename: &str) -> f64 {
        let checks: [bool; 9] = [
            // 1. $ARGUMENTS placeholder present in body.
            skill.content.contains("$ARGUMENTS"),
            // 2. description present and distinct from body (after trim).
            !skill.description.trim().is_empty()
                && skill.description.trim() != skill.content.trim(),
            // 3. when_to_use populated.
            skill
                .when_to_use
                .as_ref()
                .is_some_and(|w| !w.trim().is_empty()),
            // 4. name non-empty.
            !skill.name.trim().is_empty(),
            // 5. no disallowed-tool reference in body.
            !mentions_disallowed_tool(skill),
            // 6. body non-empty.
            !skill.content.trim().is_empty(),
            // 7. name matches the source filename (audit F6).
            skill.name.trim() == source_filename.trim(),
            // 8. description shares at least one non-stopword token with the body (audit F6).
            description_shares_token_with_body(skill),
            // 9. model pin in W10A allowlist (or no pin) (audit F6).
            match &skill.model {
                None => true, // no pin = healthy default
                Some(m) => LOCKED.model_allowlist.iter().any(|allowed| allowed == m),
            },
        ];
        let passed = checks.iter().filter(|&&b| b).count();
        passed as f64 / checks.len() as f64
    }

    pub(crate) fn score_cost(&self, trace: &TurnTrace) -> f64 {
        let usd_term = (trace.cost_usd / LOCKED.cost_saturate_usd).clamp(0.0, 1.0);
        let tok_term = (trace.output_tokens as f64 / LOCKED.tokens_saturate as f64).clamp(0.0, 1.0);
        // 50/50 mix; trace-paired cases exercise both axes.
        0.5 * usd_term + 0.5 * tok_term
    }

    fn score_size(&self, skill: &SkillMetadata) -> f64 {
        let raw = skill.content_length as f64 / LOCKED.size_saturate_bytes as f64;
        raw.clamp(0.0, 1.0)
    }
}

/// True if the skill body references a tool not in `allowed_tools`.
/// W10A: substring match on canonical tool names; W10B may upgrade
/// to AST-level inspection.
fn mentions_disallowed_tool(skill: &SkillMetadata) -> bool {
    const TOOLS: &[&str] = &["Spawn", "Bash", "Edit", "Write", "Read", "Grep", "Glob"];
    for tool in TOOLS {
        let body_mentions = skill.content.contains(tool);
        let allowed = skill.allowed_tools.iter().any(|t| t == tool);
        if body_mentions && !allowed {
            return true;
        }
    }
    false
}

/// Description-relevance proxy: returns true iff the description and
/// the body share at least one non-stopword token (>=4 chars,
/// lowercased, ascii-only). Catches "off-topic description"
/// corruptions where the description describes an unrelated capability.
fn description_shares_token_with_body(skill: &SkillMetadata) -> bool {
    const STOPWORDS: &[&str] = &[
        "the", "this", "that", "with", "from", "into", "your", "have", "skill", "user", "when",
        "what", "where", "which", "would", "should", "could", "their", "there", "about",
    ];
    fn tokens(s: &str) -> std::collections::HashSet<String> {
        s.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| t.len() >= 4 && !STOPWORDS.contains(t))
            .map(|t| t.to_owned())
            .collect()
    }
    let d = tokens(&skill.description);
    let b = tokens(&skill.content);
    if d.is_empty() || b.is_empty() {
        // empty description is already caught by the "description non-empty"
        // check above; here we err Healthy to avoid double-counting.
        return true;
    }
    d.intersection(&b).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one_in_default() {
        assert!((LOCKED.w_outcome + LOCKED.w_cost + LOCKED.w_size - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cost_penalty_saturates_at_one() {
        let s = DefaultScorer::default();
        let mut t = TurnTrace {
            turn: 0,
            model: "m".into(),
            provider: "p".into(),
            input_tokens: 0,
            output_tokens: 1_000_000,
            cache_read: 0,
            cache_write: 0,
            cache_hit_rate: 0.0,
            cost_usd: 100.0,
            tool_calls: vec![],
            hook_actions: vec![],
            source_product: "test".into(),
            agent_run_id: String::new(),
        };
        assert_eq!(s.score_cost(&t), 1.0);
        t.cost_usd = 0.0;
        t.output_tokens = 0;
        assert_eq!(s.score_cost(&t), 0.0);
    }
}
