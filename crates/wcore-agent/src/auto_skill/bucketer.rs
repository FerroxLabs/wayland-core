//! v0.8.1 U6 — task-signature bucketing.
//!
//! Groups turns by a normalized signature (lowercased + tokenized +
//! stopword-stripped + top-3-content-words). N consecutive successes in
//! the same bucket triggers a `DraftTrigger`; any failure resets that
//! bucket's streak.
//!
//! Pure CPU + memory. The engine holds one of these behind a
//! `std::sync::Mutex` and calls `observe` at the end of every `run()`.

use std::collections::HashMap;

use super::recorder::{TurnOutcome, TurnTrajectory};

/// Stopwords stripped before content-word extraction. Kept small +
/// English-only on purpose: the goal is coarse grouping, not NLP.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "with", "for", "is", "are",
    "was", "were", "be", "been", "this", "that", "it", "i", "you", "we", "they", "do", "does",
    "did", "please", "can", "could", "should", "would", "my", "your", "from", "by", "at", "as",
    "if", "so", "not", "no", "yes",
];

/// Normalize an input string into a stable task signature.
///
/// Algorithm:
///   1. lowercase
///   2. tokenize on whitespace + ASCII punctuation
///   3. drop tokens shorter than 2 chars or in `STOPWORDS`
///   4. dedup while preserving first-seen order
///   5. take top 3 by length (longest first, ties broken by first-seen)
///   6. sort alphabetically + join with `-`
///
/// Returns the empty string when the input has no content words. The
/// caller treats empty signatures as "skip" — see `Bucketer::observe`.
pub fn signature(input: &str) -> String {
    let lowered = input.to_lowercase();
    let mut seen: Vec<String> = Vec::new();
    let mut seen_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for raw in lowered.split(|c: char| !c.is_alphanumeric()) {
        let tok = raw.trim();
        if tok.len() < 2 {
            continue;
        }
        if STOPWORDS.contains(&tok) {
            continue;
        }
        let s = tok.to_string();
        if seen_set.insert(s.clone()) {
            seen.push(s);
        }
    }
    if seen.is_empty() {
        return String::new();
    }
    // Stable secondary order: original index in `seen`. Primary: length DESC.
    let mut indexed: Vec<(usize, String)> = seen.into_iter().enumerate().collect();
    indexed.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
    let top: Vec<String> = indexed.into_iter().take(3).map(|(_, s)| s).collect();
    let mut sorted = top;
    sorted.sort();
    sorted.join("-")
}

/// Bucketed observation buffer. Cheap to construct; not thread-safe on
/// its own — callers wrap it in `std::sync::Mutex` if shared.
pub struct Bucketer {
    runs: HashMap<String, Vec<TurnTrajectory>>,
    threshold: usize,
}

impl Bucketer {
    /// Construct a fresh bucketer with the given consecutive-success
    /// threshold. The engine uses N=3.
    pub fn new(threshold: usize) -> Self {
        Self {
            runs: HashMap::new(),
            threshold: threshold.max(1),
        }
    }

    /// Record a trajectory. Returns `Some(DraftTrigger)` iff this turn
    /// completes an N-consecutive-success streak on the same signature.
    ///
    /// Behaviour:
    ///   - Empty signature → ignored (returns `None`, no streak side-effect).
    ///   - `Failure` outcome → drops the matching bucket (streak reset).
    ///   - `Success` outcome → pushes; if length >= threshold the bucket
    ///     is taken (cleared) and returned as the trigger.
    pub fn observe(&mut self, traj: TurnTrajectory) -> Option<DraftTrigger> {
        let sig = signature(&traj.user_input);
        if sig.is_empty() {
            return None;
        }
        if traj.outcome != TurnOutcome::Success {
            self.runs.remove(&sig);
            return None;
        }
        let bucket = self.runs.entry(sig.clone()).or_default();
        bucket.push(traj);
        if bucket.len() >= self.threshold {
            let trajectories = std::mem::take(bucket);
            // Don't leave the empty Vec dangling.
            self.runs.remove(&sig);
            return Some(DraftTrigger {
                signature: sig,
                trajectories,
            });
        }
        None
    }
}

/// Emitted by `Bucketer::observe` when a signature has accumulated the
/// required number of consecutive successes. The drafter consumes this.
#[derive(Debug, Clone)]
pub struct DraftTrigger {
    pub signature: String,
    pub trajectories: Vec<TurnTrajectory>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_skill::recorder::{TurnOutcome, TurnTrajectory};

    fn traj(input: &str, outcome: TurnOutcome) -> TurnTrajectory {
        TurnTrajectory {
            user_input: input.to_string(),
            picked_skill: None,
            outcome,
            summary: "test".into(),
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn signature_is_word_order_invariant() {
        // Same content words, different order + filler → same signature.
        let a = signature("review the refactor in this code");
        let b = signature("code refactor review please");
        assert_eq!(a, b, "expected stable signature; got a={a:?} b={b:?}");
        assert!(!a.is_empty(), "signature must be non-empty for valid input");
    }

    #[test]
    fn signature_strips_stopwords_and_short_tokens() {
        // "the/a/is" stopwords + "x" too-short → only "alpha", "beta", "gamma" survive.
        let s = signature("the alpha is a beta x with gamma");
        // Top 3 by length, alpha-sorted: alpha=5, beta=4, gamma=5 → alpha, gamma, beta.
        assert!(s.contains("alpha"));
        assert!(s.contains("beta"));
        assert!(s.contains("gamma"));
    }

    #[test]
    fn signature_handles_empty_input() {
        assert_eq!(signature(""), "");
        assert_eq!(signature("the a an"), "");
    }

    #[test]
    fn three_successes_same_signature_triggers_draft() {
        let mut b = Bucketer::new(3);
        assert!(
            b.observe(traj("refactor this code", TurnOutcome::Success))
                .is_none()
        );
        assert!(
            b.observe(traj("code refactor please", TurnOutcome::Success))
                .is_none()
        );
        let trigger = b
            .observe(traj("please refactor the code", TurnOutcome::Success))
            .expect("third success on matching signature must trigger draft");
        assert_eq!(trigger.trajectories.len(), 3);
        assert!(!trigger.signature.is_empty());
    }

    #[test]
    fn failure_resets_streak() {
        let mut b = Bucketer::new(3);
        assert!(
            b.observe(traj("refactor this code", TurnOutcome::Success))
                .is_none()
        );
        assert!(
            b.observe(traj("refactor this code", TurnOutcome::Success))
                .is_none()
        );
        // Failure on the SAME signature should drop the bucket.
        assert!(
            b.observe(traj("refactor this code", TurnOutcome::Failure))
                .is_none()
        );
        // Next success starts a fresh streak — must NOT trigger.
        assert!(
            b.observe(traj("refactor this code", TurnOutcome::Success))
                .is_none()
        );
    }

    #[test]
    fn different_signatures_keep_independent_streaks() {
        let mut b = Bucketer::new(3);
        b.observe(traj("refactor code", TurnOutcome::Success));
        b.observe(traj("write tests", TurnOutcome::Success));
        b.observe(traj("refactor code", TurnOutcome::Success));
        // Neither signature has hit 3 yet.
        // Third refactor-code SHOULD trigger.
        let trigger = b
            .observe(traj("code refactor", TurnOutcome::Success))
            .expect("3rd 'refactor code' success must trigger");
        assert!(trigger.signature.contains("code"));
        assert!(trigger.signature.contains("refactor"));
    }

    #[test]
    fn empty_signature_is_ignored() {
        let mut b = Bucketer::new(1);
        // All stopwords → empty signature → no trigger even at threshold=1.
        assert!(b.observe(traj("the a an", TurnOutcome::Success)).is_none());
    }

    #[test]
    fn trigger_clears_bucket_so_repeat_streak_must_rebuild() {
        let mut b = Bucketer::new(3);
        for _ in 0..3 {
            b.observe(traj("refactor code", TurnOutcome::Success));
        }
        // Next single success must NOT immediately retrigger.
        assert!(
            b.observe(traj("refactor code", TurnOutcome::Success))
                .is_none()
        );
    }
}
