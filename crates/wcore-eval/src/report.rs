//! Output shape of an eval-harness run. Designed to be CDC-friendly:
//! serializable to JSON, append-only, never references internal pointers.

use serde::Serialize;

use crate::corpus::{ExpectedOutcome, Verdict};
use crate::scorer::ScoreOutcome;

#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub case_id: String,
    pub category: String,
    pub expected: ExpectedOutcome,
    pub predicted: Verdict,
    pub agreed: bool,
    pub score: ScoreOutcome,
}

/// Aggregate metrics for one harness run.
///
/// Positive = predicted Good. Precision = TP / (TP + FP).
/// Recall = TP / (TP + FN). Both must be >= 0.80 to pass the gate
/// (per design §5.3 line 1638).
#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub total: usize,
    pub true_positive: usize,
    pub true_negative: usize,
    pub false_positive: usize,
    pub false_negative: usize,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    /// agreement_rate = (TP + TN) / total; informational, NOT the gate.
    pub agreement_rate: f64,
    pub by_case: Vec<CaseResult>,
}

impl EvalReport {
    /// Pass iff precision >= p_min AND recall >= r_min.
    pub fn meets_threshold(&self, p_min: f64, r_min: f64) -> bool {
        self.precision >= p_min && self.recall >= r_min
    }
}
