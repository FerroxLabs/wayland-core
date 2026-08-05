//! 06D black-box proof — hostile gate evidence and abnormal termination fail
//! closed, through the crate's PUBLIC child-transaction surface only.
//!
//! Two boundaries are exercised:
//!
//! * The acceptance entry point `run_gate_acceptance` refuses when the durable
//!   transaction is absent or when the execution subject does not bind the
//!   orchestrator-owned durable gate plan — before any candidate code runs.
//! * The authoritative durable-receipt boundary (`ChildTransactionReceipt::
//!   validate_for_child` and `ChildTransactionStore::commit`, which reduces
//!   through the same validation) refuses every malformed, substituted,
//!   unknown, subject-drifted, or model-claimed gate receipt, and refuses
//!   conflicting / out-of-order receipts and a corrupted or rebound durable
//!   journal — with no durable effect.
//!
//! Abnormal termination (cancellation, timeout, process death, dropped guard,
//! restart) all funnel through the same RAII terminalization: dropping the
//! still-armed `MutationAttemptGuard` cleans the isolated checkout exactly once
//! and mutates no parent / durable state. That drop path is proven directly.
//!
//! Scope note (surfaced to the 20-14 audit): observed gate results and the
//! acceptance machine's order/count/duplicate enforcement are crate-private
//! (`ObservedGateResult` / `AcceptanceMachine`), so those hostile shapes are
//! proven here at the durable-receipt boundary they are ultimately reduced
//! through, which is the reachable public surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wcore_agent::child_transaction::{
    ChildTransactionStore, ChildTransactionWrite, GateExecutionSubject, MutationAcceptanceError,
    MutationAttemptGuard, run_gate_acceptance,
};
use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::session_journal::SessionJournal;
use wcore_sandbox::{FailClosedBackend, SandboxRegistry};
use wcore_swarm::worktree::{CandidateSeal, WorktreeManager};
use wcore_types::child_transaction::{
    CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION, ChildGateOutcome, ChildGatePlan, ChildGateReceipt,
    ChildGateRequirement, ChildGateSubject, ChildTransactionDisposition, ChildTransactionReceipt,
    ChildTransactionValidationError,
};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus,
};

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn revision(character: char) -> String {
    std::iter::repeat_n(character, 40).collect()
}

fn one_gate_plan() -> ChildGatePlan {
    ChildGatePlan {
        required_gates: vec![ChildGateRequirement {
            gate_id: "cargo-test".into(),
            gate_closure_digest: digest('c'),
        }],
    }
}

fn child_record() -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: "declare-child-1".into(),
        child_id: ChildId::new("child-1").unwrap(),
        parent: ChildParent {
            session_id: "session-1".into(),
            turn_id: None,
            parent_child_id: None,
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: None,
        },
        origin: ChildOrigin::Delegate,
        request: ChildRequestEvidence::redacted(digest('a')),
        policy_snapshot: ChildPolicySnapshot {
            contract_version: "effective-execution-policy/v1".into(),
            exact_digest: digest('b'),
            posture: "smart".into(),
            approvals: "on_request".into(),
            sandbox: "required".into(),
            source: "session-effective-policy".into(),
            managed_floor_active: true,
            dangerous_activation_id_digest: None,
        },
        provider: Some("test".into()),
        model: Some("test-model".into()),
        workspace: ChildWorkspace {
            mode: ChildWorkspaceMode::Isolated,
            workspace_id: "workspace-child-1".into(),
        },
        status: DurableChildStatus::Prepared,
        desired_state: ChildDesiredState::Run,
        recovery: ChildRecoveryState::Clean,
        revision: 0,
        timestamps: ChildTimestamps {
            created_at_unix_ms: 10,
            updated_at_unix_ms: 10,
            queued_at_unix_ms: None,
            started_at_unix_ms: None,
            terminal_at_unix_ms: None,
        },
        result: None,
        delivery_target: None,
        delivery_state: ChildDeliveryState::NotRequired,
        attempt: 1,
        retry_of: None,
        applied_events: BTreeMap::new(),
    }
}

/// A journal + declared isolated child + open transaction over `plan`.
/// Returns the temp dir (kept alive), journal path, journal handle, and store.
fn open_fixture(
    plan: ChildGatePlan,
) -> (
    tempfile::TempDir,
    PathBuf,
    SessionJournal,
    DurableChildStore,
    ChildTransactionStore,
    wcore_agent::child_transaction::ChildTransactionAuthority,
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    children.declare(child_record()).unwrap();
    let store = ChildTransactionStore::new(journal.clone());
    let authority = store
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            plan,
        )
        .unwrap();
    (temp, path, journal, children, store, authority)
}

/// A valid single-gate acceptance receipt for the one-gate plan. Corruptors
/// mutate one field to prove each hostile shape fails closed in isolation.
fn valid_receipt() -> ChildTransactionReceipt {
    let gate = ChildGateReceipt {
        gate_id: "cargo-test".into(),
        subject: ChildGateSubject {
            base_revision: revision('1'),
            candidate_revision: revision('2'),
            diff_digest: digest('d'),
            request_digest: digest('a'),
            policy_digest: digest('b'),
            gate_plan_digest: one_gate_plan().canonical_digest().unwrap(),
            gate_closure_digest: digest('c'),
        },
        evidence_digest: digest('f'),
        outcome: ChildGateOutcome::Passed,
        exit_code: Some(0),
    };
    ChildTransactionReceipt {
        schema_version: CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION,
        transaction_id: "transaction-1".into(),
        receipt_id: "transaction-1-accept-0".into(),
        receipt_revision: 0,
        previous_receipt_digest: None,
        child_id: ChildId::new("child-1").unwrap(),
        child_declaration_id: "declare-child-1".into(),
        child_revision: 0,
        workspace_id: "workspace-child-1".into(),
        base_revision: revision('1'),
        candidate_revision: Some(revision('2')),
        request_digest: digest('a'),
        policy_digest: digest('b'),
        gate_plan_digest: one_gate_plan().canonical_digest().unwrap(),
        diff_digest: Some(digest('d')),
        gates: vec![gate],
        disposition: ChildTransactionDisposition::Active,
        created_at_unix_ms: 100,
        updated_at_unix_ms: 100,
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo(repo: &Path) {
    run_git(repo, &["init"]);
    run_git(repo, &["config", "user.email", "wayland@example.invalid"]);
    run_git(repo, &["config", "user.name", "Wayland Test"]);
    std::fs::write(repo.join("README.md"), "candidate fixture\n").unwrap();
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "-m", "fixture"]);
}

async fn isolated_checkout(
    repo: &Path,
    state: &Path,
    child: &str,
) -> (MutationAttemptGuard, CandidateSeal, PathBuf) {
    init_repo(repo);
    let checkouts = state.join("checkouts");
    std::fs::create_dir_all(&checkouts).unwrap();
    let manager =
        WorktreeManager::new_with_workspace_root(repo, &checkouts).expect("worktree manager");
    let pinned_head = manager.pinned_head().await.expect("pinned head");
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let workspace = manager
        .create_isolated_checkout(
            child,
            &format!("wayland-child/{child}"),
            &pinned_head,
            capacity,
        )
        .await
        .expect("isolated checkout");
    let checkout_root = workspace.checkout_authority().display_path().to_path_buf();
    let seal = workspace.seal_candidate().expect("seal candidate");
    let guard = MutationAttemptGuard::new(workspace);
    (guard, seal, checkout_root)
}

fn subject_for(plan_digest: String) -> GateExecutionSubject {
    GateExecutionSubject {
        base_revision: revision('1'),
        candidate_revision: revision('2'),
        diff_digest: digest('d'),
        request_digest: digest('a'),
        policy_digest: digest('b'),
        gate_plan_digest: plan_digest,
    }
}

/// `run_gate_acceptance` refuses when the transaction is not durably open: an
/// authority minted against one store cannot mint acceptance against another
/// store that never opened it. No candidate code runs.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout lifecycle is exercised on the Linux harness"
)]
async fn missing_durable_transaction_fails_closed() {
    let plan = ChildGatePlan {
        required_gates: Vec::new(),
    };
    // Authority minted against store A (which durably opened the transaction).
    let (_temp_a, _path_a, _journal_a, _children_a, _store_a, authority) =
        open_fixture(plan.clone());

    // A fresh store B that never opened this transaction.
    let temp_b = tempfile::tempdir().unwrap();
    let journal_b = SessionJournal::open(temp_b.path().join("b.journal"), "session-1").unwrap();
    let store_b = ChildTransactionStore::new(journal_b);

    let repo = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (guard, seal, checkout_root) =
        isolated_checkout(repo.path(), state.path(), "missing-child").await;
    let sandbox = SandboxRegistry::new(Arc::new(FailClosedBackend::new()));

    let error = run_gate_acceptance(
        &sandbox,
        &store_b,
        &authority,
        &subject_for(plan.canonical_digest().unwrap()),
        Vec::new(),
        guard,
        seal,
        1_000,
    )
    .await
    .expect_err("acceptance against a store without the durable open must fail closed");
    assert!(
        matches!(error, MutationAcceptanceError::MissingTransaction),
        "expected MissingTransaction, got {error:?}"
    );
    // The consumed guard terminalized its checkout on the failing path.
    assert!(
        !checkout_root.exists(),
        "the failing acceptance path must still clean the checkout"
    );
}

/// `run_gate_acceptance` refuses a subject that does not bind the durable,
/// orchestrator-owned gate plan digest — a substituted plan cannot be accepted.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout lifecycle is exercised on the Linux harness"
)]
async fn subject_not_binding_durable_plan_fails_closed() {
    let plan = ChildGatePlan {
        required_gates: Vec::new(),
    };
    let (_temp, _path, _journal, _children, store, authority) = open_fixture(plan);

    let repo = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (guard, seal, _checkout_root) =
        isolated_checkout(repo.path(), state.path(), "subject-child").await;
    let sandbox = SandboxRegistry::new(Arc::new(FailClosedBackend::new()));

    // A subject that binds a DIFFERENT plan digest than the durable opening.
    let error = run_gate_acceptance(
        &sandbox,
        &store,
        &authority,
        &subject_for(digest('e')),
        Vec::new(),
        guard,
        seal,
        1_000,
    )
    .await
    .expect_err("a subject that does not bind the durable plan must fail closed");
    assert!(
        matches!(error, MutationAcceptanceError::SubjectPlanMismatch),
        "expected SubjectPlanMismatch, got {error:?}"
    );
}

/// Every hostile gate-receipt shape fails closed at the authoritative durable
/// boundary: the typed `validate_for_child` refusal AND a `commit` that leaves
/// no durable effect. The valid base receipt validates, so each failure is
/// attributable to the single injected corruption.
#[test]
fn hostile_gate_evidence_fails_closed_at_durable_boundary() {
    let (_temp, path, _journal, _children, store, authority) = open_fixture(one_gate_plan());
    let plan = one_gate_plan();
    let child = child_record();

    // Sanity: the un-corrupted base validates against the child + plan.
    valid_receipt()
        .validate_for_child(&child, &plan)
        .expect("the un-corrupted base receipt must validate");

    // (substituted) The gate's sealed closure digest is not the plan's.
    let mut substituted = valid_receipt();
    substituted.gates[0].subject.gate_closure_digest = digest('9');
    assert!(matches!(
        substituted.validate_for_child(&child, &plan),
        Err(ChildTransactionValidationError::GatePlanMismatch)
    ));

    // (unknown) The gate is not a member of the orchestrator-owned plan.
    let mut unknown = valid_receipt();
    unknown.gates[0].gate_id = "unknown-gate".into();
    assert!(matches!(
        unknown.validate_for_child(&child, &plan),
        Err(ChildTransactionValidationError::GatePlanMismatch)
    ));

    // (subject drift) The gate's subject does not bind the receipt's subject.
    let mut drifted = valid_receipt();
    drifted.gates[0].subject.base_revision = revision('9');
    assert!(matches!(
        drifted.validate_for_child(&child, &plan),
        Err(ChildTransactionValidationError::GateSubjectMismatch)
    ));

    // (model-claimed) A "passed" gate whose exit code contradicts the outcome.
    let mut claimed = valid_receipt();
    claimed.gates[0].exit_code = Some(1);
    assert!(matches!(
        claimed.validate_for_child(&child, &plan),
        Err(ChildTransactionValidationError::InvalidField(_))
    ));

    // (malformed) A non-digest evidence value.
    let mut malformed = valid_receipt();
    malformed.gates[0].evidence_digest = "not-a-digest".into();
    assert!(matches!(
        malformed.validate_for_child(&child, &plan),
        Err(ChildTransactionValidationError::InvalidDigest(_))
    ));

    // Each hostile receipt is refused by commit with NO durable effect: the
    // durable-receipt boundary reduces through the same validation.
    let before = std::fs::metadata(&path).unwrap().len();
    for hostile in [substituted, unknown, drifted, claimed, malformed] {
        assert!(
            store.commit(&authority, hostile).is_err(),
            "a hostile receipt must be refused by commit"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            before,
            "a refused commit must not append durable bytes"
        );
    }
}

/// A committed receipt is idempotent on exact retry, and a divergent second
/// receipt at the same revision (out-of-order / conflicting) fails closed with
/// no durable effect.
#[test]
fn conflicting_and_duplicate_receipts_fail_closed() {
    let (_temp, path, _journal, _children, store, authority) = open_fixture(one_gate_plan());

    // The genuine acceptance receipt commits once.
    assert!(matches!(
        store.commit(&authority, valid_receipt()).unwrap(),
        ChildTransactionWrite::Appended(_)
    ));
    // Exact retry is idempotent — no second append.
    assert_eq!(
        store.commit(&authority, valid_receipt()).unwrap(),
        ChildTransactionWrite::AlreadyCommitted
    );

    let after_commit = std::fs::metadata(&path).unwrap().len();
    // A divergent second genesis (revision 0, no predecessor, different bytes)
    // is an out-of-order conflict and must fail closed.
    let mut conflict = valid_receipt();
    conflict.receipt_id = "transaction-1-conflict".into();
    conflict.updated_at_unix_ms = 200;
    assert!(
        store.commit(&authority, conflict).is_err(),
        "a conflicting second genesis receipt must fail closed"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        after_commit,
        "a refused conflicting commit must not append durable bytes"
    );
}

/// A corrupted durable journal fails closed on reopen/reduce — acceptance can
/// never rest on tampered durable evidence.
#[test]
fn durable_journal_corruption_fails_closed() {
    let (temp, path, journal, children, store, authority) = open_fixture(one_gate_plan());
    store.commit(&authority, valid_receipt()).unwrap();
    drop(store);
    drop(children);
    drop(journal);

    let original = std::fs::read(&path).unwrap();
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 0xff;
    let corrupt_path = temp.path().join("corrupt.journal");
    std::fs::write(&corrupt_path, corrupt).unwrap();
    assert!(
        SessionJournal::recovered_state(&corrupt_path).is_err(),
        "a corrupted durable journal must fail closed on reopen"
    );
}

/// A retained opening authority cannot be rebound to a copied journal — a stale
/// authority against duplicated storage fails closed.
#[test]
fn copied_journal_cannot_rebind_authority() {
    let (temp, path, journal, children, store, authority) = open_fixture(one_gate_plan());
    drop(store);
    drop(children);
    drop(journal);

    let rebound_path = temp.path().join("rebound.journal");
    std::fs::copy(&path, &rebound_path).unwrap();
    let rebound = SessionJournal::open(&rebound_path, "session-1").unwrap();
    assert!(
        ChildTransactionStore::new(rebound)
            .revalidate(&authority)
            .is_err(),
        "a stale authority against a copied journal must fail closed"
    );
}

/// Abnormal termination (cancellation, timeout, process death, dropped guard,
/// restart) all funnel through RAII terminalization: dropping the still-armed
/// guard cleans the isolated checkout exactly once and mutates no durable /
/// parent state. Reopening the durable journal shows the transaction unchanged
/// (`Active`, no receipts) — the guard cleanup never landed anything.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout lifecycle is exercised on the Linux harness"
)]
async fn dropped_guard_terminalizes_and_cleans_without_parent_mutation() {
    let (_temp, path, journal, children, store, _authority) = open_fixture(one_gate_plan());

    let repo = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (guard, seal, checkout_root) =
        isolated_checkout(repo.path(), state.path(), "drop-child").await;
    assert!(checkout_root.is_dir(), "checkout must exist while armed");

    // Dropping the seal then the guard terminalizes the checkout exactly once.
    drop(seal);
    drop(guard);
    assert!(
        !checkout_root.exists(),
        "dropping the guard must terminalize and remove the checkout"
    );

    // No durable / parent mutation occurred: the transaction is still Active
    // with zero receipts (the guard cleanup is purely local to the checkout).
    let durable = store.inspect("transaction-1").unwrap().unwrap();
    assert!(
        durable.receipts.is_empty(),
        "guard termination must not append any receipt"
    );

    // Restart: reopen the durable journal from disk and confirm the same — the
    // transaction survives with no landing and the checkout stays gone.
    drop(store);
    drop(children);
    drop(journal);
    let reopened = SessionJournal::open(&path, "session-1").unwrap();
    let restarted = ChildTransactionStore::new(reopened)
        .inspect("transaction-1")
        .unwrap()
        .expect("the durable transaction survives a restart");
    assert!(restarted.receipts.is_empty(), "restart shows no landing");
    assert!(
        !checkout_root.exists(),
        "the terminalized checkout stays gone after restart"
    );
}
