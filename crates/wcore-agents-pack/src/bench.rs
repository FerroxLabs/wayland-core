//! v0.7.0 Task 3.A.5: mini-bench eval cases per built-in agent.
//!
//! Each agent gets 3-5 short input/expected-category cases embedded at
//! compile-time via `include_str!`. The full CI wiring (regression
//! gate) is deferred to v0.8; for v0.7.0 the unit-test gate in this
//! module verifies every agent has at least 3 cases and every case
//! is well-formed JSON.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchCase {
    /// Short slug for the case (kebab-case, unique within an agent).
    pub id: String,
    /// User-side prompt the agent should respond to.
    pub input: String,
    /// Tags an LLM grader looks for in the response.
    pub expected_categories: Vec<String>,
    /// One-line rubric an LLM grader uses to score the response 0-5.
    pub rubric: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBenchSet {
    pub agent: String,
    pub cases: Vec<BenchCase>,
}

const BENCH_SOURCES: &[(&str, &str)] = &[
    ("architect", include_str!("../cases/architect.json")),
    ("debugger", include_str!("../cases/debugger.json")),
    (
        "security-auditor",
        include_str!("../cases/security-auditor.json"),
    ),
    (
        "refactor-buddy",
        include_str!("../cases/refactor-buddy.json"),
    ),
    ("qa-engineer", include_str!("../cases/qa-engineer.json")),
    (
        "deep-researcher",
        include_str!("../cases/deep-researcher.json"),
    ),
    ("fact-checker", include_str!("../cases/fact-checker.json")),
    ("copywriter", include_str!("../cases/copywriter.json")),
    (
        "technical-writer",
        include_str!("../cases/technical-writer.json"),
    ),
    ("humanizer", include_str!("../cases/humanizer.json")),
    (
        "incident-commander",
        include_str!("../cases/incident-commander.json"),
    ),
    ("deploy-pilot", include_str!("../cases/deploy-pilot.json")),
    (
        "brand-strategist",
        include_str!("../cases/brand-strategist.json"),
    ),
];

static CACHE: OnceLock<Vec<AgentBenchSet>> = OnceLock::new();

fn all() -> &'static [AgentBenchSet] {
    CACHE.get_or_init(|| {
        BENCH_SOURCES
            .iter()
            .map(|(name, src)| {
                serde_json::from_str::<AgentBenchSet>(src)
                    .unwrap_or_else(|e| panic!("invalid bench JSON for {name}: {e}"))
            })
            .collect()
    })
}

/// Return every agent's bench set in declaration order.
pub fn all_sets() -> Vec<AgentBenchSet> {
    all().to_vec()
}

/// Lookup a specific agent's bench set by name.
pub fn set_for(agent: &str) -> Option<AgentBenchSet> {
    all().iter().find(|s| s.agent == agent).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentPack;

    #[test]
    fn every_built_in_agent_has_a_bench_set() {
        let agents: Vec<String> = AgentPack::list().into_iter().map(|m| m.name).collect();
        for name in &agents {
            assert!(
                set_for(name).is_some(),
                "no bench set for built-in agent {name}",
            );
        }
    }

    #[test]
    fn every_set_has_at_least_three_cases() {
        for set in all_sets() {
            assert!(
                set.cases.len() >= 3,
                "{} has {} cases; minimum is 3",
                set.agent,
                set.cases.len()
            );
            assert!(
                set.cases.len() <= 5,
                "{} has {} cases; maximum is 5",
                set.agent,
                set.cases.len()
            );
        }
    }

    #[test]
    fn case_ids_unique_within_agent() {
        for set in all_sets() {
            let mut ids: Vec<_> = set.cases.iter().map(|c| c.id.clone()).collect();
            let count = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), count, "duplicate case ids in {}", set.agent);
        }
    }

    #[test]
    fn every_case_has_required_fields() {
        for set in all_sets() {
            for c in &set.cases {
                assert!(!c.id.is_empty(), "{}: empty id", set.agent);
                assert!(!c.input.is_empty(), "{}/{}: empty input", set.agent, c.id);
                assert!(!c.rubric.is_empty(), "{}/{}: empty rubric", set.agent, c.id);
                assert!(
                    !c.expected_categories.is_empty(),
                    "{}/{}: empty expected_categories",
                    set.agent,
                    c.id
                );
            }
        }
    }
}
