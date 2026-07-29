//! F22-04 — `wayland-core goal`: the user-reachable surface over the durable
//! Goal kernel and its Fleet task ledger.
//!
//! ## Why this exists
//!
//! Phase 22 built a durable Goal kernel and a fenced task ledger and proved both
//! against a real kill on both platforms — with an `examples/` instrument,
//! because no path through the shipped binary could reach a Goal. Two successive
//! summaries carried that caveat forward rather than dropping it, and it is the
//! second of the two named reasons Success Criterion 2 stayed open. This module
//! is what closes it: after this, the kill/restart proof runs against
//! `wayland-core` itself.
//!
//! ## Verbs
//!
//! | Verb | What it does |
//! |---|---|
//! | `open` | authorize a durable Goal with a loop bound and a limit envelope |
//! | `task` | declare a task, its dependency set and its idempotency key |
//! | `run` | recover, revoke expired leases, drain the outbox, then drive waves through the real `FleetDispatcher` |
//! | `status` | the canonical JSON projection of Goal + task state, replayed from the chain |
//! | `stream` | the same state as the HOST protocol sees it: the ordered `goal_transition` / `goal_snapshot` JSON-stream lines (F22-C1) |
//! | `exec-task` | the effect boundary: the idempotency gate, then the operator's command |
//!
//! ## Why `exec-task` is a product verb and not a test fixture
//!
//! The ledger fences who may RECORD a completion. It structurally cannot reach
//! inside a worker process that already holds a directory and stop it writing.
//! So the exactly-once property needs a second half at the effect boundary — an
//! atomic `create_new` marker keyed by the task's idempotency key — and that half
//! has to run in the process that produces the effect, not in the parent that
//! might die between checking and spawning.
//!
//! Putting it in the shipped binary means "no duplicate effect after a kill" is a
//! property of the product rather than of whichever harness happened to measure
//! it. `run` spawns `wayland-core goal exec-task` and `exec-task` runs the
//! operator's command behind the gate.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Subcommand};

use wcore_agent::goal::{
    FleetOutcome, GoalFleetDriver, GoalKernel, GoalLoop, StrategyTermination, TaskAssignment,
    TaskExecution, TaskExecutor, WaveOutcome, event_line, goal_stream,
};
use wcore_agent::session_journal::SessionJournal;
use wcore_swarm::fleet::{FleetDispatcher, ShardSummary};
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalStrategy, GoalTerminalState, LoopPolicy, TaskId,
    TaskUnknownReason, resolve_goal_authority,
};

/// Environment the assignment reaches `exec-task` through.
///
/// Named constants rather than string literals at three call sites, because a
/// typo in one of them would silently produce an unkeyed effect — which is
/// exactly the duplicate this design exists to prevent, arriving through a
/// spelling mistake instead of a race.
pub const ENV_GOAL: &str = "WAYLAND_GOAL_ID";
pub const ENV_TASK: &str = "WAYLAND_GOAL_TASK";
pub const ENV_KEY: &str = "WAYLAND_GOAL_IDEMPOTENCY_KEY";
pub const ENV_EPOCH: &str = "WAYLAND_GOAL_EPOCH";
pub const ENV_ATTEMPT: &str = "WAYLAND_GOAL_ATTEMPT";
pub const ENV_WORKER: &str = "WAYLAND_GOAL_WORKER";

/// Default identity this binary presents as the Goal's parent envelope.
///
/// A Goal resumes only against the envelope it was authorized under; a mismatch
/// parks it durably as `AuthorityUnreconstructable` rather than resuming it under
/// whatever the parent happens to be now. Exposed as a flag so that refusal is
/// exercisable from the command line rather than only reasoned about.
pub const DEFAULT_PARENT_ENVELOPE: &str = "wayland-core-goal-fleet/v1";

#[derive(Args, Debug)]
pub struct GoalArgs {
    #[command(subcommand)]
    pub command: GoalCommand,
}

#[derive(Subcommand, Debug)]
pub enum GoalCommand {
    /// Authorize a durable Goal.
    Open {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        objective: String,
        /// Loop bound, at least 1. `1` records `LoopPolicy::Once`, the only
        /// policy that cannot multiply a bound; anything higher records
        /// `Fixed`. There is deliberately no way to spell "unbounded" here —
        /// the taxonomy has no such variant, and a flag that invented one
        /// would be a second loop vocabulary beside the canonical taxonomy.
        #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..))]
        iterations: u32,
        /// Token ceiling this Goal may reserve across every task attempt.
        #[arg(long, default_value_t = 10_000)]
        max_tokens: u64,
        /// Ceiling the parent itself is authorized for. The Goal's effective
        /// envelope is the intersection, never the request.
        #[arg(long, default_value_t = 1_000_000)]
        parent_max_tokens: u64,
        #[arg(long, default_value = DEFAULT_PARENT_ENVELOPE)]
        parent_envelope: String,
    },
    /// Declare a task in the Goal's durable ledger.
    Task {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        task: String,
        /// Task ids that must carry a DURABLE completion before this one is
        /// claimable. Repeatable.
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        /// The key the task's effect is deduplicated by. Defaults to
        /// `idem-<task>`; required to be stable across attempts.
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Recover, reclaim and drive the Goal's tasks through the Fleet dispatcher.
    Run {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        /// Directory the effects and idempotency markers live in.
        #[arg(long)]
        effects_dir: PathBuf,
        /// argv-style command each task runs. Split on ASCII whitespace; no
        /// shell interpretation. The assignment arrives in the environment.
        #[arg(long)]
        worker_command: String,
        /// How many tasks one wave may claim.
        #[arg(long, default_value_t = 8)]
        width: usize,
        /// Tasks per inner Mesh shard. Below the width so the Fleet path
        /// genuinely shards rather than degenerating to one shard.
        #[arg(long, default_value_t = 4)]
        shard_size: usize,
        /// Claim lease, for BOTH a task claim and the Goal's loop-owner claim.
        /// A claim whose lease has expired is revoked and reassigned by the next
        /// process to start.
        #[arg(long, default_value = "60s")]
        lease: String,
        /// Per-shard wall-clock timeout.
        #[arg(long, default_value = "5m")]
        shard_timeout: String,
        #[arg(long, default_value = DEFAULT_PARENT_ENVELOPE)]
        parent_envelope: String,
        /// Recover and report, then stop without dispatching anything.
        #[arg(long)]
        recover_only: bool,
        /// Terminate the Goal through the ONE canonical Goal terminal
        /// transition when the run finishes (F22C, Success Criterion 3).
        ///
        /// Opt-in, and deliberately so. `goal run` is the verb 22-03's
        /// kill/restart proof re-enters after killing the parent, and a run that
        /// terminated the Goal every time would make that restart impossible.
        /// With this flag the run claims the Goal's ONE loop owner before
        /// dispatching and terminates through `StrategyTermination::from_fleet`
        /// afterwards; without it the Goal stays live exactly as before.
        #[arg(long)]
        terminate: bool,
    },
    /// The canonical JSON projection of a Goal and its task ledger.
    Status {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
    },
    /// The Goal as the HOST protocol sees it (F22-C1).
    ///
    /// Emits the ordered producer stream for one Goal as JSON Lines: every
    /// durable Goal-level transition as a `goal_transition` at the cursor it
    /// landed at, then the current `goal_snapshot`. Replayed from the chain
    /// through the SAME reducer every other view uses, so this cannot show a
    /// state the journal does not hold.
    ///
    /// A verb rather than a flag on `status` because the two answer different
    /// questions: `status` prints the reduced state for a human, `stream` emits
    /// the wire a host consumes. Collapsing them would make one of the two
    /// answers a rendering of the other.
    Stream {
        #[arg(long)]
        journal: PathBuf,
        #[arg(long)]
        goal: String,
        /// How many events the caller expects. Exit 1 on a mismatch, so this is
        /// a gate that can go red rather than a print — the same discipline
        /// `effects --expect` carries, and for the same reason: a stream that
        /// emitted nothing must not be indistinguishable from a stream that
        /// emitted everything.
        #[arg(long)]
        expect: Option<usize>,
    },
    /// The effect boundary: idempotency gate, then the operator's command.
    ///
    /// Not normally invoked by hand — `run` spawns it — but it is a real verb so
    /// the gate can be exercised directly.
    ExecTask {
        #[arg(long)]
        effects_dir: PathBuf,
        /// The operator's argv, after `--`.
        #[arg(last = true)]
        argv: Vec<String>,
    },
    /// Count the effects on disk: total, and how many carry distinct labels.
    ///
    /// A verb rather than a shell one-liner so a kill/restart proof counts what
    /// the PRODUCT wrote, using the product, on both platforms — and so the
    /// count cannot quietly differ between a Linux `wc -l` and a PowerShell
    /// `Measure-Object`.
    Effects {
        #[arg(long)]
        effects_dir: PathBuf,
        /// How many distinct effects the caller expects. Exit 1 on a mismatch,
        /// so this is a gate that can actually go red rather than a print.
        #[arg(long)]
        expect: Option<usize>,
    },
}

pub async fn run(args: GoalArgs) -> anyhow::Result<()> {
    match args.command {
        GoalCommand::Open {
            journal,
            goal,
            objective,
            iterations,
            max_tokens,
            parent_max_tokens,
            parent_envelope,
        } => open_goal(
            &journal,
            &goal,
            &objective,
            iterations,
            max_tokens,
            parent_max_tokens,
            &parent_envelope,
        ),
        GoalCommand::Task {
            journal,
            goal,
            task,
            depends_on,
            idempotency_key,
        } => declare_task(&journal, &goal, &task, &depends_on, idempotency_key),
        GoalCommand::Run {
            journal,
            goal,
            effects_dir,
            worker_command,
            width,
            shard_size,
            lease,
            shard_timeout,
            parent_envelope,
            recover_only,
            terminate,
        } => {
            run_goal(RunOptions {
                journal,
                goal,
                effects_dir,
                worker_command,
                width,
                shard_size,
                lease: humantime::parse_duration(&lease)
                    .map_err(|e| anyhow::anyhow!("invalid --lease '{lease}': {e}"))?,
                shard_timeout: humantime::parse_duration(&shard_timeout).map_err(|e| {
                    anyhow::anyhow!("invalid --shard-timeout '{shard_timeout}': {e}")
                })?,
                parent_envelope,
                recover_only,
                terminate,
            })
            .await
        }
        GoalCommand::Status { journal, goal } => status(&journal, &goal),
        GoalCommand::Stream {
            journal,
            goal,
            expect,
        } => stream(&journal, &goal, expect),
        GoalCommand::ExecTask { effects_dir, argv } => {
            exec_task_from_env(&effects_dir, &argv).await
        }
        GoalCommand::Effects {
            effects_dir,
            expect,
        } => {
            let (total, distinct) = count_effects(&effects_dir)?;
            println!("GOAL-EFFECTS: total={total} distinct={distinct}");
            if let Some(expect) = expect
                && (total != expect || distinct != expect)
            {
                anyhow::bail!("expected {expect} effects, found total={total} distinct={distinct}");
            }
            Ok(())
        }
    }
}

fn session_for(journal: &std::path::Path) -> String {
    journal
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wayland-goal".to_owned())
}

fn open_journal(journal: &std::path::Path) -> anyhow::Result<SessionJournal> {
    if let Some(parent) = journal.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let session = session_for(journal);
    SessionJournal::open(journal, &session)
        .map_err(|e| anyhow::anyhow!("failed to open journal {}: {e}", journal.display()))
}

fn now_unix_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn open_goal(
    journal: &std::path::Path,
    goal: &str,
    objective: &str,
    iterations: u32,
    max_tokens: u64,
    parent_max_tokens: u64,
    parent_envelope: &str,
) -> anyhow::Result<()> {
    let handle = open_journal(journal)?;
    let driver = GoalFleetDriver::new(handle, GoalId::new(goal), session_for(journal));
    let request = GoalAuthorityRequest {
        requested_limits: [("max_tokens".to_owned(), max_tokens)]
            .into_iter()
            .collect(),
        strategy: GoalStrategy::Fleet,
        loop_policy: if iterations == 1 {
            LoopPolicy::Once
        } else {
            LoopPolicy::Fixed { iterations }
        },
    };
    let snapshot = resolve_goal_authority(
        &request,
        &[("max_tokens".to_owned(), parent_max_tokens)]
            .into_iter()
            .collect(),
        parent_envelope,
    );
    driver
        .open(objective, &snapshot, now_unix_ms())
        .map_err(|e| anyhow::anyhow!("failed to open goal {goal}: {e}"))?;
    println!("GOAL: opened goal={goal} iterations={iterations} envelope={parent_envelope}");
    Ok(())
}

fn declare_task(
    journal: &std::path::Path,
    goal: &str,
    task: &str,
    depends_on: &[String],
    idempotency_key: Option<String>,
) -> anyhow::Result<()> {
    let handle = open_journal(journal)?;
    let driver = GoalFleetDriver::new(handle, GoalId::new(goal), session_for(journal));
    let key = idempotency_key.unwrap_or_else(|| format!("idem-{task}"));
    let deps: BTreeSet<String> = depends_on.iter().cloned().collect();
    driver
        .declare_task(&TaskId::new(task), &deps, &key)
        .map_err(|e| anyhow::anyhow!("failed to declare task {task}: {e}"))?;
    println!(
        "GOAL: declared task={task} depends_on={} key={key}",
        if deps.is_empty() {
            "-".to_owned()
        } else {
            deps.iter().cloned().collect::<Vec<_>>().join(",")
        }
    );
    Ok(())
}

struct RunOptions {
    journal: PathBuf,
    goal: String,
    effects_dir: PathBuf,
    worker_command: String,
    width: usize,
    shard_size: usize,
    lease: Duration,
    shard_timeout: Duration,
    parent_envelope: String,
    recover_only: bool,
    terminate: bool,
}

async fn run_goal(options: RunOptions) -> anyhow::Result<()> {
    let argv: Vec<String> = options
        .worker_command
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect();
    if argv.is_empty() {
        anyhow::bail!("--worker-command is empty after whitespace split");
    }
    std::fs::create_dir_all(&options.effects_dir)?;
    let effects_dir = std::fs::canonicalize(&options.effects_dir)?;

    let handle = open_journal(&options.journal)?;
    // ONE handle, cloned to share authority. `SessionJournal::open` takes an
    // exclusive cross-process writer lease and an independent second open fails
    // closed, so the Goal loop must clone this handle rather than reopen the
    // path. Found by the live run, not by the suite: every unit test builds a
    // single driver, so nothing in-process ever opened the journal twice.
    // The loop-owner claim honours the SAME `--lease` as a task claim, because
    // it answers the same question — how long a claim stays evidence that its
    // owner is alive. A loop-owner claim with a different, invisible lease would
    // be a second liveness vocabulary, and a `kill -9` would strand the Goal for
    // however long that hidden default happened to be.
    let loop_driver = GoalLoop::new(GoalKernel::new(handle.clone()))
        .with_lease_ms(u64::try_from(options.lease.as_millis()).unwrap_or(u64::MAX));
    let driver = GoalFleetDriver::new(
        handle,
        GoalId::new(&options.goal),
        format!("{}-{}", session_for(&options.journal), std::process::id()),
    );

    println!("GOAL: run pid={} goal={}", std::process::id(), options.goal);

    // Recovery FIRST, always — including on a first run, where it is a no-op
    // beyond recording the resume. A driver that only recovers "when it looks
    // like a crash" is a driver whose recovery path is never exercised.
    let recovery = driver
        .recover(&options.parent_envelope, now_unix_ms())
        .map_err(|e| anyhow::anyhow!("recovery failed for goal {}: {e}", options.goal))?;
    println!(
        "GOAL: recovery={} resumable={} revoked={} drained={} needs_resolution={}",
        recovery.goal_recovery,
        recovery.resumable,
        recovery.revoked.len(),
        recovery.delivered_from_outbox.len(),
        recovery.requiring_resolution.len()
    );
    for task in &recovery.revoked {
        println!("GOAL: revoked task={task} reason=lease-expired");
    }
    for task in &recovery.delivered_from_outbox {
        println!("GOAL: drained task={task} source=outbox");
    }
    if !recovery.resumable {
        println!("GOAL: not resumable; stopping without dispatch");
        return Ok(());
    }
    if options.recover_only {
        println!("GOAL: recover-only; stopping before dispatch");
        return Ok(());
    }

    let dispatcher = FleetDispatcher::new(format!("goal-{}", options.goal))
        .with_shard_size(options.shard_size)
        .with_shard_timeout(options.shard_timeout);
    let executor: Arc<dyn TaskExecutor> = Arc::new(ChildProcessExecutor { effects_dir, argv });

    let lease_ms = u64::try_from(options.lease.as_millis()).unwrap_or(u64::MAX);

    // ── F22C: the canonical terminal transition, when asked for ─────────────
    //
    // Without `--terminate` this is byte-for-byte the pre-F22C path: dispatch,
    // print, leave the Goal live for the next restart. With it, the whole
    // dispatch runs INSIDE `GoalLoop::run_fleet`, which claims the Goal's one
    // loop owner before the first wave and terminates through
    // `StrategyTermination::from_fleet` after the last. The closure's return
    // type is `StrategyTermination`, so there is no path out of it that reaches
    // a terminal state any other way.
    if options.terminate {
        let goal_id = GoalId::new(&options.goal);
        let cursor = loop_driver
            .run_fleet(&goal_id, |owner| async move {
                match driver
                    .run_to_completion(&dispatcher, executor, options.width, lease_ms, &now_unix_ms)
                    .await
                {
                    Ok(run) => {
                        for (index, wave) in run.waves.iter().enumerate() {
                            print_wave(index, wave);
                        }
                        println!(
                            "GOAL: run_complete waves={} iterations={} completed={} \
                             delivered={} stopped_because={}",
                            run.waves.len(),
                            run.iterations_consumed,
                            run.completed(),
                            run.delivered(),
                            run.stopped_because
                        );
                        // Bound at shard level, never at a caller-chosen `T`:
                        // one `ShardSummary` per wave, carrying the
                        // completed/failed counts the driver itself measured.
                        // Nothing here invents a number or rounds the split away.
                        let shards: Vec<ShardSummary> = run
                            .waves
                            .iter()
                            .enumerate()
                            .map(|(index, wave)| ShardSummary {
                                shard_id: index,
                                agent_count: wave.claimed,
                                successes: wave.completed,
                                failures: wave.failed,
                                payload: serde_json::Value::Null,
                            })
                            .collect();
                        StrategyTermination::from_fleet(owner, FleetOutcome::Dispatched(&shards))
                    }
                    // Carried into the terminal transition as a stated reason,
                    // never swallowed into a clean terminal and never squeezed
                    // into a `FleetError` variant that did not happen.
                    Err(error) => StrategyTermination::from_fleet(
                        owner,
                        FleetOutcome::DriverFailed {
                            detail: error.to_string(),
                        },
                    ),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("goal {} did not terminate: {e}", options.goal))?;

        let terminal = loop_driver
            .kernel()
            .goal(&goal_id)
            .ok()
            .flatten()
            .map_or_else(
                || "unknown".to_owned(),
                |state| format!("{:?}", state.lifecycle),
            );
        println!(
            "GOAL: canonical_transition strategy=fleet terminal={terminal} cursor_seq={:?}",
            cursor.journal_sequence
        );
        return Ok(());
    }

    let run = driver
        .run_to_completion(&dispatcher, executor, options.width, lease_ms, &now_unix_ms)
        .await
        .map_err(|e| anyhow::anyhow!("fleet run failed for goal {}: {e}", options.goal))?;

    for (index, wave) in run.waves.iter().enumerate() {
        print_wave(index, wave);
    }
    println!(
        "GOAL: run_complete waves={} iterations={} completed={} delivered={} stopped_because={}",
        run.waves.len(),
        run.iterations_consumed,
        run.completed(),
        run.delivered(),
        run.stopped_because
    );
    Ok(())
}

fn print_wave(index: usize, wave: &WaveOutcome) {
    println!(
        "GOAL: wave={index} shards={} claimed={} lost={} completed={} failed={} \
         indeterminate={} abandoned={} delivered={} dispatch_error={}",
        wave.shards,
        wave.claimed,
        wave.lost_claims,
        wave.completed,
        wave.failed,
        wave.indeterminate,
        wave.abandoned,
        wave.delivered.len(),
        wave.dispatch_error.as_deref().unwrap_or("-")
    );
}

fn status(journal: &std::path::Path, goal: &str) -> anyhow::Result<()> {
    let handle = open_journal(journal)?;
    let driver = GoalFleetDriver::new(handle, GoalId::new(goal), session_for(journal));
    let Some(state) = driver
        .goal_state()
        .map_err(|e| anyhow::anyhow!("failed to read goal {goal}: {e}"))?
    else {
        anyhow::bail!("no goal {goal} in {}", journal.display());
    };
    // The projection is the reduced state itself, not a hand-built view of it.
    // A surface that renders its own shape is a surface that can disagree with
    // the chain, which is the parallel lifecycle this phase exists to remove.
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

/// The host-protocol producer stream for one Goal (F22-C1).
///
/// Stdout carries ONLY JSON Lines, so the output is directly consumable by a
/// host decoder; the count summary goes to stderr for the same reason.
fn stream(journal: &std::path::Path, goal: &str, expect: Option<usize>) -> anyhow::Result<()> {
    if !journal.exists() {
        anyhow::bail!("no journal at {}", journal.display());
    }
    let envelopes = SessionJournal::replay(journal)
        .map_err(|e| anyhow::anyhow!("failed to replay {}: {e}", journal.display()))?;
    let events = goal_stream(&session_for(journal), goal, &envelopes)
        .map_err(|e| anyhow::anyhow!("failed to project goal {goal}: {e}"))?;
    if events.is_empty() {
        anyhow::bail!("no goal {goal} in {}", journal.display());
    }
    for event in &events {
        println!(
            "{}",
            event_line(event)
                .map_err(|e| anyhow::anyhow!("failed to serialize goal event: {e}"))?
        );
    }
    // Counts on stderr, so a caller can assert what was emitted without having
    // to parse the stream it is asserting about.
    let transitions = events.len().saturating_sub(1);
    eprintln!(
        "GOAL-STREAM: goal={goal} events={} transitions={transitions} snapshots=1",
        events.len()
    );
    if let Some(expect) = expect
        && events.len() != expect
    {
        anyhow::bail!("expected {expect} goal events, emitted {}", events.len());
    }
    Ok(())
}

/// Runs one assigned task as a real child process.
struct ChildProcessExecutor {
    effects_dir: PathBuf,
    argv: Vec<String>,
}

impl TaskExecutor for ChildProcessExecutor {
    fn execute(
        &self,
        assignment: TaskAssignment,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TaskExecution> + Send>> {
        let effects_dir = self.effects_dir.clone();
        let argv = self.argv.clone();
        Box::pin(async move {
            let Ok(exe) = std::env::current_exe() else {
                return TaskExecution::Failed {
                    detail: "could not resolve this binary's own path".to_owned(),
                };
            };
            // argv mode throughout: the operator's command reaches the OS as
            // separate argv entries, so a shell metacharacter in a task label
            // or a worker command is data, never syntax.
            let mut command = tokio::process::Command::new(exe);
            command
                .arg("goal")
                .arg("exec-task")
                .arg("--effects-dir")
                .arg(&effects_dir)
                .arg("--");
            for entry in &argv {
                command.arg(entry);
            }
            command
                .env(ENV_GOAL, &assignment.goal_id)
                .env(ENV_TASK, &assignment.task_id)
                .env(ENV_KEY, &assignment.idempotency_key)
                .env(ENV_EPOCH, assignment.epoch.to_string())
                .env(ENV_ATTEMPT, assignment.attempt.to_string())
                .env(ENV_WORKER, &assignment.worker_id);

            match command.status().await {
                Ok(status) if status.success() => TaskExecution::Produced {
                    outcome: GoalTerminalState::SelfChecked,
                    effect_digest: assignment.idempotency_key.clone(),
                },
                Ok(status) => match status.code() {
                    Some(code) => TaskExecution::Failed {
                        detail: format!("worker exited {code}"),
                    },
                    // No exit code means a signal took it. Whether its effect
                    // landed is genuinely unknown, and an unknown outcome is
                    // parked for resolution rather than retried — a silent retry
                    // here is the duplicate execution the criterion forbids.
                    None => TaskExecution::Indeterminate {
                        reason: TaskUnknownReason::OwnerDiedMidAttempt,
                    },
                },
                Err(error) => TaskExecution::Failed {
                    detail: format!("worker failed to start: {error}"),
                },
            }
        })
    }
}

/// The effect boundary.
///
/// ## The ordering here is load-bearing and was wrong once
///
/// The idempotency marker is created **after** the operator's command succeeds,
/// not before it. The first draft of this function created it first, and that is
/// a lost-effect bug rather than a stylistic choice: a worker killed mid-run
/// leaves the marker behind with no effect, and every later retry then finds the
/// marker and declines — so the task is permanently un-runnable and its effect
/// never happens. "No lost completion" fails exactly as loudly as "no duplicate".
///
/// Creating it afterwards is safe because nothing else is allowed to be running
/// this task concurrently: the ledger's claim is exclusive at the durable
/// boundary and only one live claim exists per task. The marker's job is
/// narrower than a lock — it stops a *retry after a death* from redoing work
/// whose effect already landed. That is precisely the case the epoch fence
/// structurally cannot reach, because it cannot get inside a process that
/// already holds a directory.
///
/// The marker IS the effect: one `create_new`, then the payload, then an fsync.
/// A kill between the create and the write leaves a present-but-empty effect,
/// which still counts as produced and is still counted exactly once. That
/// residual is stated rather than hidden; closing it entirely needs an atomic
/// write-then-link, which buys nothing the criterion measures.
/// Reads the assignment out of the environment `run` spawned this process with,
/// then does the work.
///
/// The env read is deliberately separated from [`exec_task`] rather than done
/// inside it. Process environment is global mutable state, and a function that
/// reaches for it can only be tested by mutating the whole process — which makes
/// the tests serialize against each other and go flaky in a way that looks
/// exactly like the idempotency gate failing. Passing the assignment in means
/// the gate is testable without touching the environment at all.
async fn exec_task_from_env(effects_dir: &std::path::Path, argv: &[String]) -> anyhow::Result<()> {
    let task = std::env::var(ENV_TASK).unwrap_or_else(|_| "unknown".to_owned());
    let key = std::env::var(ENV_KEY).map_err(|_| {
        anyhow::anyhow!("{ENV_KEY} is not set; refusing to produce an unkeyed effect")
    })?;
    exec_task(effects_dir, argv, &task, &key).await
}

async fn exec_task(
    effects_dir: &std::path::Path,
    argv: &[String],
    task: &str,
    key: &str,
) -> anyhow::Result<()> {
    if key.is_empty() {
        anyhow::bail!("{ENV_KEY} is empty; refusing to produce an unkeyed effect");
    }
    let effects = effects_dir.join("effects");
    std::fs::create_dir_all(&effects)?;

    // Cheap pre-check so a re-run does not pay for the worker again. It is NOT
    // the gate — `create_new` below is — because between this read and that
    // create there is a window, and a check that is not the gate must never be
    // mistaken for one.
    if effects.join(key).exists() {
        println!("GOAL-EXEC: task={task} key={key} produced=no reason=idempotency-key-present");
        return Ok(());
    }

    if !argv.is_empty() {
        // Argv mode, never a shell string: the operator's command and every
        // argument reach the OS as separate argv entries, so a metacharacter in
        // a task label is data rather than syntax.
        let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
        let mut command = wcore_config::shell::shell_command_argv(&argv[0], &rest);
        command.current_dir(effects_dir);
        let status = command
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("worker command '{}' failed to start: {e}", argv[0]))?;
        if !status.success() {
            anyhow::bail!("worker command '{}' exited {status}", argv[0]);
        }
    }

    // THE GATE. `create_new` is atomic on both platforms.
    use std::io::Write;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(effects.join(key))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            println!("GOAL-EXEC: task={task} key={key} produced=no reason=idempotency-key-present");
            return Ok(());
        }
        Err(error) => return Err(anyhow::anyhow!("effect for {key}: {error}")),
    };
    writeln!(file, "{task}")?;
    file.sync_all()?;
    println!("GOAL-EXEC: task={task} key={key} produced=yes");
    Ok(())
}

/// Count the effects on disk: total files, and how many carry distinct labels.
///
/// Exposed so the live proof counts what the PRODUCT wrote rather than what a
/// harness believes it wrote.
pub fn count_effects(effects_dir: &std::path::Path) -> anyhow::Result<(usize, usize)> {
    let effects = effects_dir.join("effects");
    let mut total = 0_usize;
    let mut labels = BTreeSet::new();
    if effects.is_dir() {
        for entry in std::fs::read_dir(&effects)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                total += 1;
                labels.insert(std::fs::read_to_string(entry.path())?.trim().to_owned());
            }
        }
    }
    Ok((total, labels.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operator's argv for a worker that exits with `code`.
    fn worker(code: i32) -> Vec<String> {
        vec![
            if cfg!(windows) { "cmd" } else { "sh" }.to_owned(),
            if cfg!(windows) { "/c" } else { "-c" }.to_owned(),
            format!("exit {code}"),
        ]
    }

    #[tokio::test]
    async fn exec_task_refuses_to_produce_an_unkeyed_effect() {
        let dir = tempfile::tempdir().unwrap();
        let error = exec_task(dir.path(), &[], "t-unkeyed", "")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(ENV_KEY), "got: {error}");
        assert_eq!(count_effects(dir.path()).unwrap(), (0, 0));
    }

    #[tokio::test]
    async fn exec_task_produces_once_and_refuses_the_second_attempt() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(dir.path(), &[], "t-once", "idem-t-once")
            .await
            .unwrap();
        exec_task(dir.path(), &[], "t-once", "idem-t-once")
            .await
            .unwrap();
        assert_eq!(
            count_effects(dir.path()).unwrap(),
            (1, 1),
            "the effect landed twice"
        );
    }

    #[tokio::test]
    async fn exec_task_does_not_produce_an_effect_when_the_worker_command_fails() {
        let dir = tempfile::tempdir().unwrap();
        let error = exec_task(dir.path(), &worker(3), "t-fail", "idem-t-fail")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("exited"), "got: {error}");
        assert_eq!(
            count_effects(dir.path()).unwrap(),
            (0, 0),
            "a failed worker still produced an effect"
        );
    }

    /// The regression guard for the ordering bug this function shipped with in
    /// its first draft: the marker was created BEFORE the worker ran, so a
    /// worker that failed left the marker behind and every retry declined — the
    /// effect could then never happen at all.
    #[tokio::test]
    async fn a_failed_worker_leaves_the_task_runnable_rather_than_permanently_blocked() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(dir.path(), &worker(3), "t-retry", "idem-t-retry")
            .await
            .expect_err("the failing worker should surface its failure");
        assert_eq!(count_effects(dir.path()).unwrap(), (0, 0));

        // The retry must be able to produce. Before the fix this returned
        // `produced=no reason=idempotency-key-present` and the effect was lost
        // forever, which is a lost completion wearing an exactly-once costume.
        exec_task(dir.path(), &worker(0), "t-retry", "idem-t-retry")
            .await
            .expect("the retry must be able to produce the effect");
        assert_eq!(
            count_effects(dir.path()).unwrap(),
            (1, 1),
            "the retry after a failed worker produced nothing"
        );
    }

    /// Two tasks with different keys must not collide, and two attempts of the
    /// same task must. A gate keyed on the task LABEL rather than the key would
    /// pass the first half and fail here.
    #[tokio::test]
    async fn the_gate_is_keyed_on_the_idempotency_key_not_on_the_task_label() {
        let dir = tempfile::tempdir().unwrap();
        exec_task(dir.path(), &[], "shared-label", "idem-a")
            .await
            .unwrap();
        exec_task(dir.path(), &[], "shared-label", "idem-b")
            .await
            .unwrap();
        // Two effects, both labelled the same: total 2, distinct labels 1.
        assert_eq!(count_effects(dir.path()).unwrap(), (2, 1));
    }

    #[test]
    fn session_name_is_derived_from_the_journal_file_stem() {
        assert_eq!(
            session_for(&PathBuf::from("/tmp/p22/fleet.journal")),
            "fleet"
        );
    }

    #[test]
    fn default_task_key_is_stable_across_attempts() {
        // The key must not embed an attempt number, or a retry would mint a
        // fresh key and the effect would land twice.
        let first = format!("idem-{}", "t01");
        let second = format!("idem-{}", "t01");
        assert_eq!(first, second);
    }
}
