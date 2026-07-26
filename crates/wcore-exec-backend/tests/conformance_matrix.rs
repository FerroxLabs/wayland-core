//! Every reference backend available on this host is driven through the SAME
//! conformance harness, and every UNAVAILABLE backend is REPORTED with its
//! reason rather than skipped.
//!
//! A silent skip is how Phase 20A ended up with 283 tests carrying no
//! execution evidence. This test therefore always prints a full matrix, and
//! fails only on a backend that was exercised and failed — an honestly
//! reported unavailable surface is a result, not a red.

use wcore_exec_backend::conformance::{ConformanceReport, reference_budget, run_conformance};
use wcore_exec_backend::reference_backends;

fn temp_state() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    unsafe { std::env::set_var("WAYLAND_EXEC_BACKEND_STATE_DIR", dir.path()) };
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not() {
    let _state = temp_state();
    let backends = reference_backends(reference_budget()).expect("construct reference backends");
    assert_eq!(
        backends.len(),
        4,
        "the phase fences the reference set at exactly four backends"
    );

    let mut reports: Vec<ConformanceReport> = Vec::new();
    for reference in &backends {
        let id = reference.backend.capabilities().backend_id.clone();
        let report = run_conformance(
            reference.backend.as_ref(),
            &reference.identity,
            &reference.verifying_key,
            &format!("conf-{id}"),
        )
        .await;
        reports.push(report);
    }

    let mut matrix = String::from("\n=== F25 CONFORMANCE MATRIX ===\n");
    for report in &reports {
        matrix.push_str(&report.render());
    }
    println!("{matrix}");

    // At least one backend must actually have been exercised. A matrix where
    // everything is unavailable proves nothing and must not read as green.
    let exercised: Vec<&ConformanceReport> = reports.iter().filter(|r| r.exercised).collect();
    assert!(
        !exercised.is_empty(),
        "no reference backend was exercised on this host — the matrix proves nothing:{matrix}"
    );

    let failed: Vec<String> = reports
        .iter()
        .filter(|r| r.exercised && !r.passed())
        .map(|r| {
            format!(
                "{}: {}",
                r.backend_id,
                r.failures()
                    .iter()
                    .map(|c| format!("{} ({})", c.name, c.detail))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })
        .collect();
    assert!(
        failed.is_empty(),
        "exercised backends failed the shared harness: {failed:#?}{matrix}"
    );
}

#[tokio::test]
async fn the_local_backend_is_always_exercised_because_it_needs_nothing_external() {
    let _state = temp_state();
    let backends = reference_backends(reference_budget()).expect("construct");
    let local = backends
        .iter()
        .find(|b| b.backend.capabilities().backend_id == "local")
        .expect("a local reference backend exists");
    let report = run_conformance(
        local.backend.as_ref(),
        &local.identity,
        &local.verifying_key,
        "local-always",
    )
    .await;
    println!("{}", report.render());
    assert!(
        report.exercised,
        "the local backend depends on nothing external and must always be exercised: {:?}",
        report.unavailable_reason
    );
    assert!(
        report.passed(),
        "local conformance failures: {:#?}",
        report.failures()
    );
}
