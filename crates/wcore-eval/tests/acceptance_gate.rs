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

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test acceptance_gate`
/// executes 0 of 1 and still exits 0 printing `test result: ok`. This guard is
/// deliberately NOT `#[ignore]`d: three suites in this repo carried a guard that
/// was itself ignored, which made each inert against precisely the scenario it
/// existed for — it could only fire under `--ignored`, by which point the real
/// case were running anyway.
///
/// It always runs, so this binary can never report success on zero executed
/// tests, and it FAILS when a caller sets `WAYLAND_REQUIRE_IGNORED=1` to declare a run of the
/// ignored case while passing an invocation that cannot execute any of them.
/// Skipped under nextest, whose `no-tests = "fail"` policy covers the same
/// ground at the invocation site.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    if std::env::var("WAYLAND_REQUIRE_IGNORED").as_deref() != Ok("1") {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "declared intent to run this suite's 1 #[ignore]d case, but neither \
         --ignored nor --include-ignored was passed, so zero of them can execute. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-eval --test acceptance_gate -- --ignored --test-threads=1"
    );
}
