//! The repeatable, binary-driving form of the Success Criterion 1 proof.
//!
//! Gated behind an explicit opt-in (`--run-ignored only`) so the cheap CI
//! floor never dials a cloud vendor or an ssh host. The evidence file
//! `25-01-EQUIVALENCE-EVIDENCE.md` records the one-off operator run; this is
//! the regression form of the same claim.

use wcore_exec_backend::conformance::{reference_budget, reference_task};
use wcore_exec_backend::{ExecutionReceipt, normalized_equivalence, reference_backends};

fn temp_state() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    unsafe { std::env::set_var("WAYLAND_EXEC_BACKEND_STATE_DIR", dir.path()) };
    dir
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
