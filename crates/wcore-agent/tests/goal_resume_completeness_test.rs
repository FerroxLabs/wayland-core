//! Resume honesty: a Goal that still has work must never be reported the same
//! way as a Goal that finished.
//!
//! ## The defect these are built to reach
//!
//! Driving the shipped binary: a 12-step job was `SIGKILL`ed mid-wave and
//! restarted immediately — the obvious thing a person does. The restart printed
//! `run_complete ... stopped_because=no claimable task remains` and exited 0
//! while a third of the work was undone. Nothing was claimable because every
//! remaining task was still held by the DEAD parent's claim, whose lease had not
//! yet expired.
//!
//! "Everything is done" and "everything is leased to a process that no longer
//! exists" are opposite facts and the driver reported them with the same bytes.
//!
//! ## Why the scenarios are shaped like this
//!
//! Both halves must be present in ONE test, because the defect is an equality
//! between two outputs — a test that only looked at the stalled run would pass
//! against any wording, and a test that only looked at the finished run could
//! never see the lie. So the finished Goal and the crashed Goal are driven
//! through the SAME loop with the same width, lease and dispatcher shape, and
//! the only difference is whether a previous owner died holding the claims.
//!
//! The crash is produced by the real abort path — a shard timeout drops the
//! `JoinSet` and aborts every agent — rather than by hand-writing a `Claimed`
//! event, so the claims under test are ones the product itself left behind.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wcore_agent::goal::{
    GoalFleetDriver, TaskAssignment, TaskExecution, TaskExecutor, UnfinishedReason, UnfinishedTask,
};
use wcore_agent::session_journal::SessionJournal;
use wcore_swarm::fleet::FleetDispatcher;
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalStrategy, GoalTerminalState, LoopPolicy, TaskId,
    TaskUnknownReason, resolve_goal_authority,
};

const LEASE_MS: u64 = 30_000;
const NOW: u64 = 1_000;
const PARENT_ENVELOPE: &str = "parent-v1";

/// Never returns an outcome. Stands in for a worker whose parent is killed
/// mid-attempt: the claim is in the chain and nothing ever settles it.
struct HangingExecutor;

impl TaskExecutor for HangingExecutor {
    fn execute(
        &self,
        _assignment: TaskAssignment,
    ) -> Pin<Box<dyn Future<Output = TaskExecution> + Send>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            TaskExecution::Failed {
                detail: "an aborted agent must never reach this".to_owned(),
            }
        })
    }
}

/// Completes every assignment it is handed, except the tasks named
/// `indeterminate`, whose outcome it declares unestablishable.
#[derive(Default)]
struct CompletingExecutor {
    seen: Arc<Mutex<Vec<String>>>,
    indeterminate: BTreeSet<String>,
}

impl CompletingExecutor {
    fn indeterminate(task: &str) -> Self {
        Self {
            seen: Arc::default(),
            indeterminate: [task.to_owned()].into_iter().collect(),
        }
    }
}

impl TaskExecutor for CompletingExecutor {
    fn execute(
        &self,
        assignment: TaskAssignment,
    ) -> Pin<Box<dyn Future<Output = TaskExecution> + Send>> {
        let seen = self.seen.clone();
        let indeterminate = self.indeterminate.contains(&assignment.task_id);
        Box::pin(async move {
            seen.lock().unwrap().push(assignment.task_id.clone());
            if indeterminate {
                return TaskExecution::Indeterminate {
                    reason: TaskUnknownReason::OwnerDiedMidAttempt,
                };
            }
            TaskExecution::Produced {
                outcome: GoalTerminalState::SelfChecked,
                effect_digest: assignment.idempotency_key.clone(),
            }
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    journal: SessionJournal,
    goal: GoalId,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal =
            SessionJournal::open(dir.path().join(format!("{name}.journal")), "resume-test")
                .expect("journal opens");
        Self {
            _dir: dir,
            journal,
            goal: GoalId::new(format!("g-{name}")),
        }
    }

    /// A distinct SUPERVISOR over the same journal handle — a restarted process
    /// in every respect the ledger can observe.
    fn driver(&self, supervisor: &str) -> GoalFleetDriver {
        GoalFleetDriver::new(self.journal.clone(), self.goal.clone(), supervisor)
    }

    fn open(&self, driver: &GoalFleetDriver) {
        let request = GoalAuthorityRequest {
            requested_limits: [("max_tokens".to_owned(), 10_000_u64)]
                .into_iter()
                .collect(),
            strategy: GoalStrategy::Fleet,
            loop_policy: LoopPolicy::Fixed { iterations: 8 },
        };
        let snapshot = resolve_goal_authority(
            &request,
            &[("max_tokens".to_owned(), 1_000_000_u64)]
                .into_iter()
                .collect(),
            PARENT_ENVELOPE,
        );
        driver
            .open("resume honestly", &snapshot, 1_700_000_000_000)
            .expect("goal opens");
    }

    fn declare(&self, driver: &GoalFleetDriver, task: &str) {
        self.declare_after(driver, task, &[]);
    }

    fn declare_after(&self, driver: &GoalFleetDriver, task: &str, depends_on: &[&str]) {
        let deps: BTreeSet<String> = depends_on.iter().map(|d| (*d).to_owned()).collect();
        driver
            .declare_task(&TaskId::new(task), &deps, &format!("idem-{task}"))
            .expect("task declares");
    }
}

/// Shard size 1 with a 150ms shard timeout: the first shard blows its timeout,
/// `FleetDispatcher` returns on that error and dropping its `JoinSet` aborts the
/// siblings mid-flight. Every claim the wave won survives, held and unsettled.
fn aborting_dispatcher(name: &str) -> FleetDispatcher {
    FleetDispatcher::new(name)
        .with_shard_size(1)
        .with_shard_timeout(Duration::from_millis(150))
}

fn healthy_dispatcher(name: &str) -> FleetDispatcher {
    FleetDispatcher::new(name)
        .with_shard_size(1)
        .with_shard_timeout(Duration::from_secs(30))
}

#[tokio::test]
async fn a_resume_inside_the_lease_window_is_not_reported_as_a_finished_goal() {
    // ── Half one: a Goal that genuinely finished everything. ─────────────────
    let done = Fixture::new("done");
    let owner = done.driver("sup-done");
    done.open(&owner);
    done.declare(&owner, "t00");
    done.declare(&owner, "t01");

    let finished = owner
        .run_to_completion(
            &healthy_dispatcher("goal-done"),
            Arc::new(CompletingExecutor::default()),
            4,
            LEASE_MS,
            &|| NOW,
        )
        .await
        .expect("the finished run drives to a standstill");
    assert_eq!(finished.completed(), 2, "{finished:?}");

    // ── Half two: a Goal whose owner was killed holding every claim. ─────────
    let crashed = Fixture::new("crashed");
    let dead = crashed.driver("sup-dead");
    crashed.open(&dead);
    crashed.declare(&dead, "t00");
    crashed.declare(&dead, "t01");

    let wave = dead
        .run_wave(
            &aborting_dispatcher("goal-crashed"),
            Arc::new(HangingExecutor),
            4,
            NOW,
            LEASE_MS,
        )
        .await
        .expect("the driver does not propagate a transport failure");
    assert_eq!(wave.claimed, 2, "{wave:?}");
    assert_eq!(
        wave.abandoned, 2,
        "both claims must outlive the dead owner: {wave:?}"
    );

    // The restart, INSIDE the lease window. This is the obvious thing a real
    // person does after a crash, and it is the case the driver gets wrong.
    let restarted = crashed.driver("sup-restarted");
    let recovery = restarted
        .recover(PARENT_ENVELOPE, NOW + 1)
        .expect("the Goal recovers");
    assert!(recovery.resumable, "{recovery:?}");
    assert!(
        recovery.revoked.is_empty(),
        "no lease has expired yet, so nothing may be reclaimed: {recovery:?}"
    );

    let stalled = restarted
        .run_to_completion(
            &healthy_dispatcher("goal-restarted"),
            Arc::new(CompletingExecutor::default()),
            4,
            LEASE_MS,
            &|| NOW + 1,
        )
        .await
        .expect("the resumed run returns");

    // Ground truth: the resumed run accomplished nothing, and the chain agrees.
    assert_eq!(stalled.completed(), 0, "{stalled:?}");
    let goal = restarted.goal_state().expect("state").expect("goal exists");
    assert_eq!(
        goal.tasks
            .values()
            .filter(|task| task.completion.is_some())
            .count(),
        0,
        "no task carries a durable completion"
    );

    // ── The defect. ──────────────────────────────────────────────────────────
    // 2-of-2 done and 0-of-2 done are opposite facts. Reporting them with the
    // same bytes is the data-completeness lie: the operator is told the job is
    // finished while every task is still held by a process that no longer runs.
    assert_ne!(
        finished.stopped_because, stalled.stopped_because,
        "a Goal with 0 of 2 tasks done reports the SAME stop reason as one with \
         2 of 2 done: {:?}",
        stalled.stopped_because
    );

    // And the honest reason must name what is actually outstanding.
    assert!(
        stalled.stopped_because.contains("t00") && stalled.stopped_because.contains("t01"),
        "the stop reason does not name the unfinished tasks: {}",
        stalled.stopped_because
    );
}

/// The closed set of reasons, all four in one Goal.
///
/// Each reason on its own is reachable by a one-task Goal, and a one-task Goal
/// is exactly the scenario in which a tally that lost the reason still looks
/// right. So this drives all four at once and pins the whole list, in order —
/// a tally that dropped a task, ordered non-deterministically, or collapsed two
/// reasons into one fails here rather than passing on a count.
#[tokio::test]
async fn the_tally_names_every_outstanding_task_and_what_is_holding_it() {
    let fixture = Fixture::new("reasons");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver);

    // t1-hung alone first, so the aborting wave can leave exactly it leased.
    fixture.declare(&driver, "t1-hung");
    let wave = driver
        .run_wave(
            &aborting_dispatcher("goal-reasons-abort"),
            Arc::new(HangingExecutor),
            4,
            NOW,
            LEASE_MS,
        )
        .await
        .expect("the aborting wave returns");
    assert_eq!(wave.abandoned, 1, "{wave:?}");

    fixture.declare_after(&driver, "t2-blocked", &["t1-hung"]);
    fixture.declare(&driver, "t3-unknown");
    fixture.declare(&driver, "t4-done");

    let wave = driver
        .run_wave(
            &healthy_dispatcher("goal-reasons"),
            Arc::new(CompletingExecutor::indeterminate("t3-unknown")),
            4,
            NOW,
            LEASE_MS,
        )
        .await
        .expect("the healthy wave returns");
    assert_eq!(wave.completed, 1, "{wave:?}");
    assert_eq!(wave.indeterminate, 1, "{wave:?}");

    let tally = driver.tally().expect("the chain is tallied");
    assert_eq!(tally.declared, 4, "{tally:?}");
    assert_eq!(tally.completed, 1, "{tally:?}");
    assert!(!tally.is_complete(), "{tally:?}");
    assert_eq!(
        tally.unfinished,
        vec![
            UnfinishedTask {
                task_id: "t1-hung".to_owned(),
                reason: UnfinishedReason::Leased {
                    worker_id: "sup-a-w0".to_owned(),
                    lease_expires_unix_ms: NOW + LEASE_MS,
                },
            },
            UnfinishedTask {
                task_id: "t2-blocked".to_owned(),
                reason: UnfinishedReason::DependenciesUnmet,
            },
            UnfinishedTask {
                task_id: "t3-unknown".to_owned(),
                reason: UnfinishedReason::AwaitingResolution,
            },
        ],
        "{tally:?}"
    );

    // The operator-facing sentence must answer "when can I re-run this?" — a raw
    // unix millisecond does not, and the wait is the whole point of the window.
    let summary = tally.summary(NOW + 5_000);
    assert!(
        summary.contains("3 of 4 tasks unfinished"),
        "summary: {summary}"
    );
    assert!(
        summary.contains("reclaimable in 25s"),
        "the summary does not say how long the lease has left: {summary}"
    );
}

/// A Goal that really did finish says so, and says it about the GOAL rather than
/// about the claim pool.
#[tokio::test]
async fn a_finished_goal_reports_its_own_completeness_not_an_empty_claim_pool() {
    let fixture = Fixture::new("finished");
    let driver = fixture.driver("sup-a");
    fixture.open(&driver);
    fixture.declare(&driver, "t00");
    fixture.declare_after(&driver, "t01", &["t00"]);

    let run = driver
        .run_to_completion(
            &healthy_dispatcher("goal-finished"),
            Arc::new(CompletingExecutor::default()),
            4,
            LEASE_MS,
            &|| NOW,
        )
        .await
        .expect("the run drives to a standstill");

    assert!(run.goal_complete(), "{run:?}");
    assert_eq!(run.tally.declared, 2, "{run:?}");
    assert_eq!(run.tally.completed, 2, "{run:?}");
    assert_eq!(
        run.stopped_because, "all 2 declared tasks carry a durable completion",
        "a finished Goal must report the GOAL, not the claim pool"
    );
}
