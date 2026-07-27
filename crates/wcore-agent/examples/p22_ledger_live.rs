//! Live durability instrument for the Phase 22 durable Fleet task ledger.
//!
//! A REAL process, driving the REAL ledger code path, spawning REAL child
//! processes that produce REAL observable effects on disk, built to be killed
//! uncatchably mid-fanout and started again.
//!
//! ## Why this is an example and not the `wayland-core` subcommand
//!
//! **The caveat that used to live here is CLOSED, and is corrected rather than
//! carried forward.** Three successive summaries honestly repeated that the
//! shipped binary had no Goal surface, so this instrument was the strongest
//! available live proof and explicitly not the product. That is no longer true:
//! `wayland-core goal run` drives this exact ledger through the real
//! `FleetDispatcher`, and the kill/restart proof — `kill -9` on the process
//! group, 12 effects / 12 distinct / 12 expected, completions drained from the
//! outbox, a reassigned task refusing to produce its effect twice — now runs
//! against the release binary. See
//! `22-03-EVIDENCE/wire-live/linux/live-capture.txt`.
//!
//! This instrument is retained because it can stage a mid-flight state (an
//! effect on disk with no completion, held open by a lingering worker) more
//! precisely than the CLI can be driven into, which makes it a useful
//! adversarial harness. It is no longer the evidence Criterion 2 rests on.
//!
//! ## The two halves of at-most-once, and why both are here
//!
//! The ledger fences who may RECORD a completion — a superseded owner's write
//! is refused at the durable boundary. It cannot reach inside a worker process
//! that already holds a directory and stop it writing. So each task also
//! carries an idempotency key, and the worker creates a marker for that key
//! with `create_new` — atomic on both platforms — before producing its effect.
//! A legitimately retried attempt after its owner died finds the marker and
//! does not write again. Duplicate execution of the PROCESS is possible;
//! duplicate EFFECT is what the criterion forbids, and it is what these two
//! halves together bound.
//!
//! Modes:
//!   dispatch <journal> <goal> <effects-dir>  fan out to a mixed mid-flight
//!                                            state, mark ready, then spin
//!   resume   <journal> <goal> <effects-dir>  pick it up in a fresh process
//!   reassign <journal> <goal>                revoke a live claim and prove the
//!                                            superseded owner's write refused
//!   worker   <effects-dir> <key> <label> <pre-ms> <post-ms>
//!   report   <journal> <goal>                counts, no transitions

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::goal::{ClaimOutcome, GoalKernel, GoalLedger, TaskAuthority};
use wcore_agent::session_journal::{
    BudgetAmount, BudgetOwner, BudgetPurpose, BudgetUnit, SessionEvent, SessionJournal,
};
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalStrategy, GoalTerminalState, LoopPolicy, TaskId,
    resolve_goal_authority,
};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus,
};

const SESSION: &str = "p22-ledger-live";
/// Ten tasks, so a kill lands with tasks delivered, finished-but-undelivered,
/// claimed-and-running, and not-yet-started all at once.
const TASKS: usize = 10;

fn filled(character: char, len: usize) -> String {
    std::iter::repeat_n(character, len).collect()
}

fn task_name(index: usize) -> String {
    format!("t{index:02}")
}

/// `t05..t09` each depend on `t00..t04`, so the dependency release is exercised
/// rather than every task being independently claimable from the start.
fn dependency(index: usize) -> BTreeSet<String> {
    if index >= 5 {
        [task_name(index - 5)].into_iter().collect()
    } else {
        BTreeSet::new()
    }
}

fn child_record(child_id: &str) -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: format!("declare-{child_id}"),
        child_id: ChildId::new(child_id).expect("valid child id"),
        parent: ChildParent {
            session_id: SESSION.into(),
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
        provider: Some("live".into()),
        model: Some("live-model".into()),
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

struct Live {
    journal: SessionJournal,
    ledger: GoalLedger,
    goal: GoalId,
}

impl Live {
    fn open(journal_path: &str, goal: &str) -> Self {
        let journal = match SessionJournal::open(journal_path, SESSION) {
            Ok(journal) => journal,
            Err(error) => {
                // A kill -9 leaves the writer lease behind. Whether a fresh
                // process can still take the journal is part of what this
                // instrument measures, so the failure is reported rather than
                // swallowed.
                println!("LEDGER-LIVE: open=FAILED detail={error}");
                std::process::exit(3);
            }
        };
        Self {
            ledger: GoalLedger::new(journal.clone()),
            journal,
            goal: GoalId::new(goal),
        }
    }

    /// Reserve through the EXISTING budget events, charged to a declared child.
    /// A reassigned attempt re-enters this rather than minting a fresh budget.
    fn reserve(&self, reservation: &str) {
        DurableChildStore::new(self.journal.clone())
            .declare(child_record(reservation))
            .expect("child declares");
        self.journal
            .append(SessionEvent::BudgetReserved {
                event_id: format!("evt-{reservation}"),
                reservation_id: reservation.to_owned(),
                owner: BudgetOwner::Child {
                    child_id: reservation.to_owned(),
                },
                purpose: BudgetPurpose::ChildExecution,
                amount: BudgetAmount {
                    value: 1,
                    unit: BudgetUnit::Tokens,
                },
            })
            .expect("reservation commits");
    }

    fn claim(&self, task: &TaskId, worker: &str) -> Option<TaskAuthority> {
        let reservation = format!("res-{}-{worker}", task.as_str());
        self.reserve(&reservation);
        match self
            .ledger
            .claim_task(&self.goal, task, worker, &reservation, 30_000)
            .expect("claim decides")
        {
            ClaimOutcome::Won(authority) => Some(authority),
            ClaimOutcome::Lost { detail } => {
                println!("LEDGER-LIVE: claim_lost task={task} detail={detail}");
                None
            }
        }
    }
}

/// Spawn a REAL worker process. The effect must outlive the parent, so it is
/// produced by a separate process rather than by this one.
fn spawn_worker(
    effects: &Path,
    key: &str,
    label: &str,
    pre_ms: u64,
    post_ms: u64,
) -> std::process::Child {
    let exe = std::env::current_exe().expect("own path");
    std::process::Command::new(exe)
        .arg("worker")
        .arg(effects)
        .arg(key)
        .arg(label)
        .arg(pre_ms.to_string())
        .arg(post_ms.to_string())
        .spawn()
        .expect("worker spawns")
}

fn run_worker(effects: &Path, key: &str, label: &str, pre_ms: u64, post_ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(pre_ms));
    let keys = effects.join("keys");
    std::fs::create_dir_all(&keys).expect("key dir");
    // THE IDEMPOTENCY KEY, at the effect boundary. `create_new` is atomic on
    // both platforms, so a retried attempt after its owner died finds the
    // marker and refuses to produce the effect a second time. This is the half
    // the ledger's epoch fence structurally cannot reach.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(keys.join(key))
    {
        Ok(_) => {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(effects.join("effects.txt"))
                .expect("effects file");
            writeln!(file, "{label}").expect("effect line");
            file.sync_all().expect("fsync effect");
            println!("LEDGER-LIVE: worker_effect key={key} label={label} produced=yes");
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            println!(
                "LEDGER-LIVE: worker_effect key={key} label={label} produced=no reason=idempotency-key-present"
            );
        }
        Err(error) => panic!("key marker: {error}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(post_ms));
}

fn dispatch(journal_path: &str, goal: &str, effects: &Path) {
    let live = Live::open(journal_path, goal);
    std::fs::create_dir_all(effects).expect("effects dir");

    let request = GoalAuthorityRequest {
        requested_limits: [("max_tokens".to_owned(), 1_000_u64)].into_iter().collect(),
        strategy: GoalStrategy::Fleet,
        loop_policy: LoopPolicy::Fixed { iterations: 8 },
    };
    let snapshot = resolve_goal_authority(
        &request,
        &[("max_tokens".to_owned(), 10_000_u64)]
            .into_iter()
            .collect(),
        "parent-v1",
    );
    GoalKernel::new(live.journal.clone())
        .open_goal(&live.goal, "fan out durably", &snapshot, 1_700_000_000_000)
        .expect("goal opens");

    for index in 0..TASKS {
        let name = task_name(index);
        live.ledger
            .declare_task(
                &live.goal,
                &TaskId::new(name.clone()),
                &dependency(index),
                &format!("idem-{name}"),
            )
            .expect("task declares");
    }
    println!(
        "LEDGER-LIVE: declared tasks={TASKS} pid={}",
        std::process::id()
    );

    // t00..t03 — completed AND delivered.
    for index in 0..4 {
        let task = TaskId::new(task_name(index));
        let authority = live.claim(&task, "w-a").expect("claims");
        let mut worker = spawn_worker(effects, &format!("idem-{task}"), task.as_str(), 0, 0);
        worker.wait().expect("worker exits");
        live.ledger
            .complete_task(&authority, GoalTerminalState::SelfChecked, task.as_str())
            .expect("completes");
        live.ledger
            .deliver_completion(&live.goal, &task)
            .expect("delivers");
    }

    // t04 — completed but NOT delivered. The outbox must survive the kill.
    let t04 = TaskId::new(task_name(4));
    let authority = live.claim(&t04, "w-a").expect("claims");
    let mut worker = spawn_worker(effects, &format!("idem-{t04}"), t04.as_str(), 0, 0);
    worker.wait().expect("worker exits");
    live.ledger
        .complete_task(&authority, GoalTerminalState::SelfChecked, t04.as_str())
        .expect("completes");

    // t05 — claimed; its worker produces the effect and then lingers. The kill
    // lands with the effect ON DISK and no completion recorded, which is the
    // case the idempotency key exists for.
    let t05 = TaskId::new(task_name(5));
    let _ = live.claim(&t05, "w-b").expect("claims");
    let mut writer = spawn_worker(effects, &format!("idem-{t05}"), t05.as_str(), 0, 600_000);

    // t06 — claimed; its worker lingers BEFORE producing anything, so the kill
    // lands with neither effect nor completion.
    let t06 = TaskId::new(task_name(6));
    let _ = live.claim(&t06, "w-c").expect("claims");
    let mut sleeper = spawn_worker(effects, &format!("idem-{t06}"), t06.as_str(), 600_000, 0);

    // t07..t09 are declared and unclaimed.

    // Wait until t05's effect is genuinely on disk before declaring ready, so
    // the kill lands on a state we asserted rather than one we hoped for.
    let effect_file = effects.join("effects.txt");
    for _ in 0..300 {
        if std::fs::read_to_string(&effect_file)
            .map(|body| body.lines().any(|line| line == t05.as_str()))
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let ready = format!("{journal_path}.ready");
    let mut file = std::fs::File::create(&ready).expect("ready marker");
    writeln!(file, "{}", std::process::id()).expect("write pid");
    file.sync_all().expect("fsync ready marker");
    println!("LEDGER-LIVE: ready_for_kill pid={}", std::process::id());

    // Hold the writer lease exactly as a live run would. The children are
    // deliberately left running so the kill has a real process tree to take.
    let _ = (&mut writer, &mut sleeper);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn resume(journal_path: &str, goal: &str, effects: &Path) {
    let live = Live::open(journal_path, goal);
    let recovery = GoalKernel::new(live.journal.clone())
        .recover_with_parent_envelope(&live.goal, "parent-v1")
        .expect("recovery decides");
    println!("LEDGER-LIVE: recovery={recovery:?}");

    // Reclaim every task whose owner did not survive. The revocation is a
    // supervisor action against an owner that may be dead OR merely slow; the
    // epoch is what makes the difference not matter.
    let state = live.journal.state().expect("state reduces");
    let orphaned: Vec<String> = state.goals[goal]
        .tasks
        .values()
        .filter(|task| task.live_attempt().is_some())
        .map(|task| task.task_id.clone())
        .collect();
    for name in &orphaned {
        live.ledger
            .revoke_claim(
                &live.goal,
                &TaskId::new(name.clone()),
                "owner did not survive the kill",
            )
            .expect("revocation commits");
        println!("LEDGER-LIVE: revoked task={name}");
    }

    // Run everything still claimable, orphans included. An orphan whose worker
    // already produced its effect is re-run and the key stops it landing twice.
    loop {
        let claimable = live.ledger.claimable(&live.goal).expect("claimable reads");
        let Some(task) = claimable.into_iter().next() else {
            break;
        };
        let Some(authority) = live.claim(&task, "w-resume") else {
            break;
        };
        let mut worker = spawn_worker(effects, &format!("idem-{task}"), task.as_str(), 0, 0);
        worker.wait().expect("worker exits");
        live.ledger
            .complete_task(&authority, GoalTerminalState::SelfChecked, task.as_str())
            .expect("completes");
    }

    // Drain the outbox. Every completion durable before the kill, including the
    // one the parent never observed, wakes the parent now.
    let pending = live.ledger.pending_deliveries(&live.goal).expect("outbox");
    for task in &pending {
        live.ledger
            .deliver_completion(&live.goal, task)
            .expect("delivers");
    }
    println!("LEDGER-LIVE: drained_outbox count={}", pending.len());
    report(&live);
}

/// Revoke a live claim and prove the superseded owner's write is refused.
///
/// The old owner is STILL ALIVE and still holding its authority — the case a
/// timeout cannot distinguish from a dead one, and the reason the panel chose a
/// fencing token over the two liveness-only options.
fn reassign(journal_path: &str, goal: &str) {
    let live = Live::open(journal_path, goal);
    let request = GoalAuthorityRequest {
        requested_limits: [("max_tokens".to_owned(), 1_000_u64)].into_iter().collect(),
        strategy: GoalStrategy::Fleet,
        loop_policy: LoopPolicy::Once,
    };
    let snapshot = resolve_goal_authority(
        &request,
        &[("max_tokens".to_owned(), 10_000_u64)]
            .into_iter()
            .collect(),
        "parent-v1",
    );
    GoalKernel::new(live.journal.clone())
        .open_goal(
            &live.goal,
            "prove the fence live",
            &snapshot,
            1_700_000_000_000,
        )
        .expect("goal opens");

    let task = TaskId::new("t-fence");
    live.ledger
        .declare_task(&live.goal, &task, &BTreeSet::new(), "idem-t-fence")
        .expect("task declares");

    let superseded = live.claim(&task, "w-old").expect("claims");
    live.ledger
        .revoke_claim(
            &live.goal,
            &task,
            "lease expired while the owner may still run",
        )
        .expect("revocation commits");
    let successor = live.claim(&task, "w-new").expect("reclaims");

    match live
        .ledger
        .complete_task(&superseded, GoalTerminalState::SelfChecked, "late-write")
    {
        Err(error) => println!("LEDGER-LIVE: REASSIGN refused_late_write=yes detail={error}"),
        Ok(()) => {
            println!("LEDGER-LIVE: REASSIGN refused_late_write=no");
            std::process::exit(4);
        }
    }

    live.ledger
        .complete_task(
            &successor,
            GoalTerminalState::SelfChecked,
            "successor-write",
        )
        .expect("the current owner completes");
    let state = live
        .ledger
        .task(&live.goal, &task)
        .expect("read")
        .expect("task exists");
    println!(
        "LEDGER-LIVE: REASSIGN attempts={} effect={:?}",
        state.attempts.len(),
        state.completion.map(|completion| completion.effect_digest)
    );
}

fn report(live: &Live) {
    let state = live.journal.state().expect("state reduces");
    let goal = &state.goals[live.goal.as_str()];
    let completed = goal
        .tasks
        .values()
        .filter(|t| t.completion.is_some())
        .count();
    let delivered = goal
        .tasks
        .values()
        .filter(|t| t.completion.as_ref().is_some_and(|c| c.delivered))
        .count();
    let releases: u64 = goal.tasks.values().map(|t| t.dependency_releases).sum();
    let attempts: usize = goal.tasks.values().map(|t| t.attempts.len()).sum();
    let unresolved = goal
        .tasks
        .values()
        .filter(|t| t.requires_resolution())
        .count();
    println!(
        "LEDGER-LIVE: REPORT tasks={} completed={completed} delivered={delivered} \
         dependency_releases={releases} attempts={attempts} unresolved={unresolved}",
        goal.tasks.len()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: p22_ledger_live <dispatch|resume|reassign|worker|report> ...");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "dispatch" => dispatch(&args[2], &args[3], &PathBuf::from(&args[4])),
        "resume" => resume(&args[2], &args[3], &PathBuf::from(&args[4])),
        "reassign" => reassign(&args[2], &args[3]),
        "worker" => run_worker(
            &PathBuf::from(&args[2]),
            &args[3],
            &args[4],
            args[5].parse().expect("pre-ms"),
            args[6].parse().expect("post-ms"),
        ),
        "report" => report(&Live::open(&args[2], &args[3])),
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
}
