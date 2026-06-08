//! W10A acceptance gate. F12 GEPA (W10B) is BLOCKED until this passes.
//!
//! Threshold: precision >= 0.80 AND recall >= 0.80 against the 60-case
//! corpus (30 known-good + 30 known-bad), per design §5.3 line 1638:
//!
//!   "The harness, when given a corpus of 30 known-good + 30 known-bad
//!    skill candidates, scores them correctly (>80% precision, >80%
//!    recall) before any GEPA promotion is allowed."
//!
//! This test is BOTH `#[ignore]`'d AND gated on the `acceptance-gate`
//! feature, so `cargo nextest run --workspace` ignores it twice over.
//! Run via `just eval-gate` or
//! `vx cargo nextest run -p wcore-eval --features acceptance-gate \
//!   acceptance_gate_meets_precision_recall_threshold \
//!   --no-fail-fast --run-ignored only`.

#![cfg(feature = "acceptance-gate")]

use wcore_eval::Harness;

const P_MIN: f64 = 0.80;
const R_MIN: f64 = 0.80;

#[test]
#[ignore = "W10A acceptance gate — run via `just eval-gate`"]
fn acceptance_gate_meets_precision_recall_threshold() {
    let harness = Harness::from_manifest_dir().expect("load harness");
    let report = harness.run().expect("run harness");

    if !report.meets_threshold(P_MIN, R_MIN) {
        // Surface every disagreeing case so the operator can decide
        // whether to (a) add a structural check to score_outcome,
        // (b) re-author the offending case, or (c) escalate to
        // LLM-judge (out of W10A scope). Constant tuning is FORBIDDEN
        // post-Task 3 (audit F7).
        let disagreers: Vec<String> = report
            .by_case
            .iter()
            .filter(|c| !c.agreed)
            .map(|c| {
                format!(
                    "{} [{}, expected={:?}, predicted={:?}, score={:.3}]",
                    c.case_id, c.category, c.expected, c.predicted, c.score.dimensions.combined
                )
            })
            .collect();
        panic!(
            "W10A gate FAILED:\n  precision={:.3} (need >={:.2})\n  recall   ={:.3} (need >={:.2})\n  TP={} TN={} FP={} FN={}\nDisagreeing cases:\n  {}",
            report.precision,
            P_MIN,
            report.recall,
            R_MIN,
            report.true_positive,
            report.true_negative,
            report.false_positive,
            report.false_negative,
            disagreers.join("\n  ")
        );
    }

    eprintln!(
        "W10A gate PASSED: precision={:.3} (>={:.2}), recall={:.3} (>={:.2}), F1={:.3}",
        report.precision, P_MIN, report.recall, R_MIN, report.f1
    );
}
