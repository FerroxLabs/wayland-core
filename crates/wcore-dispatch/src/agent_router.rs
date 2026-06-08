//! Agent router — picks an agent name from the AgentPack registry given
//! a task description. Thompson-sampling backed; arm set is the union
//! of the configured allowlist (defaults to all built-in agents).
//!
//! Manual override: embed `@@agent=<name>` (case-insensitive on the
//! `@@agent=` prefix; the name itself is preserved verbatim so that
//! kebab-case persona names like `security-auditor` round-trip cleanly).
//! If the override names an arm in the allowlist, that arm is returned
//! directly; otherwise the scorer picks.
//!
//! This router does NOT instantiate sub-agents — that's the orchestrator's
//! job. It just picks a name. The orchestrator then resolves the name via
//! `wcore_agents_pack::AgentPack::get(name)`.

use crate::scorer::{BetaScorer, Scorer};
use crate::{DecisionRouter, RouterError, TaskOutcome};

/// Routes task descriptions to AgentPack persona names.
pub struct AgentRouter {
    scorer: BetaScorer<String>,
    arms: Vec<String>,
}

impl AgentRouter {
    /// Use every agent in the AgentPack as a candidate arm.
    pub fn new_with_all_agents() -> Self {
        let arms = wcore_agents_pack::AgentPack::names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();
        Self {
            scorer: BetaScorer::new(),
            arms,
        }
    }

    /// Allow only a subset of agent names. Names not in the AgentPack
    /// are silently dropped (so a stale allowlist doesn't break startup).
    pub fn with_allowlist<S: AsRef<str>>(allow: &[S]) -> Self {
        let registry: std::collections::HashSet<&'static str> =
            wcore_agents_pack::AgentPack::names().into_iter().collect();
        let arms: Vec<String> = allow
            .iter()
            .map(|s| s.as_ref().to_string())
            .filter(|s| registry.contains(s.as_str()))
            .collect();
        Self {
            scorer: BetaScorer::new(),
            arms,
        }
    }

    /// Deterministic tests.
    pub fn with_seed_and_arms(seed: u64, arms: Vec<String>) -> Self {
        Self {
            scorer: BetaScorer::with_seed(seed),
            arms,
        }
    }

    /// Read-only arm set (for diagnostics + CLI listing).
    pub fn arms(&self) -> &[String] {
        &self.arms
    }

    /// Parse `@@agent=<name>` from the input. The `<name>` is allowed
    /// kebab-case (a-z, 0-9, '-'). Whitespace, quote, comma, paren
    /// terminate the match.
    fn parse_override(input: &str) -> Option<String> {
        let lower = input.to_ascii_lowercase();
        let needle = "@@agent=";
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

impl DecisionRouter<String, &str> for AgentRouter {
    fn choose(&mut self, input: &str) -> Result<String, RouterError> {
        if let Some(name) = Self::parse_override(input)
            && self.arms.iter().any(|a| a == &name)
        {
            return Ok(name);
        }
        self.scorer.thompson_pick(&self.arms)
    }

    fn observe(&mut self, choice: &String, outcome: TaskOutcome) {
        self.scorer.record(choice, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_all_agents_has_arms_from_registry() {
        let r = AgentRouter::new_with_all_agents();
        let arms = r.arms();
        assert!(
            !arms.is_empty(),
            "AgentPack registry must yield at least one agent"
        );
        assert!(arms.iter().any(|a| a == "architect"));
    }

    #[test]
    fn allowlist_drops_unknown_names() {
        let r = AgentRouter::with_allowlist(&["architect", "nonexistent-agent", "deep-researcher"]);
        let arms = r.arms();
        assert_eq!(arms.len(), 2);
        assert!(arms.iter().any(|a| a == "architect"));
        assert!(arms.iter().any(|a| a == "deep-researcher"));
        assert!(!arms.iter().any(|a| a == "nonexistent-agent"));
    }

    #[test]
    fn override_honored_when_in_arms() {
        let mut r = AgentRouter::with_seed_and_arms(
            42,
            vec!["architect".into(), "debugger".into(), "qa-engineer".into()],
        );
        let pick = r.choose("please use @@agent=debugger here").unwrap();
        assert_eq!(pick, "debugger");
    }

    #[test]
    fn override_outside_arms_falls_back_to_scorer() {
        let mut r =
            AgentRouter::with_seed_and_arms(42, vec!["architect".into(), "debugger".into()]);
        let pick = r.choose("@@agent=qa-engineer please").unwrap();
        assert!(pick == "architect" || pick == "debugger");
    }

    #[test]
    fn kebab_case_names_round_trip() {
        let mut r = AgentRouter::with_seed_and_arms(
            42,
            vec!["security-auditor".into(), "refactor-buddy".into()],
        );
        let pick = r.choose("foo @@agent=security-auditor bar").unwrap();
        assert_eq!(pick, "security-auditor");
    }

    #[test]
    fn empty_arms_returns_no_candidates() {
        let mut r = AgentRouter::with_seed_and_arms(42, vec![]);
        assert!(matches!(
            r.choose("anything"),
            Err(RouterError::NoCandidates)
        ));
    }

    #[test]
    fn online_training_converges_to_strong_arm() {
        let mut r = AgentRouter::with_seed_and_arms(2026, vec!["good".into(), "bad".into()]);
        for _ in 0..50 {
            r.observe(&"good".to_string(), TaskOutcome::Success);
        }
        for _ in 0..50 {
            r.observe(&"bad".to_string(), TaskOutcome::Failure);
        }
        let mut good = 0;
        let mut bad = 0;
        for _ in 0..500 {
            match r.choose("task").unwrap().as_str() {
                "good" => good += 1,
                "bad" => bad += 1,
                _ => unreachable!(),
            }
        }
        assert!(
            good > bad,
            "strong arm should dominate: good={good} bad={bad}"
        );
    }

    #[test]
    fn malformed_override_falls_back() {
        let mut r = AgentRouter::with_seed_and_arms(42, vec!["architect".into()]);
        let pick = r.choose("@@agent= please").unwrap();
        assert_eq!(pick, "architect");
    }
}
