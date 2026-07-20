//! Hostile parent-landing compare-and-swap tests.
//!
//! These exercise the delegated-mutation landing entirely through the public
//! agent surface (`authorize_and_land` / `authorize_and_rollback`) against real,
//! independent Git repositories: an accepted candidate (built exclusively via
//! `run_gate_acceptance`) is landed into a real integration checkout by exact
//! compare-and-swap, and the durable journal lifecycle plus every fail-closed
//! boundary is asserted. The live-git cases are Linux-gated (the isolated
//! checkout + landing machinery is exercised on the harness); the append-denylist
//! forgery-resistance case runs everywhere.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use wcore_agent::child_transaction::{
    ChildTransactionStore, GateExecutionSubject, MutationAttemptGuard, ParentLandingAuthorization,
    authorize_and_land, authorize_and_rollback, run_gate_acceptance,
};
use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::session_journal::{LandingState, SessionEvent, SessionJournal};
use wcore_sandbox::{FailClosedBackend, SandboxRegistry};
use wcore_swarm::worktree::WorktreeManager;
use wcore_types::child_transaction::ChildGatePlan;
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

/// An accepted candidate whose working tree carries `added` (a new file), built
/// exclusively through `run_gate_acceptance` with a gate-less plan.
async fn accept_candidate(
    store: &ChildTransactionStore,
    manager: &WorktreeManager,
    capacity: wcore_swarm::worktree::WorkspaceCapacity,
    child_id: &str,
    transaction_id: &str,
    added_file: &str,
    added_body: &str,
) -> wcore_agent::child_transaction::AcceptedCandidate {
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
    // Mutate the working tree BEFORE sealing so the seal binds the mutation.
    let checkout_root = workspace.checkout_authority().display_path().to_path_buf();
    std::fs::write(checkout_root.join(added_file), added_body).unwrap();
    let seal = workspace.seal_candidate().expect("seal candidate");
    let guard = MutationAttemptGuard::new(workspace);

    let plan = ChildGatePlan {
        required_gates: Vec::new(),
    };
    let plan_digest = plan.canonical_digest().unwrap();
    let authority = store
        .open(
            transaction_id,
            ChildId::new(child_id).unwrap(),
            revision('1'),
            plan.clone(),
        )
        .unwrap();
    let subject = GateExecutionSubject {
        base_revision: revision('1'),
        candidate_revision: revision('2'),
        diff_digest: digest('d'),
        request_digest: digest('a'),
        policy_digest: digest('b'),
        gate_plan_digest: plan_digest,
    };
    let sandbox = SandboxRegistry::new(Arc::new(FailClosedBackend::new()));
    run_gate_acceptance(
        &sandbox,
        store,
        &authority,
        &subject,
        Vec::new(),
        guard,
        seal,
        1_000,
    )
    .await
    .expect("gate-less acceptance")
}

fn open_manager(source: &Path, state: &Path) -> WorktreeManager {
    let checkouts = state.join("checkouts");
    std::fs::create_dir_all(&checkouts).unwrap();
    WorktreeManager::new_with_workspace_root(source, &checkouts).expect("worktree manager")
}

/// End-to-end: an accepted candidate lands by exact CAS, the durable journal
/// records the full `Prepared → RefAdvanced → Projected → Landed` lifecycle, and
/// rollback exactly reverses it and records `RolledBack`.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + landing is exercised on the Linux harness"
)]
async fn lands_and_journals_full_lifecycle_then_rolls_back() {
    let journal_dir = tempfile::tempdir().unwrap();
    let journal_path = journal_dir.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    children
        .declare(child_record(
            "child-1",
            "declare-child-1",
            "workspace-child-1",
        ))
        .unwrap();
    let store = ChildTransactionStore::new(journal.clone());

    let source = tempfile::tempdir().unwrap();
    init_repo(source.path());
    let integration = tempfile::tempdir().unwrap();
    let integration_path = integration.path().join("checkout");
    clone_integration(source.path(), &integration_path);
    let integration_path = std::fs::canonicalize(&integration_path).unwrap();

    let state = tempfile::tempdir().unwrap();
    let manager = open_manager(source.path(), state.path());
    let capacity = manager.workspace_capacity(1).await.expect("capacity");

    let accepted = accept_candidate(
        &store,
        &manager,
        capacity,
        "child-1",
        "transaction-1",
        "added.txt",
        "landed change\n",
    )
    .await;
    let authority = store
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            ChildGatePlan {
                required_gates: Vec::new(),
            },
        )
        .unwrap();

    let base = git_stdout(&integration_path, &["rev-parse", "HEAD"]);
    let outcome = authorize_and_land(
        &store,
        &authority,
        &accepted,
        &manager,
        &integration_path,
        "refs/heads/main",
    )
    .await
    .expect("authorized landing");
    let rollback = match outcome {
        ParentLandingAuthorization::Landed {
            rollback,
            successor,
        } => {
            assert!(integration_path.join("added.txt").is_file());
            let head = git_stdout(&integration_path, &["rev-parse", "HEAD"]);
            assert_eq!(head, successor.landed_commit);
            assert_ne!(head, base, "ref did not advance");
            rollback
        }
        other => panic!("expected Landed, got {other:?}"),
    };

    // The durable journal records the full landing lifecycle.
    let durable = store.inspect("transaction-1").unwrap().unwrap();
    match durable.landing_state() {
        Some(LandingState::Landed { successor }) => {
            assert_eq!(
                successor.landed_commit,
                git_stdout(&integration_path, &["rev-parse", "HEAD"])
            );
        }
        other => panic!("expected durable Landed, got {other:?}"),
    }

    // Rollback exactly reverses and records RolledBack.
    let rolled = authorize_and_rollback(&store, &authority, &manager, &integration_path, &rollback)
        .await
        .expect("authorized rollback");
    assert!(matches!(
        rolled,
        ParentLandingAuthorization::RolledBack { .. }
    ));
    assert!(!integration_path.join("added.txt").exists());
    assert_eq!(git_stdout(&integration_path, &["rev-parse", "HEAD"]), base);
    let durable = store.inspect("transaction-1").unwrap().unwrap();
    assert!(matches!(
        durable.landing_state(),
        Some(LandingState::RolledBack { .. })
    ));

    manager
        .release_transaction(accepted.guard().workspace())
        .ok();
}

/// Restart recovery: the reduced landing state is derived solely from
/// deterministic journal replay after every live handle is dropped and the
/// journal is reopened from disk.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + landing is exercised on the Linux harness"
)]
async fn restart_recovery_replays_landed_state_from_disk() {
    let journal_dir = tempfile::tempdir().unwrap();
    let journal_path = journal_dir.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    children
        .declare(child_record(
            "child-1",
            "declare-child-1",
            "workspace-child-1",
        ))
        .unwrap();
    let store = ChildTransactionStore::new(journal.clone());

    let source = tempfile::tempdir().unwrap();
    init_repo(source.path());
    let integration = tempfile::tempdir().unwrap();
    let integration_path = integration.path().join("checkout");
    clone_integration(source.path(), &integration_path);
    let integration_path = std::fs::canonicalize(&integration_path).unwrap();

    let state = tempfile::tempdir().unwrap();
    let manager = open_manager(source.path(), state.path());
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let accepted = accept_candidate(
        &store,
        &manager,
        capacity,
        "child-1",
        "transaction-1",
        "added.txt",
        "landed\n",
    )
    .await;
    let authority = store
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            ChildGatePlan {
                required_gates: Vec::new(),
            },
        )
        .unwrap();
    let outcome = authorize_and_land(
        &store,
        &authority,
        &accepted,
        &manager,
        &integration_path,
        "refs/heads/main",
    )
    .await
    .expect("authorized landing");
    assert!(matches!(outcome, ParentLandingAuthorization::Landed { .. }));

    // Drop every live journal handle to simulate a process restart.
    drop(store);
    drop(children);
    drop(accepted);
    drop(journal);

    // Recovery derives the landing state SOLELY from deterministic journal
    // replay off disk (the real restart-reconciliation path — no writer lease).
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
        "landing state must be derived solely from journal replay"
    );
}

/// A concurrent second lander whose base is now stale cannot double-land: the
/// parent tip advanced past the candidate base, so the landing fails closed and
/// the integration checkout keeps exactly the first landing.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + landing is exercised on the Linux harness"
)]
async fn concurrent_second_lander_cannot_double_land() {
    let journal_dir = tempfile::tempdir().unwrap();
    let journal_path = journal_dir.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    children
        .declare(child_record("child-a", "declare-a", "workspace-a"))
        .unwrap();
    children
        .declare(child_record("child-b", "declare-b", "workspace-b"))
        .unwrap();
    let store = ChildTransactionStore::new(journal.clone());

    let source = tempfile::tempdir().unwrap();
    init_repo(source.path());
    let integration = tempfile::tempdir().unwrap();
    let integration_path = integration.path().join("checkout");
    clone_integration(source.path(), &integration_path);
    let integration_path = std::fs::canonicalize(&integration_path).unwrap();

    let state = tempfile::tempdir().unwrap();
    let manager = open_manager(source.path(), state.path());
    let capacity = manager.workspace_capacity(2).await.expect("capacity");

    let accepted_a = accept_candidate(
        &store,
        &manager,
        capacity,
        "child-a",
        "transaction-a",
        "a.txt",
        "A\n",
    )
    .await;
    let accepted_b = accept_candidate(
        &store,
        &manager,
        capacity,
        "child-b",
        "transaction-b",
        "b.txt",
        "B\n",
    )
    .await;
    let authority_a = store
        .open(
            "transaction-a",
            ChildId::new("child-a").unwrap(),
            revision('1'),
            ChildGatePlan {
                required_gates: Vec::new(),
            },
        )
        .unwrap();
    let authority_b = store
        .open(
            "transaction-b",
            ChildId::new("child-b").unwrap(),
            revision('1'),
            ChildGatePlan {
                required_gates: Vec::new(),
            },
        )
        .unwrap();

    // A lands.
    let landed_a = authorize_and_land(
        &store,
        &authority_a,
        &accepted_a,
        &manager,
        &integration_path,
        "refs/heads/main",
    )
    .await
    .expect("landing A");
    assert!(matches!(
        landed_a,
        ParentLandingAuthorization::Landed { .. }
    ));
    let head_after_a = git_stdout(&integration_path, &["rev-parse", "HEAD"]);
    assert!(integration_path.join("a.txt").is_file());

    // B was built on the same base; the parent tip has advanced past it. B must
    // fail closed (Err or a non-landing outcome) and must NOT land.
    let landed_b = authorize_and_land(
        &store,
        &authority_b,
        &accepted_b,
        &manager,
        &integration_path,
        "refs/heads/main",
    )
    .await;
    match landed_b {
        Err(_) => {}
        Ok(ParentLandingAuthorization::Landed { .. }) => {
            panic!("second lander double-landed on a stale base")
        }
        Ok(_) => {}
    }
    // The integration checkout keeps exactly A's landing; B never landed.
    assert_eq!(
        git_stdout(&integration_path, &["rev-parse", "HEAD"]),
        head_after_a
    );
    assert!(!integration_path.join("b.txt").exists());
    let durable_b = store.inspect("transaction-b").unwrap().unwrap();
    assert!(
        !matches!(durable_b.landing_state(), Some(LandingState::Landed { .. })),
        "B must not reach a durable Landed state"
    );

    manager
        .release_transaction(accepted_a.guard().workspace())
        .ok();
    manager
        .release_transaction(accepted_b.guard().workspace())
        .ok();
}

/// Rollback refuses after a foreign change advances the target past the landed
/// successor: the reverse CAS stops in RecoveryRequired and the foreign commit
/// is never erased.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout + landing is exercised on the Linux harness"
)]
async fn rollback_refuses_after_foreign_change() {
    let journal_dir = tempfile::tempdir().unwrap();
    let journal_path = journal_dir.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    children
        .declare(child_record(
            "child-1",
            "declare-child-1",
            "workspace-child-1",
        ))
        .unwrap();
    let store = ChildTransactionStore::new(journal.clone());

    let source = tempfile::tempdir().unwrap();
    init_repo(source.path());
    let integration = tempfile::tempdir().unwrap();
    let integration_path = integration.path().join("checkout");
    clone_integration(source.path(), &integration_path);
    let integration_path = std::fs::canonicalize(&integration_path).unwrap();

    let state = tempfile::tempdir().unwrap();
    let manager = open_manager(source.path(), state.path());
    let capacity = manager.workspace_capacity(1).await.expect("capacity");
    let accepted = accept_candidate(
        &store,
        &manager,
        capacity,
        "child-1",
        "transaction-1",
        "added.txt",
        "landed\n",
    )
    .await;
    let authority = store
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            ChildGatePlan {
                required_gates: Vec::new(),
            },
        )
        .unwrap();
    let landed = authorize_and_land(
        &store,
        &authority,
        &accepted,
        &manager,
        &integration_path,
        "refs/heads/main",
    )
    .await
    .expect("landing");
    let rollback = match landed {
        ParentLandingAuthorization::Landed { rollback, .. } => rollback,
        other => panic!("expected Landed, got {other:?}"),
    };

    // A foreign change advances the branch past the landed successor.
    std::fs::write(integration_path.join("foreign.txt"), "foreign\n").unwrap();
    run_git(&integration_path, &["add", "foreign.txt"]);
    run_git(&integration_path, &["commit", "-m", "foreign"]);
    let foreign_head = git_stdout(&integration_path, &["rev-parse", "HEAD"]);

    // Rollback must refuse and record recovery, never erasing the foreign work.
    let refused =
        authorize_and_rollback(&store, &authority, &manager, &integration_path, &rollback)
            .await
            .expect("rollback authorization returns a recovery outcome");
    assert!(
        matches!(refused, ParentLandingAuthorization::RecoveryRequired { .. }),
        "rollback after foreign drift must require recovery, got {refused:?}"
    );
    assert_eq!(
        git_stdout(&integration_path, &["rev-parse", "HEAD"]),
        foreign_head,
        "the foreign commit must survive a refused rollback"
    );
    assert!(integration_path.join("foreign.txt").is_file());
    let durable = store.inspect("transaction-1").unwrap().unwrap();
    assert!(matches!(
        durable.landing_state(),
        Some(LandingState::RecoveryRequired { .. })
    ));

    manager
        .release_transaction(accepted.guard().workspace())
        .ok();
}

/// Forgery resistance: the landing authority events are rejected by the public
/// `SessionJournal::append` denylist, so only the authorized store path can mint
/// landing/recovery/rollback authority. Runs on every platform (no git needed).
#[test]
fn public_append_rejects_landing_authority_events() {
    let journal_dir = tempfile::tempdir().unwrap();
    let journal_path = journal_dir.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();

    let subject = wcore_agent::session_journal::LandingSubject {
        accepted_receipt_digest: digest('a'),
        target_ref: "refs/heads/main".into(),
        base_commit: revision('1'),
        expected_commit: revision('1'),
        expected_tree: revision('2'),
        symbolic_head: Some("refs/heads/main".into()),
        index_tree: revision('2'),
        worktree_digest: digest('c'),
        lock_identity: "lock".into(),
        preimage_digest: digest('f'),
    };
    let successor = wcore_agent::session_journal::LandingSuccessor {
        landed_commit: revision('3'),
        landed_tree: revision('4'),
        quarantine_ref: "refs/wayland/landing/x".into(),
    };
    // Every one of the eight landing authority variants must be denied.
    let events = [
        SessionEvent::ChildTransactionLandingPrepared {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            subject,
        },
        SessionEvent::ChildTransactionLandingRefAdvanced {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            successor: successor.clone(),
        },
        SessionEvent::ChildTransactionLandingProjected {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            successor: successor.clone(),
        },
        SessionEvent::ChildTransactionLanded {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            successor: successor.clone(),
        },
        SessionEvent::ChildTransactionLandingConflict {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            detail: "conflict".into(),
        },
        SessionEvent::ChildTransactionLandingRecoveryRequired {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            detail: "recovery".into(),
        },
        SessionEvent::ChildTransactionRollbackPrepared {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            successor: successor.clone(),
        },
        SessionEvent::ChildTransactionRolledBack {
            transaction_id: "t".into(),
            opening_token_digest: digest('e'),
            successor,
        },
    ];
    assert_eq!(
        events.len(),
        8,
        "all eight landing authority variants covered"
    );
    for event in events {
        let error = journal
            .append(event)
            .expect_err("public append must reject a landing authority event");
        assert!(
            error.to_string().contains("require ChildTransactionStore"),
            "unexpected error: {error}"
        );
    }
}
