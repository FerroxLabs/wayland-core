//! Template router — picks an orchestration template given task context.
//!
//! Five templates from `wcore-agent::orchestration`:
//!   * `Direct`          — single-agent call.
//!   * `Consensus`       — parallel fanout with majority joiner.
//!   * `SelfCritique`    — agent → critic loop.
//!   * `Adaptive`        — replan-on-result via [`ReplanFn`].
//!   * `Hierarchical`    — supervisor + delegated sub-graphs.
//!
//! The router learns from observed outcomes via a Thompson Beta scorer.
//! Callers can override the learned choice on a single input by embedding
//! `"@@template=<name>"` (case-insensitive) anywhere in the input — useful
//! for tests and for the upcoming `--template=` CLI flag.
//!
//! This module deliberately does NOT touch `wcore-agent::orchestration::
//! intent::IntentClassifier`. That keyword classifier still runs; wiring
//! the router in as a replacement is a separate post-Phase-4 integration
//! task (one-line swap once the orchestration substrate exposes a
//! `Box<dyn DecisionRouter>` extension point).

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::scorer::{BetaScorer, Scorer};
use crate::{DecisionRouter, RouterError, TaskOutcome};

/// Orchestration template choices. Mirrors the canonical names used by
/// `wcore-agent::orchestration::templates`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Template {
    Direct,
    Consensus,
    SelfCritique,
    Adaptive,
    Hierarchical,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplateParseError {
    #[error(
        "unknown template {0:?} (expected: direct, consensus, self_critique, adaptive, hierarchical)"
    )]
    Unknown(String),
}

impl Template {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Consensus => "consensus",
            Self::SelfCritique => "self_critique",
            Self::Adaptive => "adaptive",
            Self::Hierarchical => "hierarchical",
        }
    }

    pub fn all() -> Vec<Template> {
        vec![
            Self::Direct,
            Self::Consensus,
            Self::SelfCritique,
            Self::Adaptive,
            Self::Hierarchical,
        ]
    }
}

impl FromStr for Template {
    type Err = TemplateParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "direct" => Ok(Self::Direct),
            "consensus" => Ok(Self::Consensus),
            // accept both "self_critique" and "self-critique" + camelCase.
            "self_critique" | "self-critique" | "selfcritique" => Ok(Self::SelfCritique),
            "adaptive" => Ok(Self::Adaptive),
            "hierarchical" => Ok(Self::Hierarchical),
            other => Err(TemplateParseError::Unknown(other.to_string())),
        }
    }
}

/// Thompson-sampling router over [`Template`] arms.
pub struct TemplateRouter {
    scorer: BetaScorer<Template>,
    arms: Vec<Template>,
}

impl Default for TemplateRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateRouter {
    /// All five templates as candidate arms.
    pub fn new() -> Self {
        Self {
            scorer: BetaScorer::new(),
            arms: Template::all(),
        }
    }

    /// Restrict the candidate set (e.g. drop `Hierarchical` on single-host
    /// deploys until multi-host dispatch lands). Empty `arms` falls back
    /// to `Template::all()`.
    pub fn with_arms(arms: Vec<Template>) -> Self {
        let arms = if arms.is_empty() {
            Template::all()
        } else {
            arms
        };
        Self {
            scorer: BetaScorer::new(),
            arms,
        }
    }

    /// Deterministic constructor for tests.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            scorer: BetaScorer::with_seed(seed),
            arms: Template::all(),
        }
    }

    pub fn with_seed_and_arms(seed: u64, arms: Vec<Template>) -> Self {
        let arms = if arms.is_empty() {
            Template::all()
        } else {
            arms
        };
        Self {
            scorer: BetaScorer::with_seed(seed),
            arms,
        }
    }

    /// Try to extract a manual override of the form `@@template=<name>`
    /// (case-insensitive). Returns the parsed template or `None`.
    fn parse_override(input: &str) -> Option<Template> {
        let lower = input.to_ascii_lowercase();
        let needle = "@@template=";
        let idx = lower.find(needle)?;
        let tail = &lower[idx + needle.len()..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '"' || c == ',' || c == ')')
            .unwrap_or(tail.len());
        let name = &tail[..end];
        Template::from_str(name).ok()
    }
}

impl DecisionRouter<Template, &str> for TemplateRouter {
    fn choose(&mut self, input: &str) -> Result<Template, RouterError> {
        if let Some(t) = Self::parse_override(input) {
            // Honor the override only if it's in the configured arm set.
            if self.arms.contains(&t) {
                return Ok(t);
            }
        }
        self.scorer.thompson_pick(&self.arms)
    }

    fn observe(&mut self, choice: &Template, outcome: TaskOutcome) {
        self.scorer.record(choice, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::Stats;

    #[test]
    fn from_str_round_trips_for_all_variants() {
        for t in Template::all() {
            assert_eq!(t.as_str().parse::<Template>().unwrap(), t);
        }
    }

    #[test]
    fn manual_override_returns_named_template() {
        let mut r = TemplateRouter::with_seed(42);
        let pick = r
            .choose("please use @@template=consensus for this")
            .unwrap();
        assert_eq!(pick, Template::Consensus);
    }

    #[test]
    fn override_outside_arms_falls_back_to_scorer() {
        // Restrict arms to {Direct}; an override of @@template=consensus
        // must NOT bypass the restriction.
        let mut r = TemplateRouter::with_seed_and_arms(42, vec![Template::Direct]);
        let pick = r.choose("@@template=consensus please").unwrap();
        assert_eq!(pick, Template::Direct);
    }

    #[test]
    fn unknown_override_falls_back_to_scorer() {
        let mut r = TemplateRouter::with_seed(42);
        // "@@template=foobar" is not a known template; scorer picks one of the all-arms.
        let pick = r.choose("@@template=foobar do the thing").unwrap();
        assert!(Template::all().contains(&pick));
    }

    #[test]
    fn observed_outcomes_train_the_scorer() {
        // Force-train Adaptive as the strong arm; Direct as weak.
        let mut r = TemplateRouter::with_seed(1234);
        // We can't reach the inner scorer directly, but we can simulate
        // 50 + 1 success / failure observations via the public API.
        for _ in 0..50 {
            r.observe(&Template::Adaptive, TaskOutcome::Success);
        }
        r.observe(&Template::Adaptive, TaskOutcome::Failure);
        for _ in 0..50 {
            r.observe(&Template::Direct, TaskOutcome::Failure);
        }
        r.observe(&Template::Direct, TaskOutcome::Success);

        // Now Thompson-pick 500 times; Adaptive should dominate Direct.
        let mut adaptive = 0;
        let mut direct = 0;
        for _ in 0..500 {
            match r.choose("a generic task").unwrap() {
                Template::Adaptive => adaptive += 1,
                Template::Direct => direct += 1,
                _ => {}
            }
        }
        assert!(
            adaptive > direct,
            "Adaptive should beat Direct after training; got adaptive={adaptive}, direct={direct}"
        );
    }

    #[test]
    fn empty_arms_falls_back_to_all() {
        let mut r = TemplateRouter::with_seed_and_arms(7, vec![]);
        let pick = r.choose("anything").unwrap();
        assert!(Template::all().contains(&pick));
    }

    #[test]
    fn restricted_arms_never_picks_excluded() {
        let mut r =
            TemplateRouter::with_seed_and_arms(7, vec![Template::Direct, Template::Consensus]);
        for _ in 0..200 {
            let pick = r.choose("task").unwrap();
            assert!(pick == Template::Direct || pick == Template::Consensus);
        }
    }

    #[test]
    fn parse_override_terminators() {
        // Various delimiters should bound the override name.
        let cases = [
            ("@@template=direct ", Template::Direct),
            ("(@@template=consensus)", Template::Consensus),
            ("foo @@template=adaptive,bar", Template::Adaptive),
            ("\"@@template=hierarchical\"", Template::Hierarchical),
        ];
        for (input, expected) in cases {
            let mut r = TemplateRouter::with_seed(99);
            assert_eq!(r.choose(input).unwrap(), expected, "input: {input}");
        }
    }

    // Sanity: Stats type is reachable via crate path used by callers.
    #[allow(dead_code)]
    fn _stats_visibility_check(_s: Stats) {}
}
