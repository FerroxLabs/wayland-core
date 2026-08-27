//! The wire: the durable task ledger becomes the Fleet dispatcher's source of
//! work (F22-03, Success Criterion 2).
//!
//! ## What this closes, and why it is here rather than in `wcore-swarm`
//!
//! Before this module the ledger existed and was fenced but nothing consumed
//! it: `FleetDispatcher` still took a caller-supplied `Vec<MeshAgent>` with no
//! durable record behind it, and no user-reachable path could reach either. A
//! fence nothing routes through manufactures exactly the confidence the claim
//! model warns about, so the ledger is only worth what its wire is worth.
//!
//! The wire lives in `wcore-agent` because it needs both halves and this is the
//! lowest crate that has them: [`GoalLedger`] is here, and `wcore-agent` already
//! depends on `wcore-swarm`. The reverse edge does not exist and must not: see
//! the note on `TaskAuthority` below.
//!
//! ## Why the parent owns every ledger write
//!
//! This is a structural consequence of the swarm sandbox, not a preference. A
//! swarm worker runs under a manifest whose `fs_write_allow` is exactly its own
//! checkout and scratch, with `network = Deny` (`wcore-swarm/src/dispatch.rs`,
//! `worker_manifest`). A worker process therefore *cannot* write the shared
//! session journal, so a pull model in which each worker claims its own task is
//! not expressible through that boundary — and should not be, because it would
//! require handing a worker the authority to write the parent's chain.
//!
//! So the parent claims, and each claim's [`TaskAuthority`] stays in the parent
//! process for its whole life. It is never serialized, never sent to a child and
//! never reconstructed from one — which is what makes "no `Deserialize`" a real
//! property rather than a decoration.
//!
//! ## Why the completion is recorded INSIDE the agent, and the outbox is drained
//! from the ledger
//!
//! Both dispatchers under this driver can lose an `AgentReport`, and neither
//! loss is a bug in them:
//!
//! * [`MeshDispatcher::dispatch`] wraps its `JoinSet` in a `tokio::time::timeout`.
//!   On expiry it returns `Err(Timeout)` and drops the set — which aborts every
//!   in-flight agent AND discards every report already collected in that wave.
//! * [`FleetDispatcher::dispatch`] returns on the first shard error, dropping its
//!   own `JoinSet` and aborting the surviving shards.
//!
//! A driver that learned its outcomes from the dispatcher's return value would
//! therefore lose completions whose work genuinely happened. So the agent closure
//! records the completion **durably, through the fenced API, before it returns its
//! report**, and the parent afterwards drains [`GoalLedger::pending_deliveries`]
//! — a replay of the chain, not a read of anything the dispatcher handed back.
//! The `AgentReport` is then pure telemetry and dropping it costs nothing.
//!
//! This is the property that makes the wave loop restart-safe for the same
//! reason a kill is survivable: there is exactly one source of truth and it is
//! on disk.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use wcore_swarm::fleet::{FleetDispatcher, FleetReducer, ShardSummary};
use wcore_swarm::mesh::{AgentReport, BlackboardCtx, MeshAgent};
use wcore_types::goal::{
    GoalAuthoritySnapshot, GoalId, GoalTerminalState, TaskId, TaskUnknownReason,
};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus,
};

use crate::durable_child::DurableChildStore;
use crate::session_journal::{
    BudgetAmount, BudgetOwner, BudgetPurpose, BudgetUnit, GoalTaskAttemptStatus, JournalError,
    SessionEvent, SessionJournal,
};

use super::kernel::{GoalKernel, GoalRecovery};
use super::ledger::{ClaimOutcome, GoalLedger, TaskAuthority};

/// A single task handed to an executor, with the epoch it is authorized at.
///
/// Carries the epoch as a NUMBER, deliberately: an executor may stamp its effect
/// with it, but possessing the number is not possessing the authority, because
/// every ledger write takes `&TaskAuthority` and the driver never lets one out of
/// the parent process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAssignment {
    pub goal_id: String,
    pub task_id: String,
    /// The key the effect is deduplicated by at the effect boundary. The ledger
    /// fences who may RECORD a completion; this is what stops the effect itself
    /// landing twice when an attempt is legitimately retried after its owner died.
    pub idempotency_key: String,
    pub worker_id: String,
    pub epoch: u64,
    /// Which attempt of this task this is, 1-based. Non-1 means a predecessor
    /// died or was superseded, which is exactly when the idempotency key earns
    /// its place.
    pub attempt: usize,
    /// The dispatcher-assigned agent identity, so a shard's telemetry can be
    /// matched to a task.
    pub agent_id: String,
}

/// What an executor established about one attempt.
///
/// `Failed` and `Indeterminate` are kept apart for the reason the ledger's own
/// attempt status keeps them apart: a failure says the effect did not happen and
/// the task may be retried; an indeterminate outcome says the driver cannot tell,
/// and the task is parked for explicit resolution rather than silently retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskExecution {
    Produced {
        outcome: GoalTerminalState,
        effect_digest: String,
    },
    Failed {
        detail: String,
    },
    Indeterminate {
        reason: TaskUnknownReason,
    },
}

/// Runs one assigned task. Implemented by the CLI over a real child process.
pub trait TaskExecutor: Send + Sync + 'static {
    fn execute(
        &self,
        assignment: TaskAssignment,
    ) -> Pin<Box<dyn Future<Output = TaskExecution> + Send>>;
}

/// What a fresh process found and reclaimed before it dispatched anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetRecovery {
    /// The kernel's verdict, rendered for the operator surface.
    pub goal_recovery: String,
    /// Whether the Goal is resumable at all.
    pub resumable: bool,
    /// Claims revoked because their lease had expired.
    pub revoked: Vec<String>,
    /// Completions that were durable before the crash and are only now observed.
    pub delivered_from_outbox: Vec<String>,
    /// Tasks parked for explicit resolution; the driver will not retry these.
    pub requiring_resolution: Vec<String>,
}

/// The result of one dispatched wave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaveOutcome {
    pub claimed: usize,
    pub lost_claims: usize,
    pub completed: usize,
    pub failed: usize,
    pub indeterminate: usize,
    /// Claims this wave won whose agent never returned an outcome — aborted by a
    /// dispatcher timeout or a sibling shard's error. The claim is still live and
    /// its lease is what releases it.
    pub abandoned: usize,
    pub shards: usize,
    /// The dispatcher's own error, if it returned one. Recorded rather than
    /// propagated: completions this wave produced are already durable, and
    /// throwing away the wave because its transport failed is precisely the
    /// lost-completion this driver exists to avoid.
    pub dispatch_error: Option<String>,
    /// Completions delivered to the parent after the wave, drained from the
    /// LEDGER rather than from any dispatcher return value.
    pub delivered: Vec<String>,
}

/// The result of driving a Goal to a standstill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FleetRun {
    pub waves: Vec<WaveOutcome>,
    pub iterations_consumed: u32,
    /// Why the loop stopped. Always populated.
    pub stopped_because: String,
    /// The ledger census taken at the moment the loop found nothing claimable.
    ///
    /// `Some` ONLY on that exit. The loop-bound and no-progress exits leave it
    /// `None` on purpose: they did not consult the ledger, so a census there
    /// would be a fabricated reading of a Goal nobody looked at.
    pub idle: Option<IdleCensus>,
}

impl FleetRun {
    #[must_use]
    pub fn completed(&self) -> usize {
        self.waves.iter().map(|wave| wave.completed).sum()
    }

    #[must_use]
    pub fn delivered(&self) -> usize {
        self.waves.iter().map(|wave| wave.delivered.len()).sum()
    }
}

/// Why a Goal had nothing claimable, when it had nothing claimable.
///
/// `claimable().is_empty()` used to be reported as `no claimable task remains`
/// in every case, which is true of exactly one of them: the Goal is finished.
/// #946 B-01 measured another. A process killed mid-wave leaves its tasks under
/// LIVE, UNEXPIRED leases — `GoalTaskState::live_attempt` takes no clock, and
/// [`GoalFleetDriver::recover`] revokes only claims whose lease has already
/// expired, deliberately, because revoking a live lease is precisely how a task
/// gets run twice. So a resume INSIDE the lease window found nothing claimable
/// and printed `run_complete … stopped_because=no claimable task remains` over
/// an unfinished job, with exit 0 behind it.
///
/// This census is the missing half. It does not change what is claimable and
/// takes no lease away from anyone; it reports the shape of what is left so the
/// sentence, and the caller's `$?`, can tell "done" from "not mine yet".
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IdleCensus {
    /// Incomplete tasks held by a live claim — another worker's, or a dead
    /// process's whose lease has not run out yet.
    pub lease_held: usize,
    /// Incomplete tasks parked for an operator decision (`--resolve`).
    pub awaiting_resolution: usize,
    /// Incomplete tasks whose dependencies are not all durably complete.
    pub dependency_blocked: usize,
    /// Earliest lease expiry among the `lease_held` tasks, so the sentence can
    /// say WHEN the work becomes reclaimable instead of only that it is not.
    pub earliest_lease_expiry_unix_ms: Option<u64>,
}

impl IdleCensus {
    /// Walk the reduced Goal once, counting the incomplete tasks by why they
    /// are not claimable. A task is counted at most once, in the order a worker
    /// would meet the refusals: resolution first (it outranks everything), then
    /// a live lease, then unmet dependencies.
    #[must_use]
    pub fn of(goal: &super::GoalState) -> Self {
        let mut census = Self::default();
        for task in goal.tasks.values() {
            if task.completion.is_some() {
                continue;
            }
            if task.requires_resolution() {
                census.awaiting_resolution += 1;
            } else if let Some(attempt) = task.live_attempt() {
                census.lease_held += 1;
                census.earliest_lease_expiry_unix_ms = Some(
                    census
                        .earliest_lease_expiry_unix_ms
                        .map_or(attempt.lease_expires_unix_ms, |seen| {
                            seen.min(attempt.lease_expires_unix_ms)
                        }),
                );
            } else if !goal.dependencies_met(task) {
                census.dependency_blocked += 1;
            }
        }
        census
    }

    /// True when the Goal really has nothing left — the one reading the old
    /// sentence was right about.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.lease_held == 0 && self.awaiting_resolution == 0 && self.dependency_blocked == 0
    }

    /// The `stopped_because` sentence for this census.
    ///
    /// The finished case is BYTE-IDENTICAL to the historical wording: operators
    /// and tests read it, and it was never wrong for a Goal that was done.
    #[must_use]
    pub fn stopped_because(&self) -> String {
        if self.is_finished() {
            return "no claimable task remains".to_owned();
        }
        let mut parts = Vec::new();
        if self.lease_held > 0 {
            parts.push(match self.earliest_lease_expiry_unix_ms {
                Some(expiry) => format!(
                    "{} task(s) leased to another worker (earliest lease expires at \
                     {expiry} unix-ms; re-run after that to reclaim)",
                    self.lease_held
                ),
                None => format!("{} task(s) leased to another worker", self.lease_held),
            });
        }
        if self.awaiting_resolution > 0 {
            parts.push(format!(
                "{} task(s) awaiting `goal exec-task --resolve produced|retry`",
                self.awaiting_resolution
            ));
        }
        if self.dependency_blocked > 0 {
            parts.push(format!(
                "{} task(s) blocked on unmet dependencies",
                self.dependency_blocked
            ));
        }
        format!(
            "nothing is claimable YET and the goal is NOT finished: {}",
            parts.join("; ")
        )
    }
}

/// Drives a durable Goal's task ledger through the real Fleet dispatcher.
#[derive(Clone)]
pub struct GoalFleetDriver {
    journal: SessionJournal,
    ledger: GoalLedger,
    kernel: GoalKernel,
    goal: GoalId,
    supervisor_id: String,
    /// Monotonic within this process, so two attempts in one wave cannot collide
    /// on a reservation identity. Uniqueness ACROSS processes comes from the
    /// journal: the reducer refuses a duplicate reservation id outright, so a
    /// collision is a refused claim rather than a silently shared budget.
    reservation_seq: Arc<AtomicU64>,
}

impl GoalFleetDriver {
    #[must_use]
    pub fn new(journal: SessionJournal, goal: GoalId, supervisor_id: impl Into<String>) -> Self {
        Self {
            ledger: GoalLedger::new(journal.clone()),
            kernel: GoalKernel::new(journal.clone()),
            journal,
            goal,
            supervisor_id: supervisor_id.into(),
            reservation_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn ledger(&self) -> &GoalLedger {
        &self.ledger
    }

    #[must_use]
    pub fn kernel(&self) -> &GoalKernel {
        &self.kernel
    }

    #[must_use]
    pub fn goal_id(&self) -> &GoalId {
        &self.goal
    }

    /// The journal handle this driver appends through.
    ///
    /// Exposed so a caller that must commit a reservation outside a wave uses
    /// the SAME handle — and therefore the same writer lock — rather than
    /// opening a second one beside it.
    #[must_use]
    pub fn journal_handle(&self) -> SessionJournal {
        self.journal.clone()
    }

    /// Authorize the Goal. Thin passthrough so a surface needs one handle.
    pub fn open(
        &self,
        objective: &str,
        snapshot: &GoalAuthoritySnapshot,
        opened_at_unix_ms: u64,
    ) -> Result<(), JournalError> {
        self.kernel
            .open_goal(&self.goal, objective, snapshot, opened_at_unix_ms)
            .map(|_| ())
    }

    /// Declare a task and its dependency set.
    pub fn declare_task(
        &self,
        task_id: &TaskId,
        depends_on: &std::collections::BTreeSet<String>,
        idempotency_key: &str,
    ) -> Result<(), JournalError> {
        self.ledger
            .declare_task(&self.goal, task_id, depends_on, idempotency_key)
    }

    /// Pick the Goal up in a fresh process: reconstruct the envelope, revoke
    /// every claim whose lease has expired, and drain whatever the dead parent
    /// never observed.
    ///
    /// Revocation is deliberately NOT conditioned on the owner being dead. A
    /// claim held by a dead process is indistinguishable from one held by a slow
    /// process, which is the whole reason the model is lease-plus-epoch: the
    /// lease decides when to reassign, and the epoch is what refuses the old
    /// owner's write if it turns out to have been merely slow.
    pub fn recover(
        &self,
        parent_envelope_digest: &str,
        now_unix_ms: u64,
    ) -> Result<FleetRecovery, JournalError> {
        let recovery = self
            .kernel
            .recover_with_parent_envelope(&self.goal, parent_envelope_digest)?;
        let resumable = matches!(recovery, GoalRecovery::Resumed { .. });
        let goal_recovery = match &recovery {
            GoalRecovery::Resumed {
                iterations_started,
                resume_count,
                ..
            } => format!("resumed iterations={iterations_started} resume_count={resume_count}"),
            GoalRecovery::AlreadyTerminal { terminal } => format!("already-terminal {terminal:?}"),
            GoalRecovery::Blocked { terminal } => format!("blocked {terminal:?}"),
        };

        let mut revoked = Vec::new();
        if resumable {
            let expired: Vec<(String, u64)> = self
                .goal_state()?
                .map(|goal| {
                    goal.tasks
                        .values()
                        .filter_map(|task| {
                            task.live_attempt().map(|attempt| {
                                (task.task_id.clone(), attempt.lease_expires_unix_ms)
                            })
                        })
                        .filter(|(_, expires)| *expires <= now_unix_ms)
                        .collect()
                })
                .unwrap_or_default();
            for (task_id, expires) in expired {
                self.ledger.revoke_claim(
                    &self.goal,
                    &TaskId::new(task_id.clone()),
                    &format!("lease expired at {expires} (now {now_unix_ms})"),
                )?;
                revoked.push(task_id);
            }
        }

        let delivered_from_outbox = if resumable {
            self.drain_outbox()?
        } else {
            Vec::new()
        };
        let requiring_resolution = self
            .ledger
            .requiring_resolution(&self.goal)?
            .into_iter()
            .map(|task| task.as_str().to_owned())
            .collect();

        Ok(FleetRecovery {
            goal_recovery,
            resumable,
            revoked,
            delivered_from_outbox,
            requiring_resolution,
        })
    }

    /// Mark every durable-but-unobserved completion as delivered.
    ///
    /// Reads [`GoalLedger::pending_deliveries`], which replays the chain. It does
    /// NOT consult any dispatcher return value, which is what makes an aborted
    /// or discarded `AgentReport` cost nothing.
    pub fn drain_outbox(&self) -> Result<Vec<String>, JournalError> {
        let pending = self.ledger.pending_deliveries(&self.goal)?;
        let mut delivered = Vec::with_capacity(pending.len());
        for task in pending {
            self.ledger.deliver_completion(&self.goal, &task)?;
            delivered.push(task.as_str().to_owned());
        }
        Ok(delivered)
    }

    /// Dispatch one wave: claim up to `width` claimable tasks, run them through
    /// the real [`FleetDispatcher`], then drain the outbox.
    ///
    /// Each claimed task becomes exactly one `MeshAgent` whose closure OWNS its
    /// `TaskAuthority`. That binding is what makes the fence expressible at this
    /// layer: an agent cannot record an effect for a task it did not win, because
    /// it does not hold the only type that admits the call.
    pub async fn run_wave(
        &self,
        dispatcher: &FleetDispatcher,
        executor: Arc<dyn TaskExecutor>,
        width: usize,
        now_unix_ms: u64,
        lease_ms: u64,
    ) -> Result<WaveOutcome, JournalError> {
        let mut outcome = WaveOutcome::default();

        let claimable = self.ledger.claimable(&self.goal)?;
        let mut agents: Vec<MeshAgent> = Vec::new();
        // (task id, epoch this wave won it at) — the exact rows the wave is
        // accountable for. Read back from the chain afterwards, never inferred
        // from the dispatcher's report.
        let mut wave_rows: Vec<(String, u64)> = Vec::new();
        for task in claimable.into_iter().take(width.max(1)) {
            let Some(assignment_seed) = self.task_seed(&task)? else {
                continue;
            };
            let reservation = self.reserve_attempt(&task)?;
            let worker_id = format!("{}-w{}", self.supervisor_id, agents.len());
            let authority = match self.ledger.claim_task(
                &self.goal,
                &task,
                &worker_id,
                &reservation,
                now_unix_ms.saturating_add(lease_ms),
            )? {
                ClaimOutcome::Won(authority) => authority,
                ClaimOutcome::Lost { detail } => {
                    tracing::info!(task = %task, %detail, "fleet claim lost");
                    outcome.lost_claims += 1;
                    continue;
                }
            };
            outcome.claimed += 1;
            wave_rows.push((task.as_str().to_owned(), authority.epoch()));
            agents.push(self.agent_for(authority, assignment_seed, executor.clone()));
        }

        if agents.is_empty() {
            outcome.delivered = self.drain_outbox()?;
            return Ok(outcome);
        }

        let reducer: FleetReducer<Vec<ShardSummary>> = Box::new(|summaries| summaries);
        match dispatcher.dispatch(agents, None, reducer).await {
            Ok(summaries) => outcome.shards = summaries.len(),
            Err(error) => {
                // Recorded, not propagated. Every completion this wave produced
                // is already on disk; discarding the wave because its transport
                // failed would lose exactly what the ledger exists to keep.
                tracing::warn!(%error, "fleet dispatch returned an error; ledger state is authoritative");
                outcome.dispatch_error = Some(error.to_string());
            }
        }

        // The ledger, not the dispatcher, is asked what happened — and only about
        // the exact (task, epoch) rows this wave claimed, so a concurrent
        // supervisor's work is never counted as ours.
        if let Some(goal) = self.goal_state()? {
            for (task_id, epoch) in &wave_rows {
                let Some(attempt) = goal
                    .tasks
                    .get(task_id)
                    .and_then(|task| task.attempts.iter().find(|a| a.epoch == *epoch))
                else {
                    continue;
                };
                match attempt.status {
                    GoalTaskAttemptStatus::Completed => outcome.completed += 1,
                    GoalTaskAttemptStatus::Unknown { .. } => outcome.indeterminate += 1,
                    GoalTaskAttemptStatus::Revoked { .. } => outcome.failed += 1,
                    // Still live means the agent never returned — it was aborted
                    // by a dispatcher timeout or a shard error. The claim stays
                    // held until its lease expires, and the next process revokes
                    // and reassigns it. That is the designed path, not a leak.
                    GoalTaskAttemptStatus::Live => outcome.abandoned += 1,
                }
            }
        }

        outcome.delivered = self.drain_outbox()?;
        Ok(outcome)
    }

    /// Drive the Goal until nothing is claimable or its authorized loop bound is
    /// consumed.
    ///
    /// Every wave consumes exactly one Goal iteration through
    /// [`GoalKernel::start_iteration`], so the bound is enforced at the DURABLE
    /// boundary by the reducer rather than by this loop remembering to count.
    /// A driver that counted its own iterations would lose the count on the kill
    /// this whole phase is about.
    pub async fn run_to_completion(
        &self,
        dispatcher: &FleetDispatcher,
        executor: Arc<dyn TaskExecutor>,
        width: usize,
        lease_ms: u64,
        clock: &dyn Fn() -> u64,
    ) -> Result<FleetRun, JournalError> {
        let mut run = FleetRun::default();
        loop {
            if self.ledger.claimable(&self.goal)?.is_empty() {
                // #946 B-01: an empty claimable set is not the same fact as a
                // finished Goal. Census the ledger and say which one this is.
                let census = self
                    .goal_state()?
                    .as_ref()
                    .map_or_else(IdleCensus::default, IdleCensus::of);
                run.stopped_because = census.stopped_because();
                run.idle = Some(census);
                break;
            }
            if let Err(error) = self.kernel.start_iteration(&self.goal) {
                // The loop bound is refused at the durable boundary. That is a
                // stop condition, not a driver fault.
                run.stopped_because = format!("iteration refused: {error}");
                break;
            }
            run.iterations_consumed = run.iterations_consumed.saturating_add(1);
            let wave = self
                .run_wave(dispatcher, executor.clone(), width, clock(), lease_ms)
                .await?;
            let progressed = wave.completed > 0 || wave.failed > 0 || wave.indeterminate > 0;
            run.waves.push(wave);
            if !progressed {
                run.stopped_because =
                    "a wave made no progress; stopping rather than spinning".to_owned();
                break;
            }
        }
        if run.stopped_because.is_empty() {
            run.stopped_because = "loop exited without a recorded reason".to_owned();
        }
        Ok(run)
    }

    /// The reduced Goal, replayed.
    pub fn goal_state(&self) -> Result<Option<super::GoalState>, JournalError> {
        self.kernel.goal(&self.goal)
    }

    /// Build the one agent that owns `authority`.
    fn agent_for(
        &self,
        authority: TaskAuthority,
        seed: AssignmentSeed,
        executor: Arc<dyn TaskExecutor>,
    ) -> MeshAgent {
        let ledger = self.ledger.clone();
        Box::new(move |ctx: BlackboardCtx| {
            Box::pin(async move {
                let assignment = TaskAssignment {
                    goal_id: authority.goal_id().as_str().to_owned(),
                    task_id: authority.task_id().as_str().to_owned(),
                    idempotency_key: seed.idempotency_key,
                    worker_id: authority.worker_id().to_owned(),
                    epoch: authority.epoch(),
                    attempt: seed.attempt,
                    agent_id: ctx.agent_id.clone(),
                };
                let task_id = assignment.task_id.clone();
                let execution = executor.execute(assignment).await;

                // Recorded HERE, inside the agent, before the report is
                // returned. Both dispatchers above this point can drop the
                // report; neither can drop what is already in the chain.
                let recorded = match &execution {
                    TaskExecution::Produced {
                        outcome,
                        effect_digest,
                    } => ledger.complete_task(&authority, outcome.clone(), effect_digest),
                    TaskExecution::Indeterminate { reason } => {
                        ledger.record_unknown_outcome(&authority, reason.clone())
                    }
                    // `release_claim`, NOT `revoke_claim`. The agent is giving
                    // up its OWN claim and must present its own epoch, so a
                    // superseded agent is refused. `revoke_claim` reads the
                    // committed epoch from the head and would revoke whatever
                    // claim is current — meaning a slow agent whose lease had
                    // expired would revoke its SUCCESSOR's live claim on the way
                    // out, handing the task back to the pool while a healthy
                    // worker was still running it.
                    TaskExecution::Failed { detail } => {
                        ledger.release_claim(&authority, &format!("attempt failed: {detail}"))
                    }
                };
                let succeeded =
                    recorded.is_ok() && matches!(execution, TaskExecution::Produced { .. });
                if let Err(error) = &recorded {
                    tracing::warn!(task = %task_id, %error, "ledger refused an attempt's outcome");
                }
                AgentReport {
                    agent_id: ctx.agent_id,
                    payload: serde_json::json!({
                        "task_id": task_id,
                        "epoch": authority.epoch(),
                        "recorded": recorded.is_ok(),
                    }),
                    succeeded,
                }
            })
        })
    }

    /// Read the immutable facts a claim needs before it is made.
    fn task_seed(&self, task: &TaskId) -> Result<Option<AssignmentSeed>, JournalError> {
        Ok(self.ledger.task(&self.goal, task)?.map(|state| {
            AssignmentSeed {
                idempotency_key: state.idempotency_key.clone(),
                // The attempt about to be made, 1-based.
                attempt: state.attempts.len().saturating_add(1),
            }
        }))
    }

    /// Commit a budget reservation through the EXISTING seam.
    ///
    /// A reassigned attempt re-enters the durable-child and budget events rather
    /// than the ledger minting a budget of its own; the reducer then enforces
    /// that the running total across a task's attempts stays inside the Goal's
    /// authorized envelope. That check is what stops N attempts costing N
    /// unbounded reservations.
    fn reserve_attempt(&self, task: &TaskId) -> Result<String, JournalError> {
        let seq = self.reservation_seq.fetch_add(1, Ordering::SeqCst);
        let reservation = format!("goalfleet-{}-{}-{seq}", self.supervisor_id, task.as_str());
        // The child's parent session must be the JOURNAL's session, not this
        // driver's supervisor identity. The two are different things and the
        // reducer refuses a child whose parent session does not match the
        // journal's authority — correctly, since a child claiming a foreign
        // parent is a lineage forgery.
        let record = child_record(&reservation, &self.journal.session_id()?).map_err(|error| {
            JournalError::InvalidTransition(format!(
                "attempt reservation identity {reservation} is not a valid child id: {error}"
            ))
        })?;
        DurableChildStore::new(self.journal.clone()).declare(record)?;
        self.journal.append(SessionEvent::BudgetReserved {
            event_id: format!("evt-{reservation}"),
            reservation_id: reservation.clone(),
            owner: BudgetOwner::Child {
                child_id: reservation.clone(),
            },
            purpose: BudgetPurpose::ChildExecution,
            amount: BudgetAmount {
                value: 1,
                unit: BudgetUnit::Tokens,
            },
        })?;
        Ok(reservation)
    }
}

/// The facts read from the committed task before its claim is attempted.
struct AssignmentSeed {
    idempotency_key: String,
    attempt: usize,
}

fn filled(character: char, len: usize) -> String {
    std::iter::repeat_n(character, len).collect()
}

/// Build the durable child a task attempt's budget is charged to.
///
/// Fallible rather than panicking: the reservation identity embeds a task id the
/// operator chose, so an id this crate did not mint can fail validation. A
/// refused identity must surface as a refused claim, never as a crash in a
/// supervisor that is holding live claims.
fn child_record(
    child_id: &str,
    session_id: &str,
) -> Result<DurableChildRecord, wcore_types::spawner::DurableChildError> {
    Ok(DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: format!("declare-{child_id}"),
        child_id: ChildId::new(child_id)?,
        parent: ChildParent {
            session_id: session_id.to_owned(),
            turn_id: None,
            parent_child_id: None,
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: None,
        },
        origin: ChildOrigin::Delegate,
        request: ChildRequestEvidence::redacted(filled('a', 64)),
        policy_snapshot: ChildPolicySnapshot {
            contract_version: "effective-execution-policy/v1".into(),
            exact_digest: filled('b', 64),
            posture: "smart".into(),
            approvals: "on_request".into(),
            sandbox: "required".into(),
            source: "session-effective-policy".into(),
            managed_floor_active: true,
            dangerous_activation_id_digest: None,
        },
        provider: None,
        model: None,
        workspace: ChildWorkspace {
            mode: ChildWorkspaceMode::Isolated,
            workspace_id: format!("workspace-{child_id}"),
        },
        status: DurableChildStatus::Prepared,
        desired_state: ChildDesiredState::Run,
        recovery: ChildRecoveryState::Clean,
        revision: 0,
        timestamps: ChildTimestamps {
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
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
    })
}
