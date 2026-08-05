//! 06D black-box proof — a qualified host candidate reaches an opaque,
//! guard-owned `AcceptedCandidate` ONLY through the public parent-owned gate
//! acceptance surface (`run_gate_acceptance`) after the authoritative durable
//! receipt is appended, reopened, reduced, and matched.
//!
//! Every type here is reached through the crate's PUBLIC API. The candidate is
//! a real isolated git checkout (the same `wcore_swarm` machinery production
//! uses), sealed and wrapped in a `MutationAttemptGuard`; acceptance is minted
//! exclusively by `run_gate_acceptance`, whose returned `AcceptedCandidate`
//! owns the still-armed guard and seal. Dropping it terminalizes the
//! non-landing transaction and removes the checkout from disk.
//!
//! Scope note (surfaced to the 20-14 audit): the per-gate closure digest that
//! a non-empty `ChildGatePlan` must pin is computed by the crate-private
//! `AuthorizedGateClosure::closure_digest` / `AuthorizedGateClosureRegistry`,
//! so a black-box caller cannot author a matching non-empty durable plan
//! without a production change (out of this packet's scope). This positive
//! proof therefore drives the acceptance PIPELINE end-to-end over a durably
//! opened transaction; the live per-gate containment execution is proven
//! separately, through the sandbox public API, in
//! `wcore-sandbox/tests/hard_process_containment.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wcore_agent::child_transaction::{
    ChildTransactionStore, GateExecutionSubject, MutationAttemptGuard, run_gate_acceptance,
};
use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::session_journal::SessionJournal;
use wcore_sandbox::{FailClosedBackend, SandboxRegistry};
use wcore_swarm::worktree::{CandidateSeal, WorktreeManager};
use wcore_types::child_transaction::{ChildGatePlan, ChildTransactionDisposition};
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

/// Build a real isolated git checkout, seal it, and wrap the armed handle in a
/// `MutationAttemptGuard`. Returns the guard, its live seal, and the checkout
/// root captured before the workspace is moved into the guard.
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

/// A qualified real host candidate (isolated git checkout, sealed) is accepted
/// only after the durable append/reopen/reduce/match round-trip, and the opaque
/// guard-owned `AcceptedCandidate` terminalizes its checkout on drop.
///
/// A gate-less durable plan keeps the proof inside the black-box public
/// surface (see the module scope note) while still exercising the full
/// `run_gate_acceptance` path: subject-binds-plan check → parent-owned executor
/// → acceptance-machine validation → authoritative durable receipt closure →
/// guard-owned mint.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "isolated git checkout lifecycle is exercised on the Linux harness"
)]
async fn qualified_host_candidate_to_accepted_candidate() {
    // Durable side: a real journal-backed transaction over an isolated child.
    let journal_dir = tempfile::tempdir().unwrap();
    let journal_path = journal_dir.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    children.declare(child_record()).unwrap();
    let store = ChildTransactionStore::new(journal.clone());

    // A gate-less plan: structurally valid, and its canonical digest is
    // publicly computable, so the black-box caller can bind the subject to it.
    let plan = ChildGatePlan {
        required_gates: Vec::new(),
    };
    let plan_digest = plan.canonical_digest().unwrap();
    let authority = store
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            plan.clone(),
        )
        .unwrap();

    // Live host candidate: a real isolated git checkout, sealed, then wrapped in
    // the still-armed guard. Capture the on-disk checkout root before the
    // workspace is moved into the guard.
    let repo = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (guard, seal, checkout_root) =
        isolated_checkout(repo.path(), state.path(), "accept-child").await;
    assert!(
        checkout_root.is_dir(),
        "the isolated candidate checkout must exist before acceptance"
    );

    // The execution subject binds the orchestrator-owned durable plan.
    let subject = GateExecutionSubject {
        base_revision: revision('1'),
        candidate_revision: revision('2'),
        diff_digest: digest('d'),
        request_digest: digest('a'),
        policy_digest: digest('b'),
        gate_plan_digest: plan_digest.clone(),
    };

    // A FailClosed sandbox is never invoked for a gate-less plan (the executor
    // walks zero gates), so acceptance rests purely on the durable pipeline.
    let sandbox = SandboxRegistry::new(Arc::new(FailClosedBackend::new()));

    let accepted = run_gate_acceptance(
        &sandbox,
        &store,
        &authority,
        &subject,
        Vec::new(),
        guard,
        seal,
        1_000,
    )
    .await
    .expect("a qualified host candidate must reach AcceptedCandidate");

    // The opaque acceptance binds this transaction and a durable receipt digest.
    assert_eq!(accepted.transaction_id(), "transaction-1");
    let accepted_digest = accepted.accepted_receipt_digest().to_owned();
    assert_eq!(accepted_digest.len(), 64, "receipt digest must be 64 hex");

    // The acceptance rests on a DURABLE receipt: reopen the transaction and
    // confirm the exact digest was appended, and that the disposition is
    // `Active` — this packet makes NO landing / merge claim.
    let durable = store.inspect("transaction-1").unwrap().unwrap();
    let committed = durable
        .receipts
        .iter()
        .find(|committed| committed.receipt_digest == accepted_digest)
        .expect("the accepted receipt digest must be durably committed");
    assert_eq!(
        committed.receipt.disposition,
        ChildTransactionDisposition::Active,
        "a non-landing acceptance must remain Active"
    );
    assert_eq!(
        committed.receipt.gate_plan_digest, plan_digest,
        "the durable receipt must bind the orchestrator-owned plan"
    );

    // The acceptance genuinely owns the live guard/seal: the checkout is still
    // on disk while the acceptance is held.
    let _ = accepted.guard();
    let _ = accepted.seal();
    assert!(
        checkout_root.is_dir(),
        "the checkout must remain while the accepted candidate is held"
    );

    // Dropping the acceptance drops the seal, then the guard, whose checkout
    // cleanup terminalizes the non-landing transaction and removes the checkout.
    drop(accepted);
    assert!(
        !checkout_root.exists(),
        "acceptance drop must terminalize and remove the candidate checkout"
    );
}
