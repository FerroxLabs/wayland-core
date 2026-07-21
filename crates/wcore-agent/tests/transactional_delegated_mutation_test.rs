//! Hostile end-to-end proof of the composed child-transaction lifecycle.
//!
//! Every case here drives a delegated mutation ONLY through the composed
//! [`ChildTransactionLifecycle`] surface (`open` → retained isolated child
//! checkout → `accept_selected_winner` → `land` → `rollback` →
//! `terminal_receipt`) against REAL, independent Git repositories, and asserts
//! the real Git state (target ref, HEAD, worktree bytes) together with the
//! durable journal state (landing lifecycle, receipts, checkout identity) at
//! every boundary — never mocks.
//!
//! The integration checkout is a clean Wayland-owned Git clone the parent
//! primitive accepts; the child mutates only its own isolated retained checkout;
//! only a durably gate-accepted winner ever reaches the parent compare-and-swap.
//!
//! The live-git cases are Linux-gated: allocating an isolated checkout and
//! projecting a coherent parent landing exercises the swarm's worktree machinery
//! on the harness platform, exactly as the sibling
//! `child_transaction_parent_cas_test.rs` gates them.
//!
//! Scope / construction notes (surfaced honestly for the 20-16 review):
//! * Acceptance is built through the real `run_gate_acceptance` path the
//!   lifecycle owns. A *gate-less* plan is used for the accepted (landing) cases
//!   — the established, proven pattern in the sibling landing test — because the
//!   subject under proof here is the parent landing / rollback / restart /
//!   winner-selection lifecycle, not the hard-containment backend. The rejection
//!   case (`gate_rejection_never_reaches_landing`) uses a REQUIRED gate to prove
//!   a failing gate blocks landing.
//! * The retained isolated checkout the child lands from is allocated through
//!   the same `WorktreeManager` seam the production
//!   `AgentSpawner::spawn_builder_into_retained_checkout` hands onward; a full
//!   `AgentSpawner`/provider engine is intentionally not stood up in this
//!   black-box test.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wcore_agent::child_transaction::{
    ChildTransactionLifecycle, ChildTransactionStore, GateExecutionSubject,
    MutationAcceptanceError, MutationAttemptGuard, ParentLandingAuthorization,
    ParentLandingAuthorizationError,
};
use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::orchestration::anvil::engine::{CandidateCheckout, EngineError};
use wcore_agent::orchestration::anvil::landing::{
    WinnerLandingError, WinnerLandingRequest, land_selected_winner,
};
use wcore_agent::session_journal::{LandingState, SessionJournal};
use wcore_sandbox::{FailClosedBackend, SandboxRegistry};
use wcore_swarm::worktree::{CandidateSeal, WorkspaceCapacity, WorktreeManager};
use wcore_types::child_transaction::{ChildGatePlan, ChildGateRequirement};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus,
};

// ---------------------------------------------------------------------------
// Fixture helpers (real git + real journal).
// ---------------------------------------------------------------------------

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn revision(character: char) -> String {
    std::iter::repeat_n(character, 40).collect()
}

fn child_record(child_id: &str, declaration_id: &str, workspace_id: &str) -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: declaration_id.into(),
        child_id: ChildId::new(child_id).unwrap(),
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
            workspace_id: workspace_id.into(),
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

fn git_stdout(repo: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn init_repo(repo: &Path) {
    run_git(repo, &["init", "-b", "main"]);
    run_git(repo, &["config", "user.email", "wayland@example.invalid"]);
    run_git(repo, &["config", "user.name", "Wayland Test"]);
    std::fs::write(repo.join("README.md"), "base\n").unwrap();
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "-m", "base"]);
}

/// Build a clean, Wayland-owned integration checkout: a real Git clone of the
/// source the parent-landing primitive accepts (a bare/dirty repo would be
/// rejected by design).
fn clone_integration(source: &Path, destination: &Path) {
    run_git(
        source,
        &[
            "clone",
            "--",
            &source.to_string_lossy(),
            &destination.to_string_lossy(),
        ],
    );
    run_git(
        destination,
        &["config", "user.email", "wayland@example.invalid"],
    );
    run_git(destination, &["config", "user.name", "Wayland Test"]);
}

fn open_manager(source: &Path, state: &Path) -> WorktreeManager {
    let checkouts = state.join("checkouts");
    std::fs::create_dir_all(&checkouts).unwrap();
    WorktreeManager::new_with_workspace_root(source, &checkouts).expect("worktree manager")
}

/// Every temp dir + live journal handle a case needs kept alive. The
/// `WorktreeManager` is returned separately because the lifecycle consumes it.
struct Fixture {
    _journal_dir: tempfile::TempDir,
    _source: tempfile::TempDir,
    _integration: tempfile::TempDir,
    _state: tempfile::TempDir,
    journal_path: PathBuf,
    journal: SessionJournal,
    children: DurableChildStore,
    store: ChildTransactionStore,
    integration_path: PathBuf,
}

fn setup() -> (Fixture, WorktreeManager) {
    let journal_dir = tempfile::tempdir().unwrap();
    let journal_path = journal_dir.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    let store = ChildTransactionStore::new(journal.clone());

    let source = tempfile::tempdir().unwrap();
    init_repo(source.path());
    let integration = tempfile::tempdir().unwrap();
    let integration_path = integration.path().join("checkout");
    clone_integration(source.path(), &integration_path);
    let integration_path = std::fs::canonicalize(&integration_path).unwrap();

    let state = tempfile::tempdir().unwrap();
    let manager = open_manager(source.path(), state.path());

    (
        Fixture {
            _journal_dir: journal_dir,
            _source: source,
            _integration: integration,
            _state: state,
            journal_path,
            journal,
            children,
            store,
            integration_path,
        },
        manager,
    )
}

fn empty_plan() -> ChildGatePlan {
    ChildGatePlan {
        required_gates: Vec::new(),
    }
}

fn one_gate_plan() -> ChildGatePlan {
    ChildGatePlan {
        required_gates: vec![ChildGateRequirement {
            gate_id: "cargo-test".into(),
            gate_closure_digest: digest('c'),
        }],
    }
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

fn sandbox() -> SandboxRegistry {
    SandboxRegistry::new(Arc::new(FailClosedBackend::new()))
}

/// Allocate one retained isolated checkout, write the candidate mutation into it,
/// and seal it — the same `TransactionWorkspace` the production
/// `spawn_builder_into_retained_checkout` seam retains. Returns the still-armed
/// guard, the seal, and the checkout root on disk.
async fn stage_candidate(
    manager: &WorktreeManager,
    capacity: WorkspaceCapacity,
    child_id: &str,
    added_file: &str,
    added_body: &str,
) -> (MutationAttemptGuard, CandidateSeal, PathBuf) {
    let pinned_head = manager.pinned_head().await.expect("pinned head");
    let workspace = manager
        .create_isolated_checkout(
            child_id,
            &format!("wayland-child/{child_id}"),
            &pinned_head,
            capacity,
        )
        .await
        .expect("isolated checkout");
    let checkout_root = workspace.checkout_authority().display_path().to_path_buf();
    // Mutate the working tree BEFORE sealing so the seal binds the mutation.
    std::fs::write(checkout_root.join(added_file), added_body).unwrap();
    let seal = workspace.seal_candidate().expect("seal candidate");
    let guard = MutationAttemptGuard::new(workspace);
    (guard, seal, checkout_root)
}

// ---------------------------------------------------------------------------
// Case 1 — Happy path: open → retained child → accept winner → land → receipt,
// then rollback exactly reverses it.
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + coherent landing is exercised on the Linux harness"
)]
async fn happy_path_open_accept_land_receipt_then_rollback() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("child-1", "declare-1", "workspace-1"))
        .unwrap();
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let (guard, seal, checkout_root) = stage_candidate(
        &manager,
        capacity,
        "child-1",
        "added.txt",
        "landed change\n",
    )
    .await;

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);

    // open binds the authoritative snapshot and revalidates BEFORE any effect.
    let plan = empty_plan();
    let authority = lifecycle
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("durable open");

    // Only the selected winner traverses parent-owned gate acceptance.
    let accepted = lifecycle
        .accept_selected_winner(
            &authority,
            &subject_for(plan.canonical_digest().unwrap()),
            Vec::new(),
            guard,
            seal,
            1_000,
        )
        .await
        .expect("winner accepted");

    let base = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);
    let outcome = lifecycle
        .land(
            &authority,
            &accepted,
            &fx.integration_path,
            "refs/heads/main",
        )
        .await
        .expect("authorized landing");

    let rollback = match outcome {
        ParentLandingAuthorization::Landed {
            successor,
            rollback,
        } => {
            // Real git state: the mutation landed and the target ref advanced.
            assert!(fx.integration_path.join("added.txt").is_file());
            let head = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);
            assert_eq!(head, successor.landed_commit, "HEAD must equal successor");
            assert_ne!(head, base, "target ref must advance");
            rollback
        }
        other => panic!("expected Landed, got {other:?}"),
    };

    // Durable state: terminal receipt is Landed with the same successor and the
    // checkout the winner landed from is still owned (checkout on disk).
    let durable = lifecycle
        .terminal_receipt("transaction-1")
        .unwrap()
        .unwrap();
    match durable.landing_state() {
        Some(LandingState::Landed { successor }) => assert_eq!(
            successor.landed_commit,
            git_stdout(&fx.integration_path, &["rev-parse", "HEAD"])
        ),
        other => panic!("expected durable Landed, got {other:?}"),
    }
    assert!(
        !durable.receipts.is_empty(),
        "acceptance receipt is durable"
    );
    assert!(checkout_root.is_dir(), "the winner checkout is still owned");

    // rollback exactly reverses ref/HEAD/worktree and records RolledBack.
    let rolled = lifecycle
        .rollback(&authority, &fx.integration_path, &rollback)
        .await
        .expect("authorized rollback");
    assert!(matches!(
        rolled,
        ParentLandingAuthorization::RolledBack { .. }
    ));
    assert!(!fx.integration_path.join("added.txt").exists());
    assert_eq!(
        git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]),
        base
    );
    let durable = lifecycle
        .terminal_receipt("transaction-1")
        .unwrap()
        .unwrap();
    assert!(matches!(
        durable.landing_state(),
        Some(LandingState::RolledBack { .. })
    ));
}

// ---------------------------------------------------------------------------
// Case 2 — Parent drift before land → CAS conflict, no overwrite, foreign work
// survives, durable state never reaches Landed.
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + coherent landing is exercised on the Linux harness"
)]
async fn parent_drift_before_land_conflicts_without_overwrite() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("child-1", "declare-1", "workspace-1"))
        .unwrap();
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let (guard, seal, _root) =
        stage_candidate(&manager, capacity, "child-1", "added.txt", "candidate\n").await;

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);
    let plan = empty_plan();
    let authority = lifecycle
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("durable open");
    let accepted = lifecycle
        .accept_selected_winner(
            &authority,
            &subject_for(plan.canonical_digest().unwrap()),
            Vec::new(),
            guard,
            seal,
            1_000,
        )
        .await
        .expect("winner accepted");

    // A foreign commit advances the target ref out-of-band past the candidate
    // base BEFORE the landing runs.
    std::fs::write(fx.integration_path.join("foreign.txt"), "foreign\n").unwrap();
    run_git(&fx.integration_path, &["add", "foreign.txt"]);
    run_git(&fx.integration_path, &["commit", "-m", "foreign"]);
    let foreign_head = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);

    // The landing must fail closed (Conflict / non-landing outcome or Err) and
    // must NOT overwrite the foreign commit.
    let outcome = lifecycle
        .land(
            &authority,
            &accepted,
            &fx.integration_path,
            "refs/heads/main",
        )
        .await;
    match outcome {
        Ok(ParentLandingAuthorization::Landed { .. }) => {
            panic!("a drifted parent must never be overwritten by a stale-base landing")
        }
        Ok(ParentLandingAuthorization::Conflict { .. })
        | Ok(ParentLandingAuthorization::RecoveryRequired { .. })
        | Ok(ParentLandingAuthorization::Incomplete { .. })
        | Err(_) => {}
        Ok(ParentLandingAuthorization::RolledBack { .. }) => {
            panic!("landing cannot report RolledBack")
        }
    }

    // Real git state: the foreign commit survives untouched.
    assert_eq!(
        git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]),
        foreign_head,
        "the foreign commit must survive a conflicting landing"
    );
    assert!(fx.integration_path.join("foreign.txt").is_file());
    assert!(
        !fx.integration_path.join("added.txt").exists(),
        "the stale candidate must not have landed"
    );
    // Durable state: never Landed.
    let durable = lifecycle
        .terminal_receipt("transaction-1")
        .unwrap()
        .unwrap();
    assert!(
        !matches!(durable.landing_state(), Some(LandingState::Landed { .. })),
        "a conflicting landing must never durably record Landed, got {:?}",
        durable.landing_state()
    );
}

// ---------------------------------------------------------------------------
// Case 3 — Gate rejection → the candidate never reaches landing; no parent
// mutation.
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout lifecycle is exercised on the Linux harness"
)]
async fn gate_rejection_never_reaches_landing() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("child-1", "declare-1", "workspace-1"))
        .unwrap();
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let (guard, seal, checkout_root) =
        stage_candidate(&manager, capacity, "child-1", "added.txt", "candidate\n").await;

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);

    // A plan with a REQUIRED gate, but NO authorized closure for it: the gate
    // executor fails closed at closure resolution before any candidate code
    // runs, so acceptance is refused and no AcceptedCandidate is ever produced.
    let plan = one_gate_plan();
    let authority = lifecycle
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("durable open");
    let base_head = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);

    let error = lifecycle
        .accept_selected_winner(
            &authority,
            &subject_for(plan.canonical_digest().unwrap()),
            Vec::new(),
            guard,
            seal,
            1_000,
        )
        .await
        .expect_err("a required gate with no authorized closure must fail closed");
    assert!(
        matches!(error, MutationAcceptanceError::GateStage(_)),
        "gate rejection must be a fail-closed gate-stage refusal, got {error:?}"
    );

    // No AcceptedCandidate exists, so the parent CAS can never be reached. The
    // consumed guard terminalized its checkout; the integration ref is untouched
    // and the durable transaction holds no acceptance receipt or landing.
    assert!(
        !checkout_root.exists(),
        "the rejected candidate checkout must be terminalized"
    );
    assert_eq!(
        git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]),
        base_head,
        "a rejected gate must not mutate the parent"
    );
    let durable = lifecycle
        .terminal_receipt("transaction-1")
        .unwrap()
        .unwrap();
    assert!(
        durable.receipts.is_empty(),
        "no acceptance receipt is durable"
    );
    assert!(
        durable.landing_state().is_none(),
        "a rejected candidate never enters the landing lifecycle"
    );
}

// ---------------------------------------------------------------------------
// Case 4 — Landing without an accepted candidate for THIS transaction fails
// closed; the parent ref is unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout lifecycle is exercised on the Linux harness"
)]
async fn landing_without_bound_accepted_candidate_fails_closed() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("child-a", "declare-a", "workspace-a"))
        .unwrap();
    fx.children
        .declare(child_record("child-b", "declare-b", "workspace-b"))
        .unwrap();
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let (guard_a, seal_a, _root_a) =
        stage_candidate(&manager, capacity, "child-a", "a.txt", "A\n").await;

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);
    let plan = empty_plan();

    // Accept a candidate for transaction A.
    let authority_a = lifecycle
        .open(
            "transaction-a",
            ChildId::new("child-a").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("open A");
    let accepted_a = lifecycle
        .accept_selected_winner(
            &authority_a,
            &subject_for(plan.canonical_digest().unwrap()),
            Vec::new(),
            guard_a,
            seal_a,
            1_000,
        )
        .await
        .expect("A accepted");

    // Open a DIFFERENT transaction B that has no accepted candidate, and try to
    // land it using A's candidate. The authorization layer must refuse because
    // the accepted candidate does not bind transaction B.
    let authority_b = lifecycle
        .open(
            "transaction-b",
            ChildId::new("child-b").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("open B");
    let base_head = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);

    let error = lifecycle
        .land(
            &authority_b,
            &accepted_a,
            &fx.integration_path,
            "refs/heads/main",
        )
        .await
        .expect_err("landing an unbound candidate must fail closed");
    assert!(
        matches!(error, ParentLandingAuthorizationError::CandidateMismatch),
        "expected CandidateMismatch, got {error:?}"
    );

    // The parent ref is unchanged and B never entered the landing lifecycle.
    assert_eq!(
        git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]),
        base_head
    );
    let durable_b = lifecycle
        .terminal_receipt("transaction-b")
        .unwrap()
        .unwrap();
    assert!(durable_b.landing_state().is_none());

    // A stayed owned through the assertions above; terminalize it now.
    drop(accepted_a);
}

// ---------------------------------------------------------------------------
// Case 5 — Cancellation/interruption mid-lifecycle → owned cleanup only,
// attributable durable state, no parent mutation.
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout lifecycle is exercised on the Linux harness"
)]
async fn cancellation_after_accept_before_land_cleans_owned_only() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("child-1", "declare-1", "workspace-1"))
        .unwrap();
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let (guard, seal, checkout_root) =
        stage_candidate(&manager, capacity, "child-1", "added.txt", "candidate\n").await;

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);
    let plan = empty_plan();
    let authority = lifecycle
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("durable open");
    let accepted = lifecycle
        .accept_selected_winner(
            &authority,
            &subject_for(plan.canonical_digest().unwrap()),
            Vec::new(),
            guard,
            seal,
            1_000,
        )
        .await
        .expect("winner accepted");

    assert!(checkout_root.is_dir(), "checkout is owned while accepted");
    let base_head = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);

    // Interruption before landing: dropping the accepted candidate terminalizes
    // ONLY its owned checkout via RAII and mutates no parent / landing state.
    drop(accepted);
    assert!(
        !checkout_root.exists(),
        "cancellation must terminalize only the owned checkout"
    );

    // Attributable durable state: the acceptance receipt survives (the winner
    // WAS accepted) but the landing lifecycle was never entered, and the parent
    // ref is untouched.
    let durable = lifecycle
        .terminal_receipt("transaction-1")
        .unwrap()
        .unwrap();
    assert!(
        !durable.receipts.is_empty(),
        "the acceptance receipt is durable and attributable"
    );
    assert!(
        durable.landing_state().is_none(),
        "an interrupted transaction never landed"
    );
    assert_eq!(
        git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]),
        base_head,
        "cancellation must not mutate the parent"
    );
}

// ---------------------------------------------------------------------------
// Case 6 — Restart recovery: the landed state replays deterministically from
// disk after every live handle is dropped.
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + coherent landing is exercised on the Linux harness"
)]
async fn restart_replays_landed_state_from_disk() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("child-1", "declare-1", "workspace-1"))
        .unwrap();
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let (guard, seal, _root) =
        stage_candidate(&manager, capacity, "child-1", "added.txt", "landed\n").await;

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);
    let plan = empty_plan();
    let authority = lifecycle
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("durable open");
    let accepted = lifecycle
        .accept_selected_winner(
            &authority,
            &subject_for(plan.canonical_digest().unwrap()),
            Vec::new(),
            guard,
            seal,
            1_000,
        )
        .await
        .expect("winner accepted");
    let outcome = lifecycle
        .land(
            &authority,
            &accepted,
            &fx.integration_path,
            "refs/heads/main",
        )
        .await
        .expect("authorized landing");
    assert!(matches!(outcome, ParentLandingAuthorization::Landed { .. }));

    // Drop every live journal handle to simulate a process restart, while
    // keeping the temp dirs (which own the journal file on disk) alive.
    let Fixture {
        _journal_dir,
        _source,
        _integration,
        _state,
        journal_path,
        journal,
        children,
        store,
        integration_path: _,
    } = fx;
    drop(lifecycle);
    drop(accepted);
    drop(store);
    drop(children);
    drop(journal);

    // Recovery derives the landing state SOLELY from deterministic journal
    // replay off disk — no writer lease, exactly the restart-reconciliation path.
    let replayed = SessionJournal::recovered_state(&journal_path).unwrap();
    let transaction = replayed
        .child_transactions
        .get("transaction-1")
        .expect("the transaction must replay from disk");
    assert!(
        matches!(
            transaction.landing_state(),
            Some(LandingState::Landed { .. })
        ),
        "landed state must be derived solely from journal replay, got {:?}",
        transaction.landing_state()
    );
}

// ---------------------------------------------------------------------------
// Case 7 — Multi-candidate keep-best: only the selected winner lands; the loser
// terminalizes and is cleaned, never reaching the parent CAS.
// ---------------------------------------------------------------------------

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + coherent landing is exercised on the Linux harness"
)]
async fn multi_candidate_only_winner_lands_loser_is_cleaned() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("winner", "declare-w", "workspace-w"))
        .unwrap();
    fx.children
        .declare(child_record("loser", "declare-l", "workspace-l"))
        .unwrap();
    let capacity = manager.workspace_capacity(2).await.expect("capacity");

    // Two candidate checkouts are allocated (a keep-best climb produced both).
    let (winner_guard, winner_seal, winner_root) =
        stage_candidate(&manager, capacity, "winner", "winner.txt", "winner\n").await;
    let (loser_guard, loser_seal, loser_root) =
        stage_candidate(&manager, capacity, "loser", "loser.txt", "loser\n").await;
    assert!(winner_root.is_dir() && loser_root.is_dir());

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);
    let plan = empty_plan();

    // Only the selected winner traverses gate acceptance + landing.
    let winner_authority = lifecycle
        .open(
            "transaction-winner",
            ChildId::new("winner").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .expect("open winner");
    let accepted_winner = lifecycle
        .accept_selected_winner(
            &winner_authority,
            &subject_for(plan.canonical_digest().unwrap()),
            Vec::new(),
            winner_guard,
            winner_seal,
            1_000,
        )
        .await
        .expect("winner accepted");

    // The loser is NOT accepted; its guard/seal terminalize by RAII (the exact
    // cleanup a keep-best climb performs on every non-selected candidate).
    drop(loser_seal);
    drop(loser_guard);
    assert!(
        !loser_root.exists(),
        "the loser checkout must be terminalized and cleaned"
    );

    let base = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);
    let outcome = lifecycle
        .land(
            &winner_authority,
            &accepted_winner,
            &fx.integration_path,
            "refs/heads/main",
        )
        .await
        .expect("winner landing");
    assert!(matches!(outcome, ParentLandingAuthorization::Landed { .. }));

    // Real git state: exactly the winner's mutation landed; the loser's never
    // reached the parent (no double mutation).
    assert!(fx.integration_path.join("winner.txt").is_file());
    assert!(
        !fx.integration_path.join("loser.txt").exists(),
        "the loser must never reach the parent"
    );
    assert_ne!(
        git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]),
        base,
        "the winner advanced the ref exactly once"
    );

    // Durable state: only the winner has a landing; the loser never opened one.
    let winner_durable = lifecycle
        .terminal_receipt("transaction-winner")
        .unwrap()
        .unwrap();
    assert!(matches!(
        winner_durable.landing_state(),
        Some(LandingState::Landed { .. })
    ));
    assert!(
        lifecycle
            .terminal_receipt("transaction-loser")
            .unwrap()
            .is_none(),
        "the loser never opened a durable transaction, let alone landed"
    );

    drop(accepted_winner);
}

// ---------------------------------------------------------------------------
// Case — the PRODUCTION landing orchestrator (`land_selected_winner`) drives the
// exact terminal chain the inline cases above prove, through the single
// production entry point the Anvil forge wiring will call. A test-local
// `CandidateCheckout` double surrenders the real staged winner's `(guard, seal)`
// via `into_landing_authority`, mirroring the production `RetainedCheckout`.
// ---------------------------------------------------------------------------

/// A `CandidateCheckout` that yields a pre-staged real `(guard, seal)` exactly
/// once — the test-side analogue of the production `RetainedCheckout`. `None`
/// authority models a released/drifted/substituted winner.
struct StagedWinner {
    authority: Option<(MutationAttemptGuard, CandidateSeal)>,
    root: PathBuf,
}

impl std::fmt::Debug for StagedWinner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StagedWinner")
            .field("has_authority", &self.authority.is_some())
            .field("root", &self.root)
            .finish()
    }
}

impl CandidateCheckout for StagedWinner {
    fn resolve_root(&self) -> Result<PathBuf, EngineError> {
        Ok(self.root.clone())
    }

    fn into_landing_authority(self: Box<Self>) -> Option<(MutationAttemptGuard, CandidateSeal)> {
        self.authority
    }
}

#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + coherent landing is exercised on the Linux harness"
)]
async fn land_selected_winner_drives_production_chain_to_landed() {
    let (fx, manager) = setup();
    fx.children
        .declare(child_record("child-1", "declare-1", "workspace-1"))
        .unwrap();
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let (guard, seal, checkout_root) = stage_candidate(
        &manager,
        capacity,
        "child-1",
        "added.txt",
        "landed change\n",
    )
    .await;

    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);
    let plan = empty_plan();
    let base = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);

    // The production entry point performs open → accept_selected_winner → land
    // itself; the test supplies only the winner, the lifecycle, and the request.
    let winner: Box<dyn CandidateCheckout> = Box::new(StagedWinner {
        authority: Some((guard, seal)),
        root: checkout_root,
    });
    let request = WinnerLandingRequest {
        transaction_id: "transaction-1".into(),
        child_id: ChildId::new("child-1").unwrap(),
        base_revision: revision('1'),
        gate_plan: plan.clone(),
        subject: subject_for(plan.canonical_digest().unwrap()),
        closures: Vec::new(),
        integration_checkout: fx.integration_path.clone(),
        target_ref: "refs/heads/main".into(),
        now_unix_ms: 1_000,
    };

    let outcome = land_selected_winner(winner, &lifecycle, request)
        .await
        .expect("production landing chain succeeds");

    match outcome {
        ParentLandingAuthorization::Landed { successor, .. } => {
            // Real git state: the winner's mutation landed and the ref advanced.
            assert!(
                fx.integration_path.join("added.txt").is_file(),
                "the winner's mutation landed in the integration checkout"
            );
            let head = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);
            assert_eq!(
                head, successor.landed_commit,
                "HEAD must equal the successor"
            );
            assert_ne!(head, base, "the target ref must have advanced");
        }
        other => panic!("expected Landed, got {other:?}"),
    }

    // Durable state proves the production path went through the real lifecycle.
    let durable = lifecycle
        .terminal_receipt("transaction-1")
        .unwrap()
        .unwrap();
    assert!(matches!(
        durable.landing_state(),
        Some(LandingState::Landed { .. })
    ));
}

#[tokio::test]
async fn land_selected_winner_refuses_winner_without_landing_authority() {
    let (fx, manager) = setup();
    let lifecycle = ChildTransactionLifecycle::new(fx.store.clone(), sandbox(), manager);
    let plan = empty_plan();
    let base = git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]);

    // A winner that surrenders no authority (released/drifted/substituted) must
    // be a HARD fail-closed refusal — never a silent no-op reporting success.
    let winner: Box<dyn CandidateCheckout> = Box::new(StagedWinner {
        authority: None,
        root: fx.integration_path.clone(),
    });
    let request = WinnerLandingRequest {
        transaction_id: "transaction-refused".into(),
        child_id: ChildId::new("child-refused").unwrap(),
        base_revision: revision('1'),
        gate_plan: plan.clone(),
        subject: subject_for(plan.canonical_digest().unwrap()),
        closures: Vec::new(),
        integration_checkout: fx.integration_path.clone(),
        target_ref: "refs/heads/main".into(),
        now_unix_ms: 1_000,
    };

    let err = land_selected_winner(winner, &lifecycle, request)
        .await
        .expect_err("no landing authority must fail closed");
    assert!(matches!(err, WinnerLandingError::NoLandingAuthority));

    // Fail-closed BEFORE any effect: no transaction opened, no ref moved.
    assert!(
        lifecycle
            .terminal_receipt("transaction-refused")
            .unwrap()
            .is_none(),
        "the refusal must precede any durable open"
    );
    assert_eq!(
        git_stdout(&fx.integration_path, &["rev-parse", "HEAD"]),
        base,
        "the integration ref must not have moved"
    );
}
