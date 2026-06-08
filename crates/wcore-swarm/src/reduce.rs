//! User-selectable reducer modes over a collected `Vec<SwarmResult>`.
//!
//! The subprocess swarm ([`crate::Swarm::dispatch`] + [`crate::Swarm::collect`])
//! produces a flat `Vec<SwarmResult>`. Callers (the `wayland swarm` CLI, the
//! orchestrator) historically printed those raw. This module adds a single
//! selector — [`ReduceMode`] — that routes the same `Vec<SwarmResult>` through
//! one of four reducers:
//!
//! - [`ReduceMode::Mesh`]     — flat passthrough (the historical behaviour).
//! - [`ReduceMode::Fleet`]    — a success/failure roll-up summary.
//! - [`ReduceMode::Consensus`] — [`Consensus::majority`] over the results.
//! - [`ReduceMode::Debate`]   — [`Debate::evaluate`] treating the collected
//!   batch as a single round.
//!
//! Consensus/Debate need a [`Scorer`]; [`reduce`] uses the supplied one (the
//! CLI defaults to [`RuleBasedScorer::normalized_stdout`]). Mesh/Fleet ignore
//! the scorer.

use serde::{Deserialize, Serialize};

use crate::consensus::{Consensus, ConsensusOutcome};
use crate::debate::{Debate, DebateOutcome, DebateRound};
use crate::scorer::Scorer;
use crate::{SwarmResult, WorkerStatus};

/// Which reducer to apply to a collected `Vec<SwarmResult>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReduceMode {
    /// Flat passthrough — return every result verbatim. The historical
    /// (and default) behaviour.
    #[default]
    Mesh,
    /// Hierarchical roll-up: counts of succeeded / failed / total.
    Fleet,
    /// Strict-majority vote via [`Consensus::majority`].
    Consensus,
    /// Multi-round debate via [`Debate::evaluate`]. The collected batch is
    /// treated as a single round (round 1); the orchestrator owns true
    /// multi-round replay.
    Debate,
}

impl ReduceMode {
    /// Parse a mode from its lowercase wire name. Accepts the same strings
    /// the serde `rename_all = "lowercase"` representation emits.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "mesh" => Ok(Self::Mesh),
            "fleet" => Ok(Self::Fleet),
            "consensus" => Ok(Self::Consensus),
            "debate" => Ok(Self::Debate),
            other => Err(format!(
                "unknown reduce mode '{other}' (expected one of: mesh, fleet, consensus, debate)"
            )),
        }
    }
}

/// Output of [`reduce`], tagged by the mode that produced it. `Serialize`-able
/// so the CLI can print it as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum ReduceOutput {
    /// Every result, verbatim.
    Mesh { results: Vec<SwarmResult> },
    /// Success/failure roll-up.
    Fleet {
        total: usize,
        succeeded: usize,
        failed: usize,
    },
    /// Consensus outcome.
    Consensus { outcome: ConsensusOutcome },
    /// Debate outcome.
    Debate { outcome: DebateOutcome },
}

/// Reduce `results` according to `mode`. `scorer` is consulted only for the
/// [`ReduceMode::Consensus`] and [`ReduceMode::Debate`] branches.
pub fn reduce<S: Scorer>(mode: ReduceMode, results: Vec<SwarmResult>, scorer: &S) -> ReduceOutput {
    match mode {
        ReduceMode::Mesh => ReduceOutput::Mesh { results },
        ReduceMode::Fleet => {
            let total = results.len();
            let succeeded = results
                .iter()
                .filter(|r| matches!(r.status, WorkerStatus::Succeeded))
                .count();
            ReduceOutput::Fleet {
                total,
                succeeded,
                failed: total - succeeded,
            }
        }
        ReduceMode::Consensus => ReduceOutput::Consensus {
            outcome: Consensus::majority(&results, scorer),
        },
        ReduceMode::Debate => {
            // The collected batch is a single round; the orchestrator owns
            // true multi-round replay (see debate.rs module docs).
            let rounds = vec![DebateRound { round: 1, results }];
            ReduceOutput::Debate {
                outcome: Debate::evaluate(&rounds, scorer),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::RuleBasedScorer;
    use std::time::Duration;

    fn ok(out: &str) -> SwarmResult {
        SwarmResult {
            worker_id: "w".into(),
            branch: "b".into(),
            status: WorkerStatus::Succeeded,
            stdout: out.into(),
            stderr: String::new(),
            duration: Duration::from_secs(1),
        }
    }

    fn failed(out: &str) -> SwarmResult {
        let mut r = ok(out);
        r.status = WorkerStatus::Failed("boom".into());
        r
    }

    #[test]
    fn parse_round_trips_all_modes() {
        assert_eq!(ReduceMode::parse("mesh").unwrap(), ReduceMode::Mesh);
        assert_eq!(ReduceMode::parse("Fleet").unwrap(), ReduceMode::Fleet);
        assert_eq!(
            ReduceMode::parse(" CONSENSUS ").unwrap(),
            ReduceMode::Consensus
        );
        assert_eq!(ReduceMode::parse("debate").unwrap(), ReduceMode::Debate);
        assert!(ReduceMode::parse("nope").is_err());
    }

    #[test]
    fn mesh_passes_results_through() {
        let scorer = RuleBasedScorer::exact_stdout();
        let out = reduce(ReduceMode::Mesh, vec![ok("a"), ok("b")], &scorer);
        match out {
            ReduceOutput::Mesh { results } => assert_eq!(results.len(), 2),
            other => panic!("expected Mesh, got {other:?}"),
        }
    }

    #[test]
    fn fleet_counts_success_and_failure() {
        let scorer = RuleBasedScorer::exact_stdout();
        let out = reduce(
            ReduceMode::Fleet,
            vec![ok("a"), ok("b"), failed("c")],
            &scorer,
        );
        match out {
            ReduceOutput::Fleet {
                total,
                succeeded,
                failed,
            } => {
                assert_eq!(total, 3);
                assert_eq!(succeeded, 2);
                assert_eq!(failed, 1);
            }
            other => panic!("expected Fleet, got {other:?}"),
        }
    }

    #[test]
    fn consensus_mode_routes_to_majority() {
        let scorer = RuleBasedScorer::exact_stdout();
        let out = reduce(
            ReduceMode::Consensus,
            vec![ok("42"), ok("42"), ok("7")],
            &scorer,
        );
        match out {
            ReduceOutput::Consensus {
                outcome:
                    ConsensusOutcome::Agreed {
                        value,
                        votes,
                        total,
                    },
            } => {
                assert_eq!(value, "42");
                assert_eq!(votes, 2);
                assert_eq!(total, 3);
            }
            other => panic!("expected Consensus::Agreed, got {other:?}"),
        }
    }

    #[test]
    fn debate_mode_routes_to_evaluate_single_round() {
        let scorer = RuleBasedScorer::exact_stdout();
        let out = reduce(ReduceMode::Debate, vec![ok("y"), ok("y"), ok("z")], &scorer);
        match out {
            ReduceOutput::Debate {
                outcome:
                    DebateOutcome::Converged {
                        value,
                        converged_at_round,
                    },
            } => {
                assert_eq!(value, "y");
                assert_eq!(converged_at_round, 1);
            }
            other => panic!("expected Debate::Converged, got {other:?}"),
        }
    }

    #[test]
    fn debate_mode_diverges_without_majority() {
        let scorer = RuleBasedScorer::exact_stdout();
        let out = reduce(ReduceMode::Debate, vec![ok("a"), ok("b")], &scorer);
        match out {
            ReduceOutput::Debate {
                outcome: DebateOutcome::Diverged { rounds, .. },
            } => assert_eq!(rounds, 1),
            other => panic!("expected Debate::Diverged, got {other:?}"),
        }
    }
}
