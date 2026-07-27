//! The durable Fleet task ledger: at-most-once effect and exactly-once unblock.
//!
//! Written from plan 22-03 Task 3's behavior list, not from the implementation.
//! Every test asserts an INVARIANT — a refusal happening at all, a state, a
//! replay equality, a count — and never an error string or a numeric status. A
//! test that pins today's refusal message keeps passing when the refusal moves
//! to a weaker cause, which is the specific way this suite could rot into
//! decoration.
//!
//! The load-bearing one is
//! `a_superseded_owners_late_completion_is_refused_while_it_is_still_alive`.
//! That is the fencing property the whole claim model was chosen for: the
//! panel's decisive argument was that heartbeat and OS-liveness options both
//! record `refuses_late_write=nothing`, and only the lease-plus-epoch bounds
//! duplicate *effect* rather than duplicate *execution*.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use wcore_agent::child_transaction::ChildTransactionStore;
use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::goal::{
    ClaimOutcome, GoalKernel, GoalLedger, GoalTaskAttemptStatus, TaskAuthority,
};
use wcore_agent::session_journal::{
    BudgetAmount, BudgetOwner, BudgetPurpose, BudgetUnit, SessionEvent, SessionJournal,
};
use wcore_types::child_transaction::{ChildGatePlan, ChildGateRequirement};
use wcore_types::goal::{
    GoalAuthorityRequest, GoalAuthoritySnapshot, GoalId, GoalStrategy, GoalTerminalState,
    LoopPolicy, TaskId, TaskUnknownReason, resolve_goal_authority,
};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus,
};

const SESSION: &str = "session-1";
const GOAL: &str = "g-fleet";

fn limits(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

/// A Goal envelope with a token ceiling of 100, so a task's attempts can be
/// driven past the Goal's authorized reservation deliberately.
fn snapshot() -> GoalAuthoritySnapshot {
    let request = GoalAuthorityRequest {
        requested_limits: limits(&[("max_tokens", 100)]),
        strategy: GoalStrategy::Fleet,
        loop_policy: LoopPolicy::Fixed { iterations: 8 },
    };
    resolve_goal_authority(
        &request,
        &limits(&[("max_tokens", 1000)]),
        "parent-envelope-digest",
    )
}

fn goal_id() -> GoalId {
    GoalId::new(GOAL)
}

fn task(name: &str) -> TaskId {
    TaskId::new(name)
}

fn deps(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

struct Fixture {
    journal: SessionJournal,
    ledger: GoalLedger,
}

/// Open a journal with one authorized Goal on it.
fn fixture(path: &Path) -> Fixture {
    let journal = SessionJournal::open(path, SESSION).expect("journal opens");
    GoalKernel::new(journal.clone())
        .open_goal(&goal_id(), "fan out durably", &snapshot(), 1_700_000_000_000)
        .expect("goal opens");
    Fixture {
        ledger: GoalLedger::new(journal.clone()),
        journal,
    }
}

impl Fixture {
    /// Commit a budget reservation through the EXISTING budget events, owned by
    /// a real durable child.
    ///
    /// This is the seam a claim must re-enter rather than mint a fresh budget
    /// beside it: the reservation is charged to a declared child under
    /// `BudgetPurpose::ChildExecution`, which is what a Fleet worker actually
    /// is. Reserving against `BudgetOwner::Session` would have made the test
    /// shorter and would have stopped proving the child half.
    fn reserve(&self, reservation_id: &str, tokens: u64) {
        DurableChildStore::new(self.journal.clone())
            .declare(child_record(reservation_id))
            .expect("child declares");
        self.journal
            .append(SessionEvent::BudgetReserved {
                event_id: format!("evt-{reservation_id}"),
                reservation_id: reservation_id.to_owned(),
                owner: BudgetOwner::Child {
                    child_id: reservation_id.to_owned(),
                },
                purpose: BudgetPurpose::ChildExecution,
                amount: BudgetAmount {
                    value: tokens,
                    unit: BudgetUnit::Tokens,
                },
            })
            .expect("reservation commits");
    }

    fn declare(&self, name: &str, depends_on: &[&str]) {
        self.ledger
            .declare_task(&goal_id(), &task(name), &deps(depends_on), &format!("idem-{name}"))
            .expect("task declares");
    }

    /// Claim a task, reserving for it first, and insist on a win.
    fn win(&self, name: &str, worker: &str, tokens: u64) -> TaskAuthority {
        let reservation = format!("res-{name}-{worker}");
        self.reserve(&reservation, tokens);
        match self
            .ledger
            .claim_task(&goal_id(), &task(name), worker, &reservation, 30_000)
            .expect("claim decides")
        {
            ClaimOutcome::Won(authority) => authority,
            ClaimOutcome::Lost { detail } => panic!("expected to win the claim: {detail}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Behavior 1 — dependencies gate claimability, and each unblocks exactly once.
// ---------------------------------------------------------------------------

#[test]
fn a_task_with_unmet_dependencies_is_not_claimable_and_unblocks_exactly_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(&temp.path().join("session.journal"));

    fixture.declare("root", &[]);
    fixture.declare("leaf", &["root"]);

    let claimable = fixture.ledger.claimable(&goal_id()).expect("claimable reads");
    assert_eq!(claimable, vec![task("root")]);

    // The refusal is durable, not advisory: a worker that ignores `claimable`
    // and claims anyway must still be refused.
    fixture.reserve("res-early", 10);
    let early = fixture
        .ledger
        .claim_task(&goal_id(), &task("leaf"), "w-eager", "res-early", 30_000)
        .expect("claim decides");
    assert!(matches!(early, ClaimOutcome::Lost { .. }));

    let root = fixture.win("root", "w-1", 10);
    fixture
        .ledger
        .complete_task(&root, GoalTerminalState::SelfChecked, "effect-root")
        .expect("root completes");

    assert_eq!(
        fixture.ledger.claimable(&goal_id()).expect("claimable reads"),
        vec![task("leaf")]
    );

    // Replaying the whole chain must not accumulate a release per replay: the
    // count is a property of the transitions, not of how many times they were
    // read back.
    let replayed = GoalLedger::new(
        SessionJournal::open(&temp.path().join("session.journal"), SESSION).expect("reopen"),
    );
    let leaf = replayed
        .task(&goal_id(), &task("leaf"))
        .expect("read")
        .expect("leaf exists");
    assert_eq!(leaf.dependency_releases, 1);
}

// ---------------------------------------------------------------------------
// Behavior 2 — a claim race has exactly one winner and the loser is told.
// ---------------------------------------------------------------------------

#[test]
fn two_workers_racing_for_one_task_produce_exactly_one_claim_and_the_loser_is_told() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(&temp.path().join("session.journal"));
    fixture.declare("t", &[]);

    let winner = fixture.win("t", "w-1", 10);

    fixture.reserve("res-loser", 10);
    let loser = fixture
        .ledger
        .claim_task(&goal_id(), &task("t"), "w-2", "res-loser", 30_000)
        .expect("claim decides");

    // Told it lost — not handed a silent no-op it could mistake for a win.
    assert!(matches!(loser, ClaimOutcome::Lost { .. }));

    let state = fixture
        .ledger
        .task(&goal_id(), &task("t"))
        .expect("read")
        .expect("task exists");
    assert_eq!(state.attempts.len(), 1);
    assert_eq!(state.live_attempt().map(|a| a.worker_id.as_str()), Some("w-1"));
    assert_eq!(winner.epoch(), state.epoch());
}

// ---------------------------------------------------------------------------
// Behavior 3 — THE FENCE. The single most important test in this plan.
// ---------------------------------------------------------------------------

#[test]
fn a_superseded_owners_late_completion_is_refused_while_it_is_still_alive() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(&temp.path().join("session.journal"));
    fixture.declare("t", &[]);

    // The old owner is still alive and still holding its authority — this is
    // the "merely slow" case a timeout cannot distinguish from a dead one.
    let superseded = fixture.win("t", "w-old", 10);

    fixture
        .ledger
        .revoke_claim(&goal_id(), &task("t"), "lease expired")
        .expect("revocation commits");
    let successor = fixture.win("t", "w-new", 10);

    let late = fixture
        .ledger
        .complete_task(&superseded, GoalTerminalState::SelfChecked, "effect-old");
    assert!(late.is_err(), "the superseded owner's late write must be refused");

    // And it is refused for every effect-bearing path it holds, not just the
    // one this test happens to call first.
    assert!(fixture.ledger.prove_liveness(&superseded, 1).is_err());
    assert!(
        fixture
            .ledger
            .record_unknown_outcome(&superseded, TaskUnknownReason::ReceiptMissing)
            .is_err()
    );

    // The successor is unaffected: the fence refuses the stale authority, it
    // does not wedge the task.
    fixture
        .ledger
        .complete_task(&successor, GoalTerminalState::SelfChecked, "effect-new")
        .expect("the current owner completes");

    let state = fixture
        .ledger
        .task(&goal_id(), &task("t"))
        .expect("read")
        .expect("task exists");
    assert_eq!(
        state.completion.as_ref().map(|c| c.effect_digest.as_str()),
        Some("effect-new")
    );
    assert_eq!(state.attempts.len(), 2);
}

// ---------------------------------------------------------------------------
// Behavior 4 — N attempts cannot cost N reservations.
// ---------------------------------------------------------------------------

#[test]
fn attempts_re_enter_the_existing_budget_seam_and_cannot_outspend_the_goals_envelope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(&temp.path().join("session.journal"));
    fixture.declare("t", &[]);

    // A claim naming a reservation that was never committed through the budget
    // events is refused: the ledger cannot mint a budget of its own.
    let unbacked = fixture
        .ledger
        .claim_task(&goal_id(), &task("t"), "w-1", "res-never-reserved", 30_000)
        .expect("claim decides");
    assert!(matches!(unbacked, ClaimOutcome::Lost { .. }));

    // Two attempts at 40 fit inside the Goal's authorized 100.
    let first = fixture.win("t", "w-1", 40);
    fixture
        .ledger
        .revoke_claim(&goal_id(), &task("t"), "owner died")
        .expect("revocation commits");
    let _second = fixture.win("t", "w-2", 40);
    fixture
        .ledger
        .revoke_claim(&goal_id(), &task("t"), "owner died again")
        .expect("revocation commits");

    // The third would take the task's running total to 120 against a Goal that
    // authorized 100, so it is refused. Without this, three retries of one task
    // cost three times what the Goal authorized.
    fixture.reserve("res-third", 40);
    let third = fixture
        .ledger
        .claim_task(&goal_id(), &task("t"), "w-3", "res-third", 30_000)
        .expect("claim decides");
    assert!(matches!(third, ClaimOutcome::Lost { .. }));

    // Nor can an attempt dodge the accounting by re-presenting an identity that
    // has already been charged to this task.
    let reused = fixture
        .ledger
        .claim_task(&goal_id(), &task("t"), "w-4", "res-t-w-1", 30_000)
        .expect("claim decides");
    assert!(matches!(reused, ClaimOutcome::Lost { .. }));

    assert_eq!(first.worker_id(), "w-1");
    let state = fixture
        .ledger
        .task(&goal_id(), &task("t"))
        .expect("read")
        .expect("task exists");
    assert_eq!(state.attempts.len(), 2);
}

// ---------------------------------------------------------------------------
// Behavior 5 — a completion is durable at production, not at delivery.
// ---------------------------------------------------------------------------

#[test]
fn a_completion_produced_before_a_crash_is_still_delivered_to_the_parent_after_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");

    {
        let fixture = fixture(&path);
        fixture.declare("t", &[]);
        let authority = fixture.win("t", "w-1", 10);
        fixture
            .ledger
            .complete_task(&authority, GoalTerminalState::SelfChecked, "effect-t")
            .expect("completion commits");
        // The parent never observes it: this process ends here, exactly as a
        // worker that finishes and dies before delivery does.
    }

    let restarted = GoalLedger::new(SessionJournal::open(&path, SESSION).expect("reopen"));
    assert_eq!(
        restarted.pending_deliveries(&goal_id()).expect("outbox reads"),
        vec![task("t")]
    );

    restarted
        .deliver_completion(&goal_id(), &task("t"))
        .expect("parent wakes with the completion");
    assert!(
        restarted
            .pending_deliveries(&goal_id())
            .expect("outbox reads")
            .is_empty()
    );

    // Delivery is not repeatable: a second drain of the same completion would
    // wake the parent twice for one piece of work.
    assert!(restarted.deliver_completion(&goal_id(), &task("t")).is_err());
}

// ---------------------------------------------------------------------------
// Behavior 6 — an unestablished outcome parks; it never silently retries.
// ---------------------------------------------------------------------------

#[test]
fn an_attempt_whose_outcome_cannot_be_established_parks_rather_than_retrying() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture = fixture(&temp.path().join("session.journal"));
    fixture.declare("t", &[]);

    let authority = fixture.win("t", "w-1", 10);
    fixture
        .ledger
        .record_unknown_outcome(&authority, TaskUnknownReason::OwnerDiedMidAttempt)
        .expect("unknown commits");

    let state = fixture
        .ledger
        .task(&goal_id(), &task("t"))
        .expect("read")
        .expect("task exists");
    assert!(state.requires_resolution());
    // Not a completion. An unknown outcome that counted as done would lose the
    // work; one that counted as failed would invite the silent retry.
    assert!(state.completion.is_none());
    assert!(matches!(
        state.attempts.last().map(|a| &a.status),
        Some(GoalTaskAttemptStatus::Unknown { .. })
    ));

    assert!(
        fixture
            .ledger
            .claimable(&goal_id())
            .expect("claimable reads")
            .is_empty()
    );
    assert_eq!(
        fixture
            .ledger
            .requiring_resolution(&goal_id())
            .expect("resolution queue reads"),
        vec![task("t")]
    );

    // And the durable boundary refuses a retry too, not only the query surface.
    fixture.reserve("res-retry", 10);
    let retry = fixture
        .ledger
        .claim_task(&goal_id(), &task("t"), "w-2", "res-retry", 30_000)
        .expect("claim decides");
    assert!(matches!(retry, ClaimOutcome::Lost { .. }));
}

// ---------------------------------------------------------------------------
// Behavior 7 — workspace handoff goes through the delegated-mutation lifecycle.
// ---------------------------------------------------------------------------

fn child_record(child_id: &str) -> DurableChildRecord {
    let filled = |c: char| -> String { std::iter::repeat_n(c, 64).collect() };
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: format!("declare-{child_id}"),
        child_id: ChildId::new(child_id).unwrap(),
        parent: ChildParent {
            session_id: SESSION.into(),
            turn_id: None,
            parent_child_id: None,
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: None,
        },
        origin: ChildOrigin::Delegate,
        request: ChildRequestEvidence::redacted(filled('a')),
        policy_snapshot: ChildPolicySnapshot {
            contract_version: "effective-execution-policy/v1".into(),
            exact_digest: filled('b'),
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
            workspace_id: format!("workspace-{child_id}"),
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

#[test]
fn workspace_ownership_moves_only_through_a_committed_delegated_mutation_transaction() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let fixture = fixture(&path);
    fixture.declare("t", &[]);
    let owner = fixture.win("t", "w-old", 10);

    // A handoff naming a transaction that does not exist in reduced state is
    // refused. There is no other transition that changes an owner, so this is
    // the whole surface: a field update without the lifecycle is not
    // expressible rather than merely discouraged.
    fixture.reserve("res-ghost", 10);
    assert!(
        fixture
            .ledger
            .hand_off_workspace(&owner, "transaction-that-never-opened", "w-new", "res-ghost", 30_000)
            .is_err()
    );

    // Now open a real one through the Phase 20 store, against the child that
    // already owns the handoff's reservation.
    fixture.reserve("res-handoff", 10);
    ChildTransactionStore::new(fixture.journal.clone())
        .open(
            "transaction-1",
            ChildId::new("res-handoff").unwrap(),
            std::iter::repeat_n('1', 40).collect::<String>(),
            ChildGatePlan {
                required_gates: vec![ChildGateRequirement {
                    gate_id: "cargo-test".into(),
                    gate_closure_digest: std::iter::repeat_n('c', 64).collect::<String>(),
                }],
            },
        )
        .expect("transaction opens");

    let new_owner = fixture
        .ledger
        .hand_off_workspace(&owner, "transaction-1", "w-new", "res-handoff", 30_000)
        .expect("handoff commits");

    // The handoff is itself a supersession: the old owner is fenced out by it.
    assert!(
        fixture
            .ledger
            .complete_task(&owner, GoalTerminalState::SelfChecked, "effect-old")
            .is_err()
    );
    fixture
        .ledger
        .complete_task(&new_owner, GoalTerminalState::SelfChecked, "effect-new")
        .expect("the new owner completes");

    let state = fixture
        .ledger
        .task(&goal_id(), &task("t"))
        .expect("read")
        .expect("task exists");
    assert_eq!(state.handoffs.len(), 1);
    assert_eq!(state.handoffs[0].transaction_id, "transaction-1");
    assert_eq!(state.handoffs[0].to_worker, "w-new");
}

// ---------------------------------------------------------------------------
// Behavior 8 — cancellation cascades and no cancelled effect lands afterward.
// ---------------------------------------------------------------------------

#[test]
fn cancelling_a_goal_cascades_to_claimed_tasks_and_no_later_effect_lands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let fixture = fixture(&path);
    fixture.declare("a", &[]);
    fixture.declare("b", &[]);

    let claimed = fixture.win("a", "w-1", 10);
    let also_claimed = fixture.win("b", "w-2", 10);

    GoalKernel::new(fixture.journal.clone())
        .terminate(&goal_id(), GoalTerminalState::Cancelled)
        .expect("goal cancels");

    // Both in-flight owners are fenced out by the cascade, including one that
    // was never touched by the cancelling code path.
    assert!(
        fixture
            .ledger
            .complete_task(&claimed, GoalTerminalState::SelfChecked, "effect-a")
            .is_err()
    );
    assert!(
        fixture
            .ledger
            .complete_task(&also_claimed, GoalTerminalState::SelfChecked, "effect-b")
            .is_err()
    );

    let replayed = GoalLedger::new(SessionJournal::open(&path, SESSION).expect("reopen"));
    for name in ["a", "b"] {
        let state = replayed
            .task(&goal_id(), &task(name))
            .expect("read")
            .expect("task exists");
        assert!(state.completion.is_none());
        assert!(state.live_attempt().is_none());
        assert!(matches!(
            state.attempts.last().map(|attempt| &attempt.status),
            Some(GoalTaskAttemptStatus::Revoked { .. })
        ));
    }
}

// ---------------------------------------------------------------------------
// Behavior 9 — the in-memory ledger is a projection, never the source of truth.
// ---------------------------------------------------------------------------

#[test]
fn the_ledger_replays_from_the_journal_to_the_same_state_after_a_fresh_load() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");

    let before = {
        let fixture = fixture(&path);
        fixture.declare("root", &[]);
        fixture.declare("mid", &["root"]);
        fixture.declare("leaf", &["mid"]);

        let root = fixture.win("root", "w-1", 10);
        fixture.ledger.prove_liveness(&root, 1_700_000_001_000).unwrap();
        fixture
            .ledger
            .complete_task(&root, GoalTerminalState::SelfChecked, "effect-root")
            .unwrap();
        fixture.ledger.deliver_completion(&goal_id(), &task("root")).unwrap();

        let mid = fixture.win("mid", "w-2", 10);
        fixture
            .ledger
            .revoke_claim(&goal_id(), &task("mid"), "lease expired")
            .unwrap();
        let mid_again = fixture.win("mid", "w-3", 10);
        fixture
            .ledger
            .record_unknown_outcome(&mid_again, TaskUnknownReason::LeaseExpiredWhileOwnerLive)
            .unwrap();
        assert_ne!(mid.epoch(), mid_again.epoch());

        fixture
            .journal
            .state()
            .expect("state reduces")
            .goals
            .get(GOAL)
            .expect("goal exists")
            .tasks
            .clone()
    };

    let after = SessionJournal::open(&path, SESSION)
        .expect("reopen")
        .state()
        .expect("state reduces")
        .goals
        .get(GOAL)
        .expect("goal exists")
        .tasks
        .clone();

    assert_eq!(before, after);
    // And the projection is not vacuously equal — it carries the whole history.
    assert_eq!(after.len(), 3);
    assert_eq!(after["mid"].attempts.len(), 2);
    assert_eq!(after["root"].dependency_releases, 1);
    assert_eq!(after["mid"].dependency_releases, 1);
    assert_eq!(after["leaf"].dependency_releases, 0);
}

// ---------------------------------------------------------------------------
// The structural half of the fence: the kernel is the sole writer.
// ---------------------------------------------------------------------------

#[test]
fn the_public_append_path_refuses_to_mint_a_task_record_beside_the_ledger() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let fixture = fixture(&path);
    fixture.declare("t", &[]);

    // A caller holding a journal handle cannot declare a task, which is what
    // makes "the ledger is the sole writer" structural rather than a
    // convention. Without this, every other guarantee here is advisory.
    let declared = fixture.journal.append(SessionEvent::GoalTaskDeclared {
        goal_id: GOAL.to_owned(),
        task_id: "smuggled".to_owned(),
        depends_on: BTreeSet::new(),
        idempotency_key: "idem-smuggled".to_owned(),
    });
    assert!(declared.is_err());

    let state = fixture.journal.state().expect("state reduces");
    assert!(!state.goals[GOAL].tasks.contains_key("smuggled"));
}
