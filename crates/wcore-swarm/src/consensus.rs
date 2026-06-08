//! Majority consensus over a `Vec<SwarmResult>`.
//!
//! [`Consensus::majority`] tallies successful workers into buckets via
//! a [`Scorer`], then either reports the dominant bucket
//! ([`ConsensusOutcome::Agreed`]) or surfaces the top-3 contending
//! buckets ([`ConsensusOutcome::Disputed`]) when no bucket has strictly
//! more than 50% of the successful votes.
//!
//! Failed / timed-out / cancelled workers are silently excluded from
//! both the tally and the `total`. If every worker failed, the outcome
//! is `Disputed { top_k: vec![], total: 0 }`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub use crate::scorer::{RuleBasedScorer, Scorer};
use crate::{SwarmResult, WorkerStatus};

/// Outcome of [`Consensus::majority`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusOutcome {
    /// Strictly more than 50% of *successful* workers bucketed to the
    /// same `value`. `votes` is the winning bucket size; `total` is the
    /// number of successful workers (so `votes * 2 > total`).
    Agreed {
        value: String,
        votes: usize,
        total: usize,
    },
    /// No bucket reached a strict majority. `top_k` lists up to 3
    /// leading `(bucket, votes)` pairs in descending vote order.
    /// `total` is the number of successful workers (0 if every worker
    /// failed).
    Disputed {
        top_k: Vec<(String, usize)>,
        total: usize,
    },
}

/// Stateless namespace for consensus algorithms.
pub struct Consensus;

impl Consensus {
    /// Strict-majority vote over `results`, bucketed by `scorer`.
    ///
    /// Semantics:
    /// - Only [`WorkerStatus::Succeeded`] workers contribute.
    /// - `total` = count of successful workers.
    /// - A bucket wins iff `bucket.votes * 2 > total` (>50%).
    /// - On no-majority, returns the top-3 buckets in descending vote
    ///   order. Tie-breaking inside the top-3 is `HashMap` iteration
    ///   order (deliberately unspecified — callers must not depend on
    ///   the relative order of equal-vote buckets).
    pub fn majority<S: Scorer>(results: &[SwarmResult], scorer: &S) -> ConsensusOutcome {
        let mut tally: HashMap<String, usize> = HashMap::new();
        let mut total = 0usize;
        for r in results {
            if !matches!(r.status, WorkerStatus::Succeeded) {
                continue;
            }
            total += 1;
            *tally.entry(scorer.bucket(r)).or_insert(0) += 1;
        }
        if total == 0 {
            return ConsensusOutcome::Disputed {
                top_k: vec![],
                total: 0,
            };
        }
        let mut entries: Vec<(String, usize)> = tally.into_iter().collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.1));
        // Safe: total > 0 implies at least one successful worker, which
        // implies at least one tally entry.
        let (top_value, top_votes) = entries
            .first()
            .cloned()
            .expect("tally non-empty when total > 0");
        if top_votes * 2 > total {
            ConsensusOutcome::Agreed {
                value: top_value,
                votes: top_votes,
                total,
            }
        } else {
            ConsensusOutcome::Disputed {
                top_k: entries.into_iter().take(3).collect(),
                total,
            }
        }
    }
}
