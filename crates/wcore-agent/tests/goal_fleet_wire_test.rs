//! The WIRE: the durable task ledger as the Fleet dispatcher's source of work.
//!
//! ## What these tests are built to reach
//!
//! Every scenario here carries state, because a scenario that does not cannot
//! reach the defect. Measured on this program: a drain loop that returned a
//! per-iteration increment where its contract required total elapsed passed its
//! first live journey, because that journey had an empty queue and the loop broke
//! on its first observation.
//!
//! For a fleet dispatcher the equivalent clean scenarios are **one worker, one
//! shard, an empty queue and a zero-length history** — and in every one of them
//! this wire looks correct while being wrong. So:
//!
//! * no test here runs a single shard where sharding is the point;
//! * the shard-error test puts the failing shard FIRST, so the surviving shards
//!   are genuinely aborted mid-flight rather than merely unobserved;
//! * the reassignment tests run against a task that already has attempt history,
//!   not a fresh one;
//! * the dependency test asserts what is NOT claimable, which a
//!   count-what-completed assertion would pass while blocked work ran early.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wcore_agent::goal::{
    ClaimOutcome, GoalFleetDriver, TaskAssignment, TaskExecution, TaskExecutor,
};
use wcore_agent::session_journal::{GoalTaskAttemptStatus, SessionJournal};
use wcore_swarm::fleet::FleetDispatcher;
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalStrategy, GoalTerminalState, LoopPolicy, TaskId,
    TaskUnknownReason, resolve_goal_authority,
};

const LEASE_MS: u64 = 30_000;

/// A test double for the effect boundary that models the REAL one: an atomic
/// keyed marker, then an append. If the key is present the effect is refused —
/// which is what the shipped `goal exec-task` does with `create_new`.
#[derive(Default)]
struct EffectLog {
    keys: Mutex<BTreeSet<String>>,
    lines: Mutex<Vec<String>>,
}

impl EffectLog {
    /// Returns true when the effect was produced by THIS call.
    fn produce(&self, key: &str, label: &str) -> bool {
        let fresh = self.keys.lock().unwrap().insert(key.to_owned());
        if fresh {
            self.lines.lock().unwrap().push(label.to_owned());
        }
        fresh
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn distinct(&self) -> usize {
        self.lines().into_iter().collect::<BTreeSet<_>>().len()
    }
}

/// Runs each assignment after a per-task delay, so a shard can be made to blow
/// its timeout while its siblings are still in flight.
struct DelayedExecutor {
    effects: Arc<EffectLog>,
    delays: BTreeMap<String, Duration>,
    default_delay: Duration,
    /// Tasks whose attempt reports an outcome that could not be established.
    indeterminate: BTreeSet<String>,
    /// Tasks whose attempt fails outright.
    failing: BTreeSet<String>,
    /// Tasks whose attempt produces the effect and then reports failure.
    produce_then_fail: BTreeSet<String>,
    started: Arc<AtomicUsize>,
    /// Assignments the executor actually saw, so the epoch each agent carried
    /// can be compared against what the chain committed.
    seen: Arc<Mutex<Vec<TaskAssignment>>>,
}

impl DelayedExecutor {
    fn new(effects: Arc<EffectLog>) -> Self {
        Self {
            effects,
            delays: BTreeMap::new(),
            default_delay: Duration::ZERO,
            indeterminate: BTreeSet::new(),
            failing: BTreeSet::new(),
            produce_then_fail: BTreeSet::new(),
            started: Arc::new(AtomicUsize::new(0)),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn delay(mut self, task: &str, delay: Duration) -> Self {
        self.delays.insert(task.to_owned(), delay);
        self
    }

    fn default_delay(mut self, delay: Duration) -> Self {
        self.default_delay = delay;
        self
    }

    fn indeterminate(mut self, task: &str) -> Self {
        self.indeterminate.insert(task.to_owned());
        self
    }

    fn failing(mut self, task: &str) -> Self {
        self.failing.insert(task.to_owned());
        self
    }

    /// Produce the effect and THEN report failure — a worker that did the work
    /// and died before its outcome was recorded.
    fn produce_then_fail(mut self, task: &str) -> Self {
        self.produce_then_fail.insert(task.to_owned());
        self
    }
}

impl TaskExecutor for DelayedExecutor {
    fn execute(
        &self,
        assignment: TaskAssignment,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TaskExecution> + Send>> {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(assignment.clone());
        let effects = self.effects.clone();
        let delay = self
            .delays
            .get(&assignment.task_id)
            .copied()
            .unwrap_or(self.default_delay);
        let indeterminate = self.indeterminate.contains(&assignment.task_id);
        let failing = self.failing.contains(&assignment.task_id);
        let produce_then_fail = self.produce_then_fail.contains(&assignment.task_id);
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            if produce_then_fail {
                effects.produce(&assignment.idempotency_key, &assignment.task_id);
                return TaskExecution::Failed {
                    detail: "worker produced its effect and then died".to_owned(),
                };
            }
            if failing {
                return TaskExecution::Failed {
                    detail: "worker exited 1".to_owned(),
                };
            }
            if indeterminate {
                return TaskExecution::Indeterminate {
                    reason: TaskUnknownReason::OwnerDiedMidAttempt,
                };
            }
            effects.produce(&assignment.idempotency_key, &assignment.task_id);
            TaskExecution::Produced {
                outcome: GoalTerminalState::SelfChecked,
                effect_digest: assignment.idempotency_key.clone(),
            }
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    /// Read only by `a_second_opener_is_refused_the_writer_lease_on_unix`,
    /// which is `cfg(unix)` because the journal's writer lease is a Unix-only
    /// construction (threat T-22-06). The field is gated to match, so it
    /// exists exactly where something reads it rather than being carried as
    /// dead weight on Windows.
    #[cfg(unix)]
    journal_path: std::path::PathBuf,
    journal: SessionJournal,
    goal: GoalId,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal_path = dir.path().join(format!("{name}.journal"));
        let journal = SessionJournal::open(&journal_path, "wire-test").expect("journal opens");
        Self {
            _dir: dir,
            #[cfg(unix)]
            journal_path,
            journal,
            goal: GoalId::new(format!("g-{name}")),
        }
    }

    /// A distinct SUPERVISOR over the same journal handle.
    ///
    /// Not a second `SessionJournal::open` — the writer lease refuses that, and
    /// `a_second_opener_is_refused_the_writer_lease` pins exactly that. Two
    /// supervisors sharing one writer is the case the epoch fence still has to
    /// cover: a driver object that outlived its wave, a second wave started
    /// before the first was reaped, or any Windows build, where the lease is
    /// `cfg(unix)`-gated and gives no such protection at all.
    fn driver(&self, supervisor: &str) -> GoalFleetDriver {
        GoalFleetDriver::new(self.journal.clone(), self.goal.clone(), supervisor)
    }

    fn open(&self, driver: &GoalFleetDriver, iterations: u32) {
        let request = GoalAuthorityRequest {
            requested_limits: [("max_tokens".to_owned(), 10_000_u64)]
                .into_iter()
                .collect(),
            strategy: GoalStrategy::Fleet,
            loop_policy: LoopPolicy::Fixed { iterations },
        };
        let snapshot = resolve_goal_authority(
            &request,
            &[("max_tokens".to_owned(), 1_000_000_u64)]
                .into_iter()
                .collect(),
            "parent-v1",
        );
        driver
            .open("wire the ledger", &snapshot, 1_700_000_000_000)
            .expect("goal opens");
    }

    fn declare(&self, driver: &GoalFleetDriver, task: &str, depends_on: &[&str]) {
        let deps: BTreeSet<String> = depends_on.iter().map(|d| (*d).to_owned()).collect();
        driver
            .declare_task(&TaskId::new(task), &deps, &format!("idem-{task}"))
            .expect("task declares");
    }
}

fn task_name(index: usize) -> String {
    format!("t{index:02}")
}

// ---------------------------------------------------------------------------
// 1. Sharding, with a partial last shard, and each agent bound to its own claim.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_sharded_wave_binds_every_agent_to_its_own_committed_claim() {
    let fixture = Fixture::new("shards");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    for index in 0..12 {
        fixture.declare(&driver, &task_name(index), &[]);
    }

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let seen = executor.seen.clone();
    // 12 tasks at shard size 5 is 3 shards of [5, 5, 2] — a partial last shard,
    // which a shard size that divides the width would never produce.
    let dispatcher = FleetDispatcher::new("wire-shards")
        .with_shard_size(5)
        .with_shard_timeout(Duration::from_secs(30));

    let wave = driver
        .run_wave(&dispatcher, executor, 12, 1_000, LEASE_MS)
        .await
        .expect("wave runs");

    assert_eq!(wave.shards, 3, "expected a partial last shard: {wave:?}");
    assert_eq!(wave.claimed, 12, "{wave:?}");
    assert_eq!(wave.completed, 12, "{wave:?}");
    assert_eq!(wave.abandoned, 0, "{wave:?}");
    assert_eq!(wave.delivered.len(), 12, "{wave:?}");

    assert_eq!(effects.lines().len(), 12, "effect lines");
    assert_eq!(effects.distinct(), 12, "distinct effects");

    // Every agent carried the epoch the chain actually committed for its task.
    // A wire that handed an agent the wrong task's authority would still count
    // twelve completions; this is what separates the two.
    let goal = driver.goal_state().expect("state").expect("goal exists");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 12);
    for assignment in seen.iter() {
        let task = goal
            .tasks
            .get(&assignment.task_id)
            .unwrap_or_else(|| panic!("task {} missing", assignment.task_id));
        assert_eq!(
            task.epoch(),
            assignment.epoch,
            "task {}",
            assignment.task_id
        );
        assert_eq!(task.idempotency_key, assignment.idempotency_key);
        assert_eq!(assignment.attempt, 1);
    }
}

// ---------------------------------------------------------------------------
// 2. The one a single shard cannot reach: a shard error aborts its siblings, and
//    the ledger must still be right.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_shard_timeout_aborts_its_siblings_and_the_ledger_survives_it() {
    let fixture = Fixture::new("shardfail");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    for index in 0..4 {
        fixture.declare(&driver, &task_name(index), &[]);
    }

    let effects = Arc::new(EffectLog::default());
    // Shard size 1 puts t00 alone in shard 0. It blows the shard timeout first,
    // `FleetDispatcher` returns on that first shard error, and dropping its
    // JoinSet ABORTS shards 1..3 while their agents are still sleeping. That is
    // the abort a single-shard scenario structurally cannot produce.
    let executor = Arc::new(
        DelayedExecutor::new(effects.clone())
            .delay(&task_name(0), Duration::from_secs(30))
            .default_delay(Duration::from_secs(10)),
    );
    let dispatcher = FleetDispatcher::new("wire-shardfail")
        .with_shard_size(1)
        .with_shard_timeout(Duration::from_millis(150));

    let wave = driver
        .run_wave(&dispatcher, executor, 4, 1_000, LEASE_MS)
        .await
        .expect("the driver does NOT propagate a transport failure");

    assert!(
        wave.dispatch_error.is_some(),
        "the shard error was not recorded: {wave:?}"
    );
    assert_eq!(wave.claimed, 4, "{wave:?}");
    assert_eq!(wave.completed, 0, "{wave:?}");
    // Every agent was aborted before it could record anything, so every claim
    // this wave won is still live and accounted for as abandoned rather than
    // silently vanishing.
    assert_eq!(wave.abandoned, 4, "{wave:?}");
    assert!(
        effects.lines().is_empty(),
        "an aborted agent produced an effect"
    );

    // An abandoned claim is NOT claimable. A wire that let the next wave claim
    // straight over a live claim would run the task twice.
    assert!(
        driver
            .ledger()
            .claimable(&fixture.goal)
            .expect("claimable")
            .is_empty(),
        "a live claim was offered for reclaim without its lease expiring"
    );

    // The lease is what releases it. Recover past the lease and every task comes
    // back with its history intact and a SUCCESSOR epoch.
    let recovery = driver
        .recover("parent-v1", 1_000 + LEASE_MS + 1)
        .expect("recovery");
    assert_eq!(recovery.revoked.len(), 4, "{recovery:?}");

    let goal = driver.goal_state().expect("state").expect("goal");
    for index in 0..4 {
        let task = &goal.tasks[&task_name(index)];
        assert_eq!(task.attempts.len(), 1, "history was discarded");
        assert!(matches!(
            task.attempts[0].status,
            GoalTaskAttemptStatus::Revoked { .. }
        ));
    }

    // Second wave, this time fast: all four complete, and each effect lands ONCE
    // even though every task is on its second attempt.
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let dispatcher = FleetDispatcher::new("wire-shardfail-2")
        .with_shard_size(2)
        .with_shard_timeout(Duration::from_secs(30));
    let wave = driver
        .run_wave(&dispatcher, executor, 4, 2_000, LEASE_MS)
        .await
        .expect("second wave runs");
    assert_eq!(wave.completed, 4, "{wave:?}");
    assert_eq!(effects.lines().len(), 4, "{:?}", effects.lines());
    assert_eq!(effects.distinct(), 4, "{:?}", effects.lines());

    let goal = driver.goal_state().expect("state").expect("goal");
    for index in 0..4 {
        let task = &goal.tasks[&task_name(index)];
        assert_eq!(
            task.attempts.len(),
            2,
            "the retry did not re-enter the ledger"
        );
        assert_eq!(task.epoch(), 2);
        assert!(task.completion.is_some());
    }
}

// ---------------------------------------------------------------------------
// 3. Two supervisors on one journal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_supervisor_cannot_claim_a_task_the_first_already_holds() {
    let fixture = Fixture::new("twosup");
    let first = fixture.driver("sup-a");
    fixture.open(&first, 8);
    fixture.declare(&first, "t00", &[]);

    let second = fixture.driver("sup-b");

    // The first supervisor claims through the ledger directly so the claim is
    // held for the duration of the second's attempt, exactly as a live wave
    // would hold it.
    let held = match first
        .ledger()
        .claim_task(
            &fixture.goal,
            &TaskId::new("t00"),
            "w-a",
            &reserve(&first, "r-a"),
            99_000,
        )
        .expect("claim decides")
    {
        ClaimOutcome::Won(authority) => authority,
        ClaimOutcome::Lost { detail } => panic!("the first claim lost: {detail}"),
    };

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let dispatcher = FleetDispatcher::new("wire-twosup").with_shard_size(2);
    let wave = second
        .run_wave(&dispatcher, executor, 4, 1_000, LEASE_MS)
        .await
        .expect("second supervisor's wave runs");

    assert_eq!(
        wave.claimed, 0,
        "the second supervisor claimed a held task: {wave:?}"
    );
    assert!(effects.lines().is_empty(), "a held task was executed twice");

    // And the holder's own completion still commits, because it was never
    // superseded.
    first
        .ledger()
        .complete_task(&held, GoalTerminalState::SelfChecked, "idem-t00")
        .expect("the live owner completes");
}

// ---------------------------------------------------------------------------
// 4. The fence, at the wire.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_superseded_supervisors_completion_is_refused_after_the_lease_moves_the_task_on() {
    let fixture = Fixture::new("fence");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    fixture.declare(&driver, "t00", &[]);

    let superseded = match driver
        .ledger()
        .claim_task(
            &fixture.goal,
            &TaskId::new("t00"),
            "w-old",
            &reserve(&driver, "r-old"),
            500,
        )
        .expect("claim decides")
    {
        ClaimOutcome::Won(authority) => authority,
        ClaimOutcome::Lost { detail } => panic!("first claim lost: {detail}"),
    };

    // A fresh process finds the lease expired and reassigns. The old owner is
    // STILL ALIVE and still holding its authority — the case a timeout alone
    // cannot distinguish from a dead one.
    let successor = fixture.driver("sup-b");
    let recovery = successor.recover("parent-v1", 1_000).expect("recovery");
    assert_eq!(recovery.revoked, vec!["t00".to_owned()], "{recovery:?}");

    // Hand the task to a new owner FIRST, so the committed epoch has genuinely
    // moved. This matters: if the late write were attempted after the successor
    // had already completed, the reducer would refuse it on
    // "already carries a durable completion" and the epoch fence would never be
    // the thing under test.
    let successor_authority = match successor
        .ledger()
        .claim_task(
            &fixture.goal,
            &TaskId::new("t00"),
            "w-new",
            &reserve(&successor, "r-new"),
            99_000,
        )
        .expect("claim decides")
    {
        ClaimOutcome::Won(authority) => authority,
        ClaimOutcome::Lost { detail } => panic!("successor could not reclaim: {detail}"),
    };
    assert_eq!(successor_authority.epoch(), 2);

    // THE FENCE, isolated: the task has no completion, the successor's claim is
    // live, and the ONLY thing wrong with the old owner's write is its epoch.
    let error = driver
        .ledger()
        .complete_task(&superseded, GoalTerminalState::SelfChecked, "late-write")
        .expect_err("a superseded owner's completion was ACCEPTED");
    let detail = error.to_string();
    assert!(
        detail.contains("superseded claim epoch"),
        "refused for the wrong reason — the epoch fence is not what stopped it: {detail}"
    );

    successor
        .ledger()
        .complete_task(
            &successor_authority,
            GoalTerminalState::SelfChecked,
            "idem-t00",
        )
        .expect("the current owner completes");

    // The next wave finds nothing left to do, and no effect was produced twice.
    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let dispatcher = FleetDispatcher::new("wire-fence").with_shard_size(2);
    let wave = successor
        .run_wave(&dispatcher, executor, 4, 2_000, LEASE_MS)
        .await
        .expect("successor's wave runs");
    assert_eq!(
        wave.claimed, 0,
        "a completed task was claimed again: {wave:?}"
    );
    assert!(effects.lines().is_empty(), "{:?}", effects.lines());

    let task = driver
        .ledger()
        .task(&fixture.goal, &TaskId::new("t00"))
        .expect("read")
        .expect("task");
    assert_eq!(task.attempts.len(), 2, "{task:?}");
    assert_eq!(
        task.completion.as_ref().map(|c| c.epoch),
        Some(2),
        "the superseded epoch produced the recorded completion"
    );
}

// ---------------------------------------------------------------------------
// 5. Dependencies gate the wave, at the wire and not only in the query.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_wave_never_dispatches_a_task_whose_dependency_has_not_durably_completed() {
    let fixture = Fixture::new("deps");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    // A chain, not a fan: t02 is two hops from anything runnable, so a wire that
    // released dependents one level too early would still fail here.
    fixture.declare(&driver, "t00", &[]);
    fixture.declare(&driver, "t01", &["t00"]);
    fixture.declare(&driver, "t02", &["t01"]);
    // A dependency that FAILS, so its dependent must stay blocked forever rather
    // than being released by the attempt merely ending.
    fixture.declare(&driver, "t03", &[]);
    fixture.declare(&driver, "t04", &["t03"]);

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()).failing("t03"));
    let dispatcher = FleetDispatcher::new("wire-deps").with_shard_size(2);

    let wave = driver
        .run_wave(&dispatcher, executor, 8, 1_000, LEASE_MS)
        .await
        .expect("wave runs");
    // Only the two independent tasks were even claimable.
    assert_eq!(wave.claimed, 2, "a blocked task was claimed: {wave:?}");
    assert_eq!(wave.completed, 1, "{wave:?}");
    assert_eq!(wave.failed, 1, "{wave:?}");
    assert_eq!(effects.lines(), vec!["t00".to_owned()]);

    // t01 released by t00's DURABLE completion; t04 still blocked because t03
    // failed — an attempt ending is not a completion.
    let claimable: Vec<String> = driver
        .ledger()
        .claimable(&fixture.goal)
        .expect("claimable")
        .into_iter()
        .map(|task| task.as_str().to_owned())
        .collect();
    assert!(claimable.contains(&"t01".to_owned()), "{claimable:?}");
    assert!(
        !claimable.contains(&"t02".to_owned()),
        "t02 released early: {claimable:?}"
    );
    assert!(
        !claimable.contains(&"t04".to_owned()),
        "t04 released by a failure: {claimable:?}"
    );
    // t03 is retryable — a failure says the effect did not happen.
    assert!(claimable.contains(&"t03".to_owned()), "{claimable:?}");
}

// ---------------------------------------------------------------------------
// 6. An unestablished outcome is parked, never retried.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unestablished_outcome_parks_the_task_instead_of_retrying_it() {
    let fixture = Fixture::new("unknown");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    fixture.declare(&driver, "t00", &[]);
    fixture.declare(&driver, "t01", &[]);

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()).indeterminate("t00"));
    let dispatcher = FleetDispatcher::new("wire-unknown").with_shard_size(2);

    let wave = driver
        .run_wave(&dispatcher, executor.clone(), 4, 1_000, LEASE_MS)
        .await
        .expect("wave runs");
    assert_eq!(wave.indeterminate, 1, "{wave:?}");

    let claimable: Vec<String> = driver
        .ledger()
        .claimable(&fixture.goal)
        .expect("claimable")
        .into_iter()
        .map(|task| task.as_str().to_owned())
        .collect();
    assert!(
        !claimable.contains(&"t00".to_owned()),
        "a task with an unestablished outcome was offered for a silent retry: {claimable:?}"
    );
    assert_eq!(
        driver
            .recover("parent-v1", 9_999_999)
            .expect("recovery")
            .requiring_resolution,
        vec!["t00".to_owned()],
        "the parked task was not surfaced for resolution"
    );
}

// ---------------------------------------------------------------------------
// 7. The loop bound stops the run, at the durable boundary.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_authorized_loop_bound_stops_the_run_even_though_work_remains() {
    let fixture = Fixture::new("bound");
    let driver = fixture.driver("sup-a");
    // Two iterations authorized; a four-deep dependency chain needs four waves.
    fixture.open(&driver, 2);
    fixture.declare(&driver, "t00", &[]);
    fixture.declare(&driver, "t01", &["t00"]);
    fixture.declare(&driver, "t02", &["t01"]);
    fixture.declare(&driver, "t03", &["t02"]);

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let dispatcher = FleetDispatcher::new("wire-bound").with_shard_size(2);

    let run = driver
        .run_to_completion(&dispatcher, executor, 4, LEASE_MS, &|| 1_000)
        .await
        .expect("run completes");

    assert_eq!(run.iterations_consumed, 2, "{run:?}");
    assert_eq!(run.completed(), 2, "{run:?}");
    assert!(
        run.stopped_because.contains("iteration refused"),
        "the run stopped for the wrong reason: {}",
        run.stopped_because
    );
    // Two of four tasks ran. The bound held against real remaining work, which a
    // chain shorter than the bound could never demonstrate.
    assert_eq!(effects.lines(), vec!["t00".to_owned(), "t01".to_owned()]);
}

// ---------------------------------------------------------------------------
// 8. The whole loop, end to end, over a graph that needs several waves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_to_completion_drives_a_dependency_graph_to_a_standstill_exactly_once() {
    let fixture = Fixture::new("graph");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    for index in 0..5 {
        fixture.declare(&driver, &task_name(index), &[]);
    }
    for index in 5..10 {
        fixture.declare(&driver, &task_name(index), &[&task_name(index - 5)]);
    }

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let dispatcher = FleetDispatcher::new("wire-graph")
        .with_shard_size(3)
        .with_shard_timeout(Duration::from_secs(30));

    let run = driver
        .run_to_completion(&dispatcher, executor, 5, LEASE_MS, &|| 1_000)
        .await
        .expect("run completes");

    assert_eq!(run.completed(), 10, "{run:?}");
    assert_eq!(run.delivered(), 10, "{run:?}");
    assert_eq!(effects.lines().len(), 10, "{:?}", effects.lines());
    assert_eq!(effects.distinct(), 10, "{:?}", effects.lines());
    assert!(
        run.waves.len() >= 2,
        "a dependency graph collapsed into one wave"
    );

    let goal = driver.goal_state().expect("state").expect("goal");
    // One dependency release per dependent, counted rather than asserted.
    let releases: u64 = goal
        .tasks
        .values()
        .map(|task| task.dependency_releases)
        .sum();
    assert_eq!(releases, 10, "dependency releases");
    for task in goal.tasks.values() {
        assert!(
            task.completion.as_ref().is_some_and(|c| c.delivered),
            "{task:?}"
        );
        assert_eq!(task.attempts.len(), 1, "{task:?}");
    }
}

// ---------------------------------------------------------------------------
// 8b. A superseded agent must not revoke its successor's claim on the way out.
// ---------------------------------------------------------------------------

/// Found by reading the wire back, not by a failure: the agent's failure path
/// originally called `revoke_claim`, which reads the CURRENT epoch from the
/// committed head rather than presenting the caller's own.
///
/// The scenario that makes that wrong needs three things at once — a slow agent,
/// an expired lease, and a successor already running — and none of the other
/// tests here has all three. With any one missing, `revoke_claim` and
/// `release_claim` behave identically and the bug is invisible.
#[tokio::test]
async fn a_superseded_agents_failure_does_not_revoke_its_successors_claim() {
    let fixture = Fixture::new("release");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    fixture.declare(&driver, "t00", &[]);

    // The slow original owner, with a lease that is about to expire.
    let stale = match driver
        .ledger()
        .claim_task(
            &fixture.goal,
            &TaskId::new("t00"),
            "w-slow",
            &reserve(&driver, "r-slow"),
            500,
        )
        .expect("claim decides")
    {
        ClaimOutcome::Won(authority) => authority,
        ClaimOutcome::Lost { detail } => panic!("first claim lost: {detail}"),
    };

    // A supervisor reclaims past the expired lease and a successor takes it.
    let successor_driver = fixture.driver("sup-b");
    let recovery = successor_driver
        .recover("parent-v1", 1_000)
        .expect("recovery");
    assert_eq!(recovery.revoked, vec!["t00".to_owned()], "{recovery:?}");
    let successor = match successor_driver
        .ledger()
        .claim_task(
            &fixture.goal,
            &TaskId::new("t00"),
            "w-fresh",
            &reserve(&successor_driver, "r-fresh"),
            99_000,
        )
        .expect("claim decides")
    {
        ClaimOutcome::Won(authority) => authority,
        ClaimOutcome::Lost { detail } => panic!("successor could not claim: {detail}"),
    };
    assert_eq!(successor.epoch(), 2);

    // NOW the stale owner finally fails and tries to give its claim back.
    let refused = driver
        .ledger()
        .release_claim(&stale, "attempt failed: worker exited 1")
        .expect_err("a superseded owner released a claim it no longer held");
    assert!(
        refused.to_string().contains("superseded claim epoch"),
        "refused for the wrong reason: {refused}"
    );

    // The successor's claim is untouched and still live — it was NOT handed back
    // to the pool while a healthy worker was still running it.
    let task = driver
        .ledger()
        .task(&fixture.goal, &TaskId::new("t00"))
        .expect("read")
        .expect("task");
    assert_eq!(task.epoch(), 2);
    assert!(
        task.live_attempt().is_some(),
        "the successor's live claim was revoked by a superseded predecessor: {task:?}"
    );
    assert!(
        driver
            .ledger()
            .claimable(&fixture.goal)
            .expect("claimable")
            .is_empty(),
        "the task was offered for reclaim while a live successor held it"
    );

    // And the successor can still complete normally.
    successor_driver
        .ledger()
        .complete_task(&successor, GoalTerminalState::SelfChecked, "idem-t00")
        .expect("the live successor completes");
}

// ---------------------------------------------------------------------------
// 9. The half the epoch fence structurally cannot reach.
// ---------------------------------------------------------------------------

/// A worker that PRODUCED its effect and then failed to have it recorded is the
/// case the whole idempotency key exists for, and it is the one every clean
/// scenario misses: a task that never ran, or ran and was recorded, both look
/// correct with no key at all.
///
/// Here t00's executor produces the effect and *then* reports failure, so the
/// first attempt leaves the effect on disk with no completion — exactly the
/// state a kill leaves behind. The retry must find the key and not write again.
#[tokio::test]
async fn a_retry_whose_predecessor_already_produced_the_effect_does_not_produce_it_twice() {
    let fixture = Fixture::new("idem");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    fixture.declare(&driver, "t00", &[]);
    fixture.declare(&driver, "t01", &[]);

    let effects = Arc::new(EffectLog::default());
    // Produces, then fails. The ledger revokes the claim; the effect stays.
    let executor = Arc::new(DelayedExecutor::new(effects.clone()).produce_then_fail("t00"));
    let dispatcher = FleetDispatcher::new("wire-idem").with_shard_size(2);

    let wave = driver
        .run_wave(&dispatcher, executor, 4, 1_000, LEASE_MS)
        .await
        .expect("first wave runs");
    assert_eq!(wave.failed, 1, "{wave:?}");
    assert_eq!(wave.completed, 1, "{wave:?}");
    // The effect IS on disk, with no completion recorded for it.
    assert_eq!(effects.lines().len(), 2, "{:?}", effects.lines());
    let t00 = driver
        .ledger()
        .task(&fixture.goal, &TaskId::new("t00"))
        .expect("read")
        .expect("task");
    assert!(
        t00.completion.is_none(),
        "a failed attempt recorded a completion"
    );

    // Second wave retries t00. Its idempotency key is unchanged across attempts,
    // which is what stops the effect landing a second time. A key that embedded
    // the attempt number would pass every other assertion in this file and fail
    // exactly here.
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let seen = executor.seen.clone();
    let dispatcher = FleetDispatcher::new("wire-idem-2").with_shard_size(2);
    let wave = driver
        .run_wave(&dispatcher, executor, 4, 2_000, LEASE_MS)
        .await
        .expect("second wave runs");
    assert_eq!(wave.completed, 1, "{wave:?}");

    let retried = seen.lock().unwrap().clone();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].task_id, "t00");
    assert_eq!(
        retried[0].attempt, 2,
        "the retry did not present as a second attempt"
    );
    assert_eq!(
        retried[0].idempotency_key, "idem-t00",
        "the retry was handed a DIFFERENT key, so the effect would land twice"
    );

    let lines = effects.lines();
    assert_eq!(lines.len(), 2, "the effect landed twice: {lines:?}");
    assert_eq!(effects.distinct(), 2, "{lines:?}");
}

// ---------------------------------------------------------------------------
// 10. What actually stops two SUPERVISOR PROCESSES, and where it does not.
// ---------------------------------------------------------------------------

/// The first line of defence against two supervisors is not the epoch — it is
/// the journal's writer lease, and it is `cfg(unix)`-gated.
///
/// This test exists to pin which mechanism is doing the work, because the two
/// answers have very different Windows consequences and the phase's own verdict
/// already records the lease as a Unix-only construction (threat T-22-06). If
/// this ever passes on Windows too, that gap has closed and this test should be
/// un-gated rather than left as folklore.
#[cfg(unix)]
#[tokio::test]
async fn a_second_opener_is_refused_the_writer_lease_on_unix() {
    let fixture = Fixture::new("lease");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    fixture.declare(&driver, "t00", &[]);

    // Matched rather than `expect_err`, because the Ok arm holds a live writer
    // handle and unwrapping it for a panic message would be a second opener in
    // its own right.
    let Err(error) = SessionJournal::open(&fixture.journal_path, "wire-test") else {
        panic!("a second process opened the journal while a supervisor held it");
    };
    assert!(
        error.to_string().contains("AlreadyOwned") || format!("{error:?}").contains("AlreadyOwned"),
        "refused for the wrong reason: {error:?}"
    );
}

/// Commit a budget reservation the way the driver does, for the two tests that
/// need to hold a claim outside a wave.
fn reserve(driver: &GoalFleetDriver, id: &str) -> String {
    use wcore_agent::durable_child::DurableChildStore;
    use wcore_agent::session_journal::{
        BudgetAmount, BudgetOwner, BudgetPurpose, BudgetUnit, SessionEvent,
    };
    use wcore_types::spawner::{
        ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent,
        ChildPolicySnapshot, ChildRecoveryState, ChildRequestEvidence, ChildTimestamps,
        ChildWorkspace, ChildWorkspaceMode, DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord,
        DurableChildStatus,
    };
    let journal = driver.journal_handle();
    let filled = |c: char, n: usize| -> String { std::iter::repeat_n(c, n).collect() };
    DurableChildStore::new(journal.clone())
        .declare(DurableChildRecord {
            schema_version: DURABLE_CHILD_SCHEMA_VERSION,
            declaration_id: format!("declare-{id}"),
            child_id: ChildId::new(id).expect("child id"),
            parent: ChildParent {
                session_id: "wire-test".into(),
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
                workspace_id: format!("workspace-{id}"),
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
        .expect("child declares");
    journal
        .append(SessionEvent::BudgetReserved {
            event_id: format!("evt-{id}"),
            reservation_id: id.to_owned(),
            owner: BudgetOwner::Child {
                child_id: id.to_owned(),
            },
            purpose: BudgetPurpose::ChildExecution,
            amount: BudgetAmount {
                value: 1,
                unit: BudgetUnit::Tokens,
            },
        })
        .expect("reservation commits");
    id.to_owned()
}

// ---------------------------------------------------------------------------
// 9. #946 B-01: an empty claimable set is not a finished Goal.
// ---------------------------------------------------------------------------

/// A resume INSIDE the claim-lease window must not borrow the finished Goal's
/// words.
///
/// The measured defect: a process killed mid-wave leaves every task it claimed
/// under a live, unexpired lease. `recover` revokes only EXPIRED claims — by
/// design, because revoking a live one is how a task gets run twice — so a
/// restart within the lease window finds `claimable()` empty and reported
/// `stopped_because=no claimable task remains`, the exact sentence a Goal that
/// is genuinely done produces, with exit 0 behind it.
///
/// The abandoned-claim state is reached the same way
/// `a_shard_timeout_aborts_its_siblings_and_the_ledger_survives_it` reaches it,
/// so the premise is a state the wire really produces, not one hand-written
/// into the journal.
///
/// CONTROL (`census_of_a_finished_goal_keeps_the_historical_sentence`, below)
/// proves the new wording is not simply always emitted.
#[tokio::test]
async fn a_resume_inside_the_lease_window_does_not_report_the_finished_sentence() {
    let fixture = Fixture::new("leasewindow");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    for index in 0..4 {
        fixture.declare(&driver, &task_name(index), &[]);
    }

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(
        DelayedExecutor::new(effects.clone())
            .delay(&task_name(0), Duration::from_secs(30))
            .default_delay(Duration::from_secs(10)),
    );
    let dispatcher = FleetDispatcher::new("wire-leasewindow")
        .with_shard_size(1)
        .with_shard_timeout(Duration::from_millis(150));

    // The kill. Four claims won at t=1_000, every agent aborted before it could
    // record anything, so all four leases run to 1_000 + LEASE_MS.
    let wave = driver
        .run_wave(&dispatcher, executor.clone(), 4, 1_000, LEASE_MS)
        .await
        .expect("wave returns");
    assert_eq!(wave.abandoned, 4, "premise not reached: {wave:?}");

    // The resume, by a DIFFERENT supervisor, still inside the lease window.
    let resumed = fixture.driver("sup-b");
    let run = resumed
        .run_to_completion(&dispatcher, executor, 4, LEASE_MS, &|| 1_500)
        .await
        .expect("run returns");

    assert!(
        run.waves.is_empty(),
        "nothing should have been claimed: {run:?}"
    );
    let census = run.idle.expect("the idle exit must carry a census");
    assert_eq!(census.lease_held, 4, "{census:?}");
    assert_eq!(census.awaiting_resolution, 0, "{census:?}");
    assert_eq!(census.dependency_blocked, 0, "{census:?}");
    assert!(!census.is_finished(), "{census:?}");
    assert_eq!(
        census.earliest_lease_expiry_unix_ms,
        Some(1_000 + LEASE_MS),
        "the sentence must be able to say when the work comes back"
    );

    // THE DEFECT, stated as an assertion: this run must not describe itself
    // with the finished Goal's sentence.
    assert!(
        !run.stopped_because.contains("no claimable task remains"),
        "a resume inside the lease window reported the FINISHED sentence over \
         four unfinished tasks: {}",
        run.stopped_because
    );
    assert!(
        run.stopped_because.contains("leased to another worker")
            && run
                .stopped_because
                .contains(&(1_000 + LEASE_MS).to_string()),
        "the sentence must name the lease and when it expires: {}",
        run.stopped_because
    );
}

/// The CONTROL. A Goal that really is finished keeps the historical sentence
/// byte for byte, so the assertion above is discriminating rather than a
/// tautology about the new wording.
#[tokio::test]
async fn census_of_a_finished_goal_keeps_the_historical_sentence() {
    let fixture = Fixture::new("leasecontrol");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver, 8);
    for index in 0..3 {
        fixture.declare(&driver, &task_name(index), &[]);
    }

    let effects = Arc::new(EffectLog::default());
    let executor = Arc::new(DelayedExecutor::new(effects.clone()));
    let dispatcher = FleetDispatcher::new("wire-leasecontrol").with_shard_size(2);

    let run = driver
        .run_to_completion(&dispatcher, executor, 3, LEASE_MS, &|| 1_000)
        .await
        .expect("run completes");

    assert_eq!(run.completed(), 3, "{run:?}");
    let census = run.idle.expect("the idle exit must carry a census");
    assert!(census.is_finished(), "{census:?}");
    assert_eq!(
        run.stopped_because, "no claimable task remains",
        "a finished goal must keep the historical wording"
    );
}
