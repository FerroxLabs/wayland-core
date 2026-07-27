//! Live durability instrument for the Phase 22 Goal kernel.
//!
//! This is a REAL separate process writing a REAL journal to disk through the
//! shipped kernel code path. It exists to be killed uncatchably.
//!
//! ## Why this is an example and not the `wayland-core` subcommand
//!
//! Stated plainly rather than glossed: the shipped binary has NO Goal surface
//! yet. Adding one is plan 22-04 (`crates/wcore-cli/src/goal_cmd.rs`), which
//! this lane is explicitly not executing, and 22-04 is itself blocked on the
//! kernel this file exercises. So the strongest honest live proof available at
//! this commit is a real process running the real kernel against a real
//! on-disk journal, killed with a real SIGKILL — not a mocked crash, not a
//! cooperative shutdown, and not a test harness that never leaves the process.
//!
//! What this does NOT prove is the shipped `wayland-core` binary resuming a
//! Goal, because no user-reachable path can create one yet. That gap is real
//! and is recorded as such.
//!
//! Modes:
//!   open   <journal> <goal-id> <parent-digest>  authorize a Goal, run one
//!                                              iteration, park on a wait,
//!                                              signal ready, then spin forever
//!   resume <journal> <goal-id> <parent-digest>  pick it up in a fresh process
//!   show   <journal> <goal-id>                  read state without transitioning

use std::collections::BTreeMap;
use std::io::Write;

use wcore_agent::goal::{GoalKernel, GoalLifecycle, GoalRecovery};
use wcore_agent::session_journal::SessionJournal;
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalStrategy, LoopPolicy, WaitKind, resolve_goal_authority,
};

const SESSION: &str = "p22-goal-live";

fn parent_limits() -> BTreeMap<String, u64> {
    [
        ("max_tokens".to_owned(), 1000_u64),
        ("max_cost_cents".to_owned(), 25_u64),
    ]
    .into_iter()
    .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: p22_goal_live <open|resume|show> <journal> <goal-id> [parent-digest]");
        std::process::exit(2);
    }
    let mode = args[1].as_str();
    let journal_path = args[2].clone();
    let goal_id = GoalId::new(args[3].clone());
    let parent_digest = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| "parent-v1".to_owned());

    let journal = match SessionJournal::open(&journal_path, SESSION) {
        Ok(journal) => journal,
        Err(error) => {
            // A kill -9 leaves the writer lease file behind. Whether a fresh
            // process can still take the journal is exactly the thing this
            // instrument is here to find out, so the failure is reported as a
            // measured outcome rather than swallowed.
            println!("GOAL-LIVE: open=FAILED detail={error}");
            std::process::exit(3);
        }
    };
    println!("GOAL-LIVE: open=OK pid={}", std::process::id());
    let kernel = GoalKernel::new(journal);

    match mode {
        "open" => {
            let request = GoalAuthorityRequest {
                requested_limits: [("max_tokens".to_owned(), 500_u64)].into_iter().collect(),
                strategy: GoalStrategy::Anvil,
                loop_policy: LoopPolicy::Fixed { iterations: 4 },
            };
            let snapshot = resolve_goal_authority(&request, &parent_limits(), &parent_digest);

            let cursor = kernel
                .open_goal(
                    &goal_id,
                    "survive an uncatchable kill",
                    &snapshot,
                    1_700_000_000_000,
                )
                .expect("open goal");
            println!(
                "GOAL-LIVE: opened goal={goal_id} cursor_seq={:?} cursor_digest={}",
                cursor.journal_sequence, cursor.journal_digest
            );

            kernel.start_iteration(&goal_id).expect("iteration 1");
            kernel
                .begin_wait(
                    &goal_id,
                    WaitKind::Event {
                        event: "external-signal".to_owned(),
                    },
                )
                .expect("begin wait");

            let goal = kernel.goal(&goal_id).expect("read").expect("exists");
            println!(
                "GOAL-LIVE: pre_kill lifecycle={:?} iterations={} resumes={}",
                goal.lifecycle, goal.iterations_started, goal.resume_count
            );

            // Tell the harness the durable work is committed and it is safe to
            // kill. Written and flushed AFTER the appends, so a kill that lands
            // on this marker lands with real committed state on disk.
            let ready = format!("{journal_path}.ready");
            let mut file = std::fs::File::create(&ready).expect("ready marker");
            writeln!(file, "{}", std::process::id()).expect("write pid");
            file.sync_all().expect("fsync ready marker");

            println!("GOAL-LIVE: ready_for_kill pid={}", std::process::id());
            // Spin holding the writer lease, exactly as a live run would.
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
        "resume" => {
            let recovery = kernel
                .recover_with_parent_envelope(&goal_id, &parent_digest)
                .expect("recovery decides");
            match &recovery {
                GoalRecovery::Resumed {
                    snapshot,
                    iterations_started,
                    resume_count,
                } => {
                    let tokens = snapshot.effective_limits.get("max_tokens").copied();
                    let cost = snapshot.effective_limits.get("max_cost_cents").copied();
                    println!(
                        "GOAL-LIVE: RESUMED iterations={iterations_started} resumes={resume_count} \
                         max_tokens={tokens:?} max_cost_cents={cost:?} strategy={:?}",
                        snapshot.strategy
                    );
                }
                GoalRecovery::AlreadyTerminal { terminal } => {
                    println!("GOAL-LIVE: ALREADY-TERMINAL terminal={terminal:?}");
                }
                GoalRecovery::Blocked { terminal } => {
                    println!("GOAL-LIVE: PARKED terminal={terminal:?}");
                }
            }
            let goal = kernel.goal(&goal_id).expect("read").expect("exists");
            let cursor = goal.cursor();
            println!(
                "GOAL-LIVE: post_resume objective={:?} lifecycle={:?} cursor_seq={:?} cursor_digest={}",
                goal.objective, goal.lifecycle, cursor.journal_sequence, cursor.journal_digest
            );
        }
        "hold" => {
            // Take the writer lease and spin without transitioning. Used to
            // stage a SECOND uncatchable kill against a process that genuinely
            // holds the journal, so the lease-recovery path is exercised under a
            // real crash rather than after a clean exit.
            let ready = format!("{journal_path}.ready");
            let mut file = std::fs::File::create(&ready).expect("ready marker");
            writeln!(file, "{}", std::process::id()).expect("write pid");
            file.sync_all().expect("fsync ready marker");
            println!("GOAL-LIVE: holding_lease pid={}", std::process::id());
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
        "show" => {
            let goal = kernel.goal(&goal_id).expect("read").expect("exists");
            let waiting = matches!(goal.lifecycle, GoalLifecycle::Waiting { .. });
            println!(
                "GOAL-LIVE: show lifecycle={:?} waiting={waiting} iterations={} resumes={}",
                goal.lifecycle, goal.iterations_started, goal.resume_count
            );
        }
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
}
