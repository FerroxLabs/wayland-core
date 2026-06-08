//! Smoke tests for `Debate::evaluate`.
//!
//! Covers:
//! - rounds collapse to a majority -> `DebateOutcome::Converged`
//! - no round ever reaches majority -> `DebateOutcome::Diverged`
//! - first agreed round wins (later rounds are not consulted).

use std::time::Duration;

use wcore_swarm::consensus::{ConsensusOutcome, RuleBasedScorer};
use wcore_swarm::debate::{Debate, DebateOutcome, DebateRound};
use wcore_swarm::{SwarmResult, WorkerStatus};

fn mk_with_id(id: &str, out: &str) -> SwarmResult {
    SwarmResult {
        worker_id: id.into(),
        branch: format!("b-{id}"),
        status: WorkerStatus::Succeeded,
        stdout: out.into(),
        stderr: String::new(),
        duration: Duration::from_secs(1),
    }
}

#[test]
fn debate_converges_when_rounds_collapse_to_majority() {
    let rounds = vec![
        DebateRound {
            round: 1,
            results: vec![
                mk_with_id("a", "x"),
                mk_with_id("b", "y"),
                mk_with_id("c", "z"),
            ],
        },
        DebateRound {
            round: 2,
            results: vec![
                mk_with_id("a", "y"),
                mk_with_id("b", "y"),
                mk_with_id("c", "z"),
            ],
        },
        DebateRound {
            round: 3,
            results: vec![
                mk_with_id("a", "y"),
                mk_with_id("b", "y"),
                mk_with_id("c", "y"),
            ],
        },
    ];
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Debate::evaluate(&rounds, &scorer);
    match outcome {
        DebateOutcome::Converged {
            value,
            converged_at_round,
        } => {
            // Round 2 already has y with 2/3 (>50%), so that wins.
            assert_eq!(value, "y");
            assert_eq!(converged_at_round, 2);
        }
        other => panic!("expected Converged, got {other:?}"),
    }
}

#[test]
fn debate_returns_diverged_when_no_round_yields_majority() {
    let rounds = vec![
        DebateRound {
            round: 1,
            results: vec![mk_with_id("a", "x"), mk_with_id("b", "y")],
        },
        DebateRound {
            round: 2,
            results: vec![mk_with_id("a", "y"), mk_with_id("b", "x")],
        },
    ];
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Debate::evaluate(&rounds, &scorer);
    match outcome {
        DebateOutcome::Diverged {
            final_round_outcome,
            rounds,
        } => {
            assert_eq!(rounds, 2);
            assert!(matches!(
                final_round_outcome,
                ConsensusOutcome::Disputed { .. }
            ));
        }
        other => panic!("expected Diverged, got {other:?}"),
    }
}

#[test]
fn debate_first_agreed_round_short_circuits() {
    // Round 1 already has a majority (a,b -> "y", c -> "z"). Round 2 is
    // unanimous on a different value but must NOT override round 1.
    let rounds = vec![
        DebateRound {
            round: 1,
            results: vec![
                mk_with_id("a", "y"),
                mk_with_id("b", "y"),
                mk_with_id("c", "z"),
            ],
        },
        DebateRound {
            round: 2,
            results: vec![
                mk_with_id("a", "x"),
                mk_with_id("b", "x"),
                mk_with_id("c", "x"),
            ],
        },
    ];
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Debate::evaluate(&rounds, &scorer);
    match outcome {
        DebateOutcome::Converged {
            value,
            converged_at_round,
        } => {
            assert_eq!(value, "y");
            assert_eq!(converged_at_round, 1);
        }
        other => panic!("expected Converged at round 1, got {other:?}"),
    }
}

#[test]
fn debate_empty_rounds_is_diverged_zero() {
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Debate::evaluate(&[], &scorer);
    match outcome {
        DebateOutcome::Diverged {
            final_round_outcome,
            rounds,
        } => {
            assert_eq!(rounds, 0);
            assert!(matches!(
                final_round_outcome,
                ConsensusOutcome::Disputed { total: 0, .. }
            ));
        }
        other => panic!("expected Diverged with rounds=0, got {other:?}"),
    }
}
