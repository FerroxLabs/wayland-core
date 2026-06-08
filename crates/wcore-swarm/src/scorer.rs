//! Scorer trait used by [`crate::consensus::Consensus`] + [`crate::debate::Debate`].
//!
//! A `Scorer` maps a [`SwarmResult`] to a comparable bucket key. Workers
//! whose results bucket to the same key are treated as agreeing.
//!
//! Two implementations ship in-tree:
//! - [`RuleBasedScorer::exact_stdout`] — byte-exact stdout match.
//! - [`RuleBasedScorer::normalized_stdout`] — trim + lowercase.
//!
//! LLM-judge scoring is a separate impl that lives downstream in
//! `wcore-agent` (out of scope for M5.6, per the plan).

use crate::SwarmResult;

/// Maps a [`SwarmResult`] to a comparable bucket key. Implementations
/// must be deterministic for a given result.
pub trait Scorer: Send + Sync {
    /// Return the bucket key for `result`. Workers whose key is identical
    /// are counted as agreeing.
    fn bucket(&self, result: &SwarmResult) -> String;
}

/// Built-in scorer that buckets by stdout content. Use one of the
/// constructor functions; the underlying mode is intentionally not
/// exposed so we can add new modes without a breaking change.
pub struct RuleBasedScorer {
    mode: ScoreMode,
}

enum ScoreMode {
    ExactStdout,
    NormalizedStdout,
}

impl RuleBasedScorer {
    /// Bucket by the worker's stdout byte-for-byte. Suitable when
    /// workers emit a single canonical answer with no incidental
    /// whitespace differences.
    pub fn exact_stdout() -> Self {
        Self {
            mode: ScoreMode::ExactStdout,
        }
    }

    /// Bucket by trimmed + lowercased stdout. Tolerates trailing
    /// newlines, case drift, and surrounding whitespace.
    pub fn normalized_stdout() -> Self {
        Self {
            mode: ScoreMode::NormalizedStdout,
        }
    }
}

impl Scorer for RuleBasedScorer {
    fn bucket(&self, result: &SwarmResult) -> String {
        match self.mode {
            ScoreMode::ExactStdout => result.stdout.clone(),
            ScoreMode::NormalizedStdout => result.stdout.trim().to_lowercase(),
        }
    }
}
