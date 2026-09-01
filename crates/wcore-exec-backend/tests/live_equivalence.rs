//! The repeatable, binary-driving form of the Success Criterion 1 proof.
//!
//! Gated behind an explicit opt-in (`--run-ignored only`) so the cheap CI
//! floor never dials a cloud vendor or an ssh host. The evidence file
//! `25-01-EQUIVALENCE-EVIDENCE.md` records the one-off operator run; this is
//! the regression form of the same claim.

use wcore_exec_backend::conformance::{reference_budget, reference_task};
use wcore_exec_backend::{ExecutionReceipt, normalized_equivalence, reference_backends};

/// A private state directory for THIS TEST, injected PER THREAD.
///
/// Deliberately NOT `WAYLAND_EXEC_BACKEND_STATE_DIR`. That variable is a
/// PROCESS global, and `cargo test` -- which the shared-process CI leg runs,
/// and which nextest's process-per-test can never see -- puts every test of
/// this binary on a thread of ONE process. The env var therefore pointed
/// every concurrently-running sibling's registry at this `TempDir`, which was
/// then deleted out from under them when it dropped. That is the race that
/// failed `conformance_matrix` on ci-linux at e37e72f0b: `reference_backends`
/// could not construct because its state dir had just been removed by a
/// sibling finishing first (gh#1233).
///
/// `StateDirGuard` is the per-thread override built for exactly this; see
/// `registry.rs`, whose doc comment names this defect. `fail_closed_matrix`
/// was migrated to it and this binary was not. Both halves are returned so
/// the directory outlives every read taken through the guard.
fn temp_state() -> (
    tempfile::TempDir,
    wcore_exec_backend::registry::StateDirGuard,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let guard = wcore_exec_backend::registry::StateDirGuard::set(dir.path());
    (dir, guard)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "drives real transports: opt in with --run-ignored only"]
async fn the_same_task_normalizes_equal_across_every_available_backend() {
    let _state = temp_state();
    let backends = reference_backends(reference_budget()).expect("construct");

    let mut receipts: Vec<ExecutionReceipt> = Vec::new();
    let mut ran: Vec<String> = Vec::new();
    let mut unexercised: Vec<String> = Vec::new();

    for reference in &backends {
        let id = reference.backend.capabilities().backend_id.clone();
        let availability = reference.backend.availability().await;
        if !availability.available {
            // Named with its reason, never silently skipped and never counted
            // as a pass.
            unexercised.push(format!(
                "{id}: UNEXERCISED — {} (probe {:?})",
                availability.detail, availability.probe
            ));
            continue;
        }
        // ONE deterministic task definition, byte-identical on every backend
        // INCLUDING its id and nonce. The task id is NOT excluded from the
        // normalized body — it is part of what four backends running the same
        // task must agree on — so suffixing it per backend would silently turn
        // one equivalence claim into four unrelated runs.
        let task = reference_task(
            "equiv-reference",
            "equiv-reference-nonce",
            reference_budget(),
        );
        match reference.backend.execute(&task).await {
            Ok(receipt) => {
                receipt
                    .verify(&reference.identity, &reference.verifying_key)
                    .expect("each receipt must verify individually");
                ran.push(id);
                receipts.push(receipt);
            }
            Err(e) => unexercised.push(format!("{id}: FAILED — {e}")),
        }
    }

    println!("ran: {ran:?}");
    println!("unexercised:\n  {}", unexercised.join("\n  "));

    assert!(
        receipts.len() >= 2,
        "an equivalence claim needs at least two real receipts; got {} ({unexercised:?})",
        receipts.len()
    );

    let (equivalent, differing) = normalized_equivalence(&receipts);
    assert!(
        equivalent,
        "normalized bodies diverged across {ran:?} on fields: {differing:?}"
    );

    // The digests must agree explicitly, not merely as a side effect of the
    // whole-body comparison — this is the assertion a future over-normalizing
    // change would have to defeat directly.
    let first = &receipts[0].body;
    for receipt in &receipts[1..] {
        assert_eq!(receipt.body.task.input_sha256, first.task.input_sha256);
        assert_eq!(
            receipt.body.task.workspace_sha256,
            first.task.workspace_sha256
        );
        assert_eq!(
            receipt.body.artifact.as_ref().map(|a| &a.sha256),
            first.artifact.as_ref().map(|a| &a.sha256)
        );
    }

    // And the fields that SHOULD differ must actually differ, or the
    // equivalence was manufactured by running the same backend twice.
    let transports: std::collections::BTreeSet<String> = receipts
        .iter()
        .map(|r| r.body.transport.kind.as_str().to_string())
        .collect();
    assert_eq!(
        transports.len(),
        receipts.len(),
        "every receipt must come from a DIFFERENT transport; got {transports:?}"
    );
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test live_equivalence`
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
         cargo test -p wcore-exec-backend --test live_equivalence -- --ignored --test-threads=1"
    );
}
