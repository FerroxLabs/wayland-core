//! Smoke: harness runs the corpus end-to-end without panicking. The
//! strict acceptance threshold is in tests/acceptance_gate.rs.

use wcore_eval::Harness;

#[test]
fn harness_runs_full_corpus() {
    let h = Harness::from_manifest_dir().expect("load");
    let report = h.run().expect("run");
    assert_eq!(report.total, 60);
    assert_eq!(report.by_case.len(), 60);
}
