//! Smoke tests for `Consensus::majority`.
//!
//! Covers the three behaviors locked by the M5.6 plan:
//! - clear majority -> `ConsensusOutcome::Agreed`
//! - no majority -> `ConsensusOutcome::Disputed` with top-k buckets
//! - failed workers are excluded from the vote (and the total).

use std::time::Duration;

use wcore_swarm::consensus::{Consensus, ConsensusOutcome, RuleBasedScorer};
use wcore_swarm::{SwarmResult, WorkerStatus};

fn mk(out: &str) -> SwarmResult {
    SwarmResult {
        worker_id: "w".into(),
        branch: "b".into(),
        status: WorkerStatus::Succeeded,
        stdout: out.into(),
        stderr: String::new(),
        duration: Duration::from_secs(1),
    }
}

#[test]
fn majority_consensus_picks_dominant_output() {
    let results = vec![
        mk("answer: 42"),
        mk("answer: 42"),
        mk("answer: 42"),
        mk("answer: 7"),
        mk("answer: 13"),
    ];
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Consensus::majority(&results, &scorer);
    match outcome {
        ConsensusOutcome::Agreed {
            value,
            votes,
            total,
        } => {
            assert_eq!(value, "answer: 42");
            assert_eq!(votes, 3);
            assert_eq!(total, 5);
        }
        other => panic!("expected Agreed, got {other:?}"),
    }
}

#[test]
fn no_majority_surfaces_dispute_top_k() {
    let results = vec![mk("a"), mk("b"), mk("c"), mk("d")];
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Consensus::majority(&results, &scorer);
    match outcome {
        ConsensusOutcome::Disputed { top_k, total } => {
            // 4 distinct buckets, capped at top-3.
            assert_eq!(top_k.len(), 3);
            assert_eq!(total, 4);
        }
        other => panic!("expected Disputed, got {other:?}"),
    }
}

#[test]
fn failed_workers_excluded_from_vote() {
    let mut bad = mk("garbage");
    bad.status = WorkerStatus::Failed("boom".into());
    let results = vec![mk("ok"), mk("ok"), bad];
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Consensus::majority(&results, &scorer);
    match outcome {
        ConsensusOutcome::Agreed {
            value,
            votes,
            total,
        } => {
            assert_eq!(value, "ok");
            assert_eq!(votes, 2);
            assert_eq!(total, 2, "failed workers should not count in total");
        }
        other => panic!("expected Agreed, got {other:?}"),
    }
}

#[test]
fn all_failed_returns_empty_disputed() {
    let mut a = mk("x");
    a.status = WorkerStatus::Failed("boom".into());
    let mut b = mk("y");
    b.status = WorkerStatus::TimedOut;
    let results = vec![a, b];
    let scorer = RuleBasedScorer::exact_stdout();
    let outcome = Consensus::majority(&results, &scorer);
    match outcome {
        ConsensusOutcome::Disputed { top_k, total } => {
            assert!(top_k.is_empty());
            assert_eq!(total, 0);
        }
        other => panic!("expected Disputed with empty top_k, got {other:?}"),
    }
}

#[test]
fn normalized_scorer_buckets_whitespace_and_case() {
    let results = vec![mk("  YES\n"), mk("yes"), mk(" yes "), mk("no")];
    let scorer = RuleBasedScorer::normalized_stdout();
    let outcome = Consensus::majority(&results, &scorer);
    match outcome {
        ConsensusOutcome::Agreed {
            value,
            votes,
            total,
        } => {
            assert_eq!(value, "yes");
            assert_eq!(votes, 3);
            assert_eq!(total, 4);
        }
        other => panic!("expected Agreed, got {other:?}"),
    }
}
