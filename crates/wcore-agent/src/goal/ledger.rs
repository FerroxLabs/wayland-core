//! The durable Fleet task ledger (F22-03), built above the existing executors.
//!
//! ## What this is, and what it deliberately is not
//!
//! It is a ledger ABOVE `FleetDispatcher`, the spawner and ForgeFlows, not a
//! replacement for any of them. It records what none of them persist: a task's
//! dependencies, its attempt history, who owns it now, when that owner last
//! proved it was alive, whether its completion was ever delivered to the
//! parent, and whether an outcome could be established at all.
//!
//! It is NOT a second store. Every record is a `SessionEvent` appended into the
//! same F12 chain the Goal's own transitions live in, through the same
//! `append_built_from_head` seam Phase 21 repaired for the parallel-sibling
//! TOCTOU. A Goal and its tasks therefore cannot disagree after a crash,
//! because there is only one thing to disagree with.
//!
//! ## The fence
//!
//! At-most-once execution after a kill is a fencing problem, not a locking one:
//! a claim held by a dead process is indistinguishable from one held by a slow
//! process. The authorized model (`22-03-CLAIM-MODEL.md`,
//! `lease-with-fencing-token`, 4-of-4) pairs a time-bounded lease with a
//! monotonic claim epoch, and the epoch is what refuses the merely-slow owner.
//!
//! The fence is structural in two layers that are both required:
//!
//! 1. **In the type system.** [`TaskAuthority`] has private fields, no public
//!    constructor and no `Deserialize`. The only routes to one are
//!    [`GoalLedger::claim_task`] and [`GoalLedger::hand_off_workspace`], both of
//!    which only return one to the winner. Every method here that can record an
//!    effect on behalf of a task takes `&TaskAuthority`, so a caller that never
//!    won a claim cannot *express* an effect-recording call. This is the same
//!    shape `VerifiedTerminal` uses for the reserved `verified` stamp, and for
//!    the same reason: a guard each contributor has to remember is the hole the
//!    design exists to close.
//!
//! 2. **At the durable boundary.** The reducer compares the presented epoch
//!    against the task's committed epoch before applying anything. That layer is
//!    what makes the property hold against a hand-built journal record rather
//!    than only against well-behaved callers — the type-system layer alone would
//!    be a convention with a compiler behind it, not a fence.
//!
//! ## What the fence does not cover, stated rather than glossed
//!
//! It bounds duplicate **effect recording**. It does not reach inside a worker
//! process to stop that process writing to a file it already holds. That is why
//! every task carries an `idempotency_key`: the ledger fences who may record a
//! completion, and the key is what stops the effect itself landing twice when an
//! attempt is legitimately retried after an owner died. Both halves are needed
//! and neither substitutes for the other.

use std::collections::BTreeSet;

use wcore_types::goal::{GoalId, GoalTerminalState, TaskId, TaskUnknownReason};

use crate::session_journal::{
    GoalState, GoalTaskState, GoalTaskTransition, JournalError, ReducedSessionState, SessionEvent,
    SessionJournal,
};

/// Proof that its holder owns a task at a specific claim epoch.
///
/// There is no public constructor, no public field and no `Deserialize`. A JSON
/// payload — a model-authored tool result, a child's report, a host command —
/// has no route to this type, so an effect-recording call cannot be assembled
/// from untrusted data. The only producers are [`GoalLedger::claim_task`] and
/// [`GoalLedger::hand_off_workspace`].
///
/// Deliberately NOT `Clone`: a claim is an exclusive authority, and a type that
/// hands out copies of one invites the second copy being used after the first
/// was superseded. Callers that need it in two places borrow it.
#[derive(Debug, PartialEq, Eq)]
pub struct TaskAuthority {
    goal_id: GoalId,
    task_id: TaskId,
    worker_id: String,
    epoch: u64,
}

impl TaskAuthority {
    #[must_use]
    pub fn goal_id(&self) -> &GoalId {
        &self.goal_id
    }

    #[must_use]
    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    #[must_use]
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    /// The epoch this authority was won at. Readable so a worker can stamp its
    /// own effect with it; possessing the number is not possessing the
    /// authority, because every ledger write takes the authority itself.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// What happened when a worker tried to claim a task.
///
/// A loser is TOLD it lost. Returning `Ok(None)` or a silently-unclaimed task
/// would let a worker that lost the race proceed as though it had won, which is
/// the duplicate execution this ledger exists to prevent.
#[derive(Debug)]
pub enum ClaimOutcome {
    /// This worker owns the task.
    Won(TaskAuthority),
    /// Another worker owns it, or it is not claimable. The detail says which.
    Lost { detail: String },
}

/// The durable task ledger over the 22-01 Goal kernel.
///
/// Holds no authoritative state: every read replays the chain. If a field
/// cannot be reconstructed by replay it does not exist here.
#[derive(Clone)]
pub struct GoalLedger {
    journal: SessionJournal,
}

impl GoalLedger {
    #[must_use]
    pub fn new(journal: SessionJournal) -> Self {
        Self { journal }
    }

    /// Declare a task and its dependency set.
    ///
    /// `idempotency_key` is the key the task's EFFECT is deduplicated by at the
    /// effect boundary. It is required, not optional: a task with no key has no
    /// route to an at-most-once effect when its owner dies mid-attempt, and an
    /// optional key is a key nobody sets.
    pub fn declare_task(
        &self,
        goal_id: &GoalId,
        task_id: &TaskId,
        depends_on: &BTreeSet<String>,
        idempotency_key: &str,
    ) -> Result<(), JournalError> {
        let goal = goal_id.as_str().to_owned();
        let task = task_id.as_str().to_owned();
        let depends_on = depends_on.clone();
        let idempotency_key = idempotency_key.to_owned();
        self.append(move |_state| {
            Ok(SessionEvent::GoalTaskDeclared {
                goal_id: goal.clone(),
                task_id: task.clone(),
                depends_on: depends_on.clone(),
                idempotency_key: idempotency_key.clone(),
            })
        })
    }

    /// Attempt to claim a task, returning an authority only to the winner.
    ///
    /// The epoch is DERIVED from the committed head inside the writer lock, so
    /// two workers racing cannot both compute the same successor and both win:
    /// the loser's event is built from a head that already moved, and the
    /// reducer refuses it as a non-successor.
    ///
    /// `budget_reservation_id` must name a reservation already committed
    /// through the existing budget events. A reassigned attempt re-enters that
    /// seam rather than minting a fresh budget, and the reducer enforces that
    /// the running total across a task's attempts stays inside the Goal's
    /// authorized envelope.
    pub fn claim_task(
        &self,
        goal_id: &GoalId,
        task_id: &TaskId,
        worker_id: &str,
        budget_reservation_id: &str,
        lease_expires_unix_ms: u64,
    ) -> Result<ClaimOutcome, JournalError> {
        let goal = goal_id.as_str().to_owned();
        let task = task_id.as_str().to_owned();
        let worker = worker_id.to_owned();
        let reservation = budget_reservation_id.to_owned();
        let result = self.append(move |state| {
            let task_state = require_task(state, &goal, &task)?;
            Ok(SessionEvent::GoalTaskTransitioned {
                goal_id: goal.clone(),
                task_id: task.clone(),
                transition: GoalTaskTransition::Claimed {
                    epoch: task_state.epoch().saturating_add(1),
                    worker_id: worker.clone(),
                    budget_reservation_id: reservation.clone(),
                    lease_expires_unix_ms,
                },
            })
        });
        match result {
            Ok(()) => {
                let epoch = self
                    .task(goal_id, task_id)?
                    .map_or(0, |task_state| task_state.epoch());
                Ok(ClaimOutcome::Won(TaskAuthority {
                    goal_id: goal_id.clone(),
                    task_id: task_id.clone(),
                    worker_id: worker_id.to_owned(),
                    epoch,
                }))
            }
            // A refused claim is a LOST race, not a broken ledger. The detail
            // is carried so the loser is told why rather than being handed an
            // outcome it could mistake for a win.
            Err(JournalError::InvalidTransition(detail)) => Ok(ClaimOutcome::Lost { detail }),
            Err(error) => Err(error),
        }
    }

    /// Prove the owner is still alive.
    pub fn prove_liveness(
        &self,
        authority: &TaskAuthority,
        at_unix_ms: u64,
    ) -> Result<(), JournalError> {
        self.transition(
            authority,
            GoalTaskTransition::LivenessProved {
                epoch: authority.epoch,
                at_unix_ms,
            },
        )
    }

    /// Revoke a claim under the authorized lease model.
    ///
    /// Takes no [`TaskAuthority`] because revocation is a SUPERVISOR action
    /// against an owner that may be dead — requiring the dead owner's authority
    /// to revoke its own claim would make the mechanism unusable exactly when it
    /// is needed. The safety does not come from who may revoke; it comes from
    /// the epoch the revocation supersedes, which is what refuses the old
    /// owner's late write whether or not it is still running.
    pub fn revoke_claim(
        &self,
        goal_id: &GoalId,
        task_id: &TaskId,
        reason: &str,
    ) -> Result<(), JournalError> {
        let goal = goal_id.as_str().to_owned();
        let task = task_id.as_str().to_owned();
        let reason = reason.to_owned();
        self.append(move |state| {
            let task_state = require_task(state, &goal, &task)?;
            Ok(SessionEvent::GoalTaskTransitioned {
                goal_id: goal.clone(),
                task_id: task.clone(),
                transition: GoalTaskTransition::ClaimRevoked {
                    epoch: task_state.epoch(),
                    reason: reason.clone(),
                },
            })
        })
    }

    /// Record a durable completion.
    ///
    /// Durable at the moment it is PRODUCED. Delivery to the parent is a
    /// separate transition, so a worker that finishes and dies before the parent
    /// observes it leaves the completion in the chain rather than nowhere.
    pub fn complete_task(
        &self,
        authority: &TaskAuthority,
        outcome: GoalTerminalState,
        effect_digest: &str,
    ) -> Result<(), JournalError> {
        self.transition(
            authority,
            GoalTaskTransition::Completed {
                epoch: authority.epoch,
                outcome,
                effect_digest: effect_digest.to_owned(),
            },
        )
    }

    /// Record that an attempt's outcome could not be established.
    ///
    /// The task then requires explicit resolution and is NOT claimable. That is
    /// the posture the journal already takes for started external effects, and
    /// the ledger inherits it rather than inventing a softer one: an outcome
    /// that cannot be established is never silently retried.
    pub fn record_unknown_outcome(
        &self,
        authority: &TaskAuthority,
        reason: TaskUnknownReason,
    ) -> Result<(), JournalError> {
        self.transition(
            authority,
            GoalTaskTransition::OutcomeUnknown {
                epoch: authority.epoch,
                reason,
            },
        )
    }

    /// Hand workspace ownership to a new worker through a delegated-mutation
    /// transaction that must already exist in reduced state.
    ///
    /// There is no transition that writes an owner field directly, so a handoff
    /// that bypasses the Phase 20 lifecycle is not expressible rather than
    /// merely discouraged.
    pub fn hand_off_workspace(
        &self,
        authority: &TaskAuthority,
        transaction_id: &str,
        to_worker: &str,
        budget_reservation_id: &str,
        lease_expires_unix_ms: u64,
    ) -> Result<TaskAuthority, JournalError> {
        let to_epoch = authority.epoch.saturating_add(1);
        self.transition(
            authority,
            GoalTaskTransition::WorkspaceHandedOff {
                epoch: authority.epoch,
                to_epoch,
                transaction_id: transaction_id.to_owned(),
                to_worker: to_worker.to_owned(),
                budget_reservation_id: budget_reservation_id.to_owned(),
                lease_expires_unix_ms,
            },
        )?;
        Ok(TaskAuthority {
            goal_id: authority.goal_id.clone(),
            task_id: authority.task_id.clone(),
            worker_id: to_worker.to_owned(),
            epoch: to_epoch,
        })
    }

    /// Mark a durable completion as observed by the parent.
    ///
    /// The parent's side of the outbox. Takes no worker authority because the
    /// parent is not a task owner; the epoch it must present is the one that
    /// produced the completion, and the reducer checks that.
    pub fn deliver_completion(
        &self,
        goal_id: &GoalId,
        task_id: &TaskId,
    ) -> Result<(), JournalError> {
        let goal = goal_id.as_str().to_owned();
        let task = task_id.as_str().to_owned();
        self.append(move |state| {
            let task_state = require_task(state, &goal, &task)?;
            let completion = task_state.completion.as_ref().ok_or_else(|| {
                JournalError::InvalidTransition(format!(
                    "goal {goal} task {task}: no durable completion to deliver"
                ))
            })?;
            Ok(SessionEvent::GoalTaskTransitioned {
                goal_id: goal.clone(),
                task_id: task.clone(),
                transition: GoalTaskTransition::CompletionDelivered {
                    epoch: completion.epoch,
                },
            })
        })
    }

    /// The reduced task, replayed from the chain.
    pub fn task(
        &self,
        goal_id: &GoalId,
        task_id: &TaskId,
    ) -> Result<Option<GoalTaskState>, JournalError> {
        Ok(self
            .goal(goal_id)?
            .and_then(|goal| goal.tasks.get(task_id.as_str()).cloned()))
    }

    /// The tasks a worker may claim right now.
    pub fn claimable(&self, goal_id: &GoalId) -> Result<Vec<TaskId>, JournalError> {
        Ok(self.goal(goal_id)?.map_or_else(Vec::new, |goal| {
            goal.claimable_tasks()
                .into_iter()
                .map(|task| TaskId::new(task.task_id.clone()))
                .collect()
        }))
    }

    /// Completions that are durable but not yet observed by the parent — the
    /// outbox a restarted parent drains.
    pub fn pending_deliveries(&self, goal_id: &GoalId) -> Result<Vec<TaskId>, JournalError> {
        Ok(self.goal(goal_id)?.map_or_else(Vec::new, |goal| {
            goal.tasks
                .values()
                .filter(|task| task.completion_pending_delivery())
                .map(|task| TaskId::new(task.task_id.clone()))
                .collect()
        }))
    }

    /// Tasks awaiting explicit resolution because an outcome could not be
    /// established.
    pub fn requiring_resolution(&self, goal_id: &GoalId) -> Result<Vec<TaskId>, JournalError> {
        Ok(self.goal(goal_id)?.map_or_else(Vec::new, |goal| {
            goal.tasks
                .values()
                .filter(|task| task.requires_resolution())
                .map(|task| TaskId::new(task.task_id.clone()))
                .collect()
        }))
    }

    fn goal(&self, goal_id: &GoalId) -> Result<Option<GoalState>, JournalError> {
        Ok(self.journal.state()?.goals.get(goal_id.as_str()).cloned())
    }

    fn transition(
        &self,
        authority: &TaskAuthority,
        transition: GoalTaskTransition,
    ) -> Result<(), JournalError> {
        let goal = authority.goal_id.as_str().to_owned();
        let task = authority.task_id.as_str().to_owned();
        self.append(move |_state| {
            Ok(SessionEvent::GoalTaskTransitioned {
                goal_id: goal.clone(),
                task_id: task.clone(),
                transition: transition.clone(),
            })
        })
    }

    fn append<F>(&self, build: F) -> Result<(), JournalError>
    where
        F: FnOnce(&ReducedSessionState) -> Result<SessionEvent, JournalError>,
    {
        self.journal.append_built_from_head(build).map(|_| ())
    }
}

fn require_task<'a>(
    state: &'a ReducedSessionState,
    goal_id: &str,
    task_id: &str,
) -> Result<&'a GoalTaskState, JournalError> {
    state
        .goals
        .get(goal_id)
        .and_then(|goal| goal.tasks.get(task_id))
        .ok_or_else(|| {
            JournalError::InvalidTransition(format!("unknown goal {goal_id} task {task_id}"))
        })
}
