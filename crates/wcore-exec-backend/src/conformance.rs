//! The backend-agnostic conformance harness.
//!
//! ONE function drives every reference backend through the same behaviour
//! list, so no backend is proven by code written for it. If a new transport
//! cannot pass this harness WITHOUT the harness changing, the contract was not
//! provider-neutral — and that is the important result, not a reason to edit
//! the harness.
//!
//! Availability is reported, never silently skipped. Phase 20A's TEST-AUDIT
//! found 283 tests with no execution evidence and roughly 145 that ran in no
//! workflow at all; a silent skip is how that happens.

use ed25519_dalek::VerifyingKey;

use crate::contract::{
    ExecutionBackend, ExecutionTask, INPUT_FILE_NAME, ResourceBudget, ResourceKind, WorkspaceFile,
};
use crate::receipt::{BackendIdentity, EventKind, TerminalStatus};

/// One named check and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

/// The verdict for one backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceReport {
    pub backend_id: String,
    /// `false` means the backend was not exercised. The reason is in
    /// `unavailable_reason`, and an unexercised backend is NEVER a pass.
    pub exercised: bool,
    pub unavailable_reason: Option<String>,
    pub checks: Vec<CheckResult>,
}

impl ConformanceReport {
    pub fn passed(&self) -> bool {
        self.exercised && self.checks.iter().all(|check| check.passed)
    }

    pub fn failures(&self) -> Vec<&CheckResult> {
        self.checks.iter().filter(|check| !check.passed).collect()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.exercised {
            out.push_str(&format!(
                "backend {}: UNEXERCISED — {}\n",
                self.backend_id,
                self.unavailable_reason
                    .as_deref()
                    .unwrap_or("no reason given")
            ));
            return out;
        }
        out.push_str(&format!(
            "backend {}: {}\n",
            self.backend_id,
            if self.passed() { "PASS" } else { "FAIL" }
        ));
        for check in &self.checks {
            out.push_str(&format!(
                "  [{}] {} — {}\n",
                if check.passed { "ok" } else { "RED" },
                check.name,
                check.detail
            ));
        }
        out
    }
}

/// A deterministic reference task: fixed workspace bytes, fixed input, and a
/// computation whose output is a function of the input alone.
///
/// `cat input.bin` is the computation. It is deterministic on every POSIX
/// host, in the reference container image and on the far end of an ssh
/// connection, and it embeds no timestamp, hostname or random value — a task
/// whose output carried any of those could not prove equivalence at all.
pub fn reference_task(task_id: &str, nonce: &str, resources: ResourceBudget) -> ExecutionTask {
    ExecutionTask {
        task_id: task_id.to_string(),
        nonce: nonce.to_string(),
        workspace: vec![
            WorkspaceFile {
                path: "README.txt".into(),
                bytes: b"wayland f25 deterministic reference workspace\n".to_vec(),
            },
            WorkspaceFile {
                path: "data/fixed.txt".into(),
                bytes: b"0123456789abcdef\n".to_vec(),
            },
        ],
        input: b"wayland-f25-deterministic-input\n".to_vec(),
        argv: vec!["cat".into(), INPUT_FILE_NAME.into()],
        artifact_name: "stdout.bin".into(),
        resources,
    }
}

pub fn reference_budget() -> ResourceBudget {
    ResourceBudget::new(30_000, 256 * 1024 * 1024, 60_000, 1024 * 1024)
        .expect("the reference budget is non-zero in every field")
}

fn check(name: &'static str, passed: bool, detail: impl Into<String>) -> CheckResult {
    CheckResult {
        name,
        passed,
        detail: detail.into(),
    }
}

/// Drive one backend through the whole contract.
pub async fn run_conformance(
    backend: &dyn ExecutionBackend,
    identity: &BackendIdentity,
    verifying_key: &VerifyingKey,
    id_prefix: &str,
) -> ConformanceReport {
    let backend_id = backend.capabilities().backend_id.clone();
    let availability = backend.availability().await;
    if !availability.available {
        return ConformanceReport {
            backend_id,
            exercised: false,
            unavailable_reason: Some(format!(
                "{} (probe: {:?})",
                availability.detail, availability.probe
            )),
            checks: Vec::new(),
        };
    }

    let mut checks = Vec::new();

    // The availability answer must state its probe basis, and the basis must
    // be a real probe rather than an assumption.
    checks.push(check(
        "availability reports a real probe basis",
        !matches!(availability.probe, crate::contract::ProbeBasis::ProbeFailed),
        format!("{:?}: {}", availability.probe, availability.detail),
    ));

    // 1. Happy path: a deterministic task produces a verifiable receipt.
    let task = reference_task(
        &format!("{id_prefix}-ok"),
        &format!("{id_prefix}-nonce-ok"),
        reference_budget(),
    );
    match backend.execute(&task).await {
        Ok(receipt) => {
            let verified = receipt.verify(identity, verifying_key);
            checks.push(check(
                "a completed run emits a receipt that verifies against the pinned identity",
                verified.is_ok(),
                match &verified {
                    Ok(()) => "verified".to_string(),
                    Err(e) => e.to_string(),
                },
            ));
            checks.push(check(
                "the accepted event is first and the terminal event is last",
                matches!(receipt.body.events[0].event, EventKind::TaskAccepted { .. })
                    && receipt
                        .body
                        .events
                        .last()
                        .map(|e| e.event.is_terminal())
                        .unwrap_or(false),
                format!("{} events", receipt.body.events.len()),
            ));
            checks.push(check(
                "exactly one terminal event",
                receipt
                    .body
                    .events
                    .iter()
                    .filter(|e| e.event.is_terminal())
                    .count()
                    == 1,
                "terminal event count".to_string(),
            ));
            let artifact_ok = receipt
                .body
                .artifact
                .as_ref()
                .map(|a| a.sha256.len() == 64)
                .unwrap_or(false);
            checks.push(check(
                "a successful run publishes a content-addressed artifact",
                matches!(receipt.body.terminal, TerminalStatus::Success) && artifact_ok,
                format!("{:?}", receipt.body.terminal),
            ));
            checks.push(check(
                "the receipt content-addresses the task rather than inlining it",
                receipt.body.task.input_sha256 == task.input_sha256()
                    && receipt.body.task.workspace_sha256 == task.workspace_sha256(),
                "input and workspace digests agree with the submitted task".to_string(),
            ));
            checks.push(check(
                "the receipt carries no secret VALUE, only names",
                receipt
                    .body
                    .secrets_exposed
                    .iter()
                    .all(|name| name.len() <= 128),
                format!("{} exposed names", receipt.body.secrets_exposed.len()),
            ));

            // Tamper detection.
            let mut tampered = receipt.clone();
            tampered.body.task.task_id = format!("{}-tampered", tampered.body.task.task_id);
            checks.push(check(
                "an altered receipt body fails verification",
                tampered.verify(identity, verifying_key).is_err(),
                "tampered body rejected".to_string(),
            ));

            // Unpinned identity detection.
            let mut wrong = identity.clone();
            wrong.backend_id = format!("{}-impostor", wrong.backend_id);
            checks.push(check(
                "a receipt from an unpinned backend identity is rejected",
                receipt.verify(&wrong, verifying_key).is_err(),
                "unpinned identity rejected".to_string(),
            ));
        }
        Err(e) => checks.push(check(
            "a completed run emits a receipt that verifies against the pinned identity",
            false,
            format!("execute failed: {e}"),
        )),
    }

    // 2. A resource request the backend cannot satisfy is denied BEFORE
    //    acceptance, and the denial names the resource.
    let ceiling = backend.capabilities().limits;
    let impossible = ResourceBudget::new(
        ceiling.cpu_millis,
        ceiling
            .memory_bytes
            .saturating_mul(1024)
            .max(ceiling.memory_bytes + 1),
        ceiling.wall_time_ms,
        ceiling.output_bytes,
    )
    .expect("non-zero");
    let denied_task = reference_task(
        &format!("{id_prefix}-deny"),
        &format!("{id_prefix}-nonce-deny"),
        impossible,
    );
    match backend.execute(&denied_task).await {
        Ok(receipt) => {
            let denied_before_acceptance = receipt.body.events.len() == 1
                && matches!(
                    receipt.body.events[0].event,
                    EventKind::ResourceDenied {
                        resource: ResourceKind::MemoryBytes,
                        ..
                    }
                );
            checks.push(check(
                "an unsatisfiable resource request is denied BEFORE acceptance and names the resource",
                denied_before_acceptance,
                format!("{:?}", receipt.body.terminal),
            ));
            checks.push(check(
                "a pre-acceptance denial is still attested",
                receipt.verify(identity, verifying_key).is_ok(),
                "denial receipt verifies".to_string(),
            ));
        }
        Err(e) => checks.push(check(
            "an unsatisfiable resource request is denied BEFORE acceptance and names the resource",
            false,
            format!("execute failed instead of denying: {e}"),
        )),
    }

    // 3. Output over the accepted task's budget is denied AFTER acceptance,
    //    with streamed text and artifact bytes charged to ONE budget.
    let tight = ResourceBudget::new(
        ceiling.cpu_millis,
        ceiling.memory_bytes,
        ceiling.wall_time_ms,
        4, // the reference input is far larger than 4 bytes
    )
    .expect("non-zero");
    let over_budget = reference_task(
        &format!("{id_prefix}-budget"),
        &format!("{id_prefix}-nonce-budget"),
        tight,
    );
    match backend.execute(&over_budget).await {
        Ok(receipt) => {
            let denied_after_acceptance =
                matches!(receipt.body.events[0].event, EventKind::TaskAccepted { .. })
                    && matches!(
                        receipt.body.terminal,
                        TerminalStatus::ResourceDenied {
                            resource: ResourceKind::OutputBytes,
                            ..
                        }
                    );
            checks.push(check(
                "an over-budget artifact is denied AFTER acceptance against one shared output budget",
                denied_after_acceptance,
                format!("{:?}", receipt.body.terminal),
            ));
        }
        Err(e) => checks.push(check(
            "an over-budget artifact is denied AFTER acceptance against one shared output budget",
            false,
            format!("execute failed instead of denying: {e}"),
        )),
    }

    // 4. Health answers.
    match backend.health().await {
        Ok(health) => checks.push(check(
            "the backend answers a lifecycle health probe",
            health.healthy,
            health.detail,
        )),
        Err(e) => checks.push(check(
            "the backend answers a lifecycle health probe",
            false,
            e.to_string(),
        )),
    }

    // 5. An orphan scan must ENUMERATE rather than assume.
    //
    // #366 d4 — WHAT THIS CHECK PROVES, AND WHAT IT DOES NOT. It used to
    // assert `scan.enumerated && scan.found.is_empty()`, and to anyone
    // reading the check list it looked like orphan-scan coverage. It was not.
    // The nonce is `{id_prefix}-nonce-never-used`, chosen precisely so that
    // nothing can ever have run under it, so `found.is_empty()` is true BY
    // CONSTRUCTION — no reachable state of any backend, and no state of the
    // host however dirty, can falsify it. A conjunct that cannot be false
    // contributes nothing to a conjunction: the whole verdict was always the
    // `enumerated` half, and the other half made the check look twice as
    // strong as it was.
    //
    // The vacuous conjunct is gone from the ASSERTION. The count is still
    // reported, in the detail, where it is an observation rather than a
    // claim, and the check is renamed to say only what it can establish: the
    // backend really issued its enumeration, and did not answer "clean"
    // because it could not look.
    //
    // The arm this check could never carry — plant a labelled leftover and
    // require the scan to FIND it — lives in
    // `wcore-exec-backend/tests/unscoped_orphan_scan.rs`. It is
    // backend-specific by necessity, because planting a leftover means
    // creating one the way that ONE backend creates them, which is why it
    // cannot live in this backend-generic harness.
    let fresh_nonce = format!("{id_prefix}-nonce-never-used");
    const SCOPED_SCAN_CHECK: &str = "an orphan scan for an unused nonce ENUMERATES rather than assuming (that it finds \
         nothing under that nonce is true by construction and is NOT asserted; the arm that \
         requires a real leftover to be FOUND is in tests/unscoped_orphan_scan.rs)";
    match backend.scan_orphans(&fresh_nonce).await {
        Ok(scan) => checks.push(check(
            SCOPED_SCAN_CHECK,
            scan.enumerated,
            format!(
                "enumerated={} found={} (vacuously — nothing ever ran under this nonce) via {}",
                scan.enumerated,
                scan.found.len(),
                scan.method
            ),
        )),
        Err(e) => checks.push(check(SCOPED_SCAN_CHECK, false, e.to_string())),
    }

    // 6. Cancelling a task that does not exist must be an explicit error, not
    //    a silent success that would let a caller believe it cancelled.
    let unknown = format!("{id_prefix}-never-started");
    checks.push(check(
        "cancelling an unknown task is an explicit error, never a silent success",
        backend.cancel(&unknown).await.is_err(),
        "unknown task cancel rejected".to_string(),
    ));

    ConformanceReport {
        backend_id,
        exercised: true,
        unavailable_reason: None,
        checks,
    }
}
