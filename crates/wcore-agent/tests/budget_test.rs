//! W8a A.2 — ExecutionBudget + CancellationToken plumbing tests.
//!
//! Covers the runtime view created from a budget config, the per-cap
//! exceeded-reason reporting, sub-budget tree semantics, and a thin
//! cooperative-cancellation handle that fires when the budget trips.

use std::time::Duration;

use wcore_agent::budget::ExecutionBudget;
use wcore_agent::cancel::{CancellationToken, budget_linked, child_of};

#[test]
fn budget_default_has_no_limits() {
    let b = ExecutionBudget::default();
    assert!(b.max_wall_time.is_none());
    assert!(b.max_tool_runtime.is_none());
    assert!(b.max_processes.is_none());
    assert!(b.max_agent_depth.is_none());
    assert!(b.max_tokens_in.is_none());
    assert!(b.max_tokens_out.is_none());
    assert!(b.max_cost_usd.is_none());

    let view = b.start_root();
    assert!(!view.is_exceeded());
    assert!(view.first_exceeded_reason().is_none());
}

#[test]
fn budget_with_max_wall_time_blocks_if_elapsed() {
    let b = ExecutionBudget {
        max_wall_time: Some(Duration::from_millis(10)),
        ..Default::default()
    };
    let view = b.start_root();
    std::thread::sleep(Duration::from_millis(25));
    assert!(view.is_exceeded());
    assert_eq!(view.first_exceeded_reason(), Some("max_wall_time"));
}

#[test]
fn budget_tokens_exceeded_reports_reason() {
    let b = ExecutionBudget {
        max_tokens_out: Some(100),
        ..Default::default()
    };
    let view = b.start_root();
    view.record_tokens(0, 50);
    assert!(!view.is_exceeded());
    view.record_tokens(0, 51);
    assert!(view.is_exceeded());
    assert_eq!(view.first_exceeded_reason(), Some("max_tokens_out"));
}

#[test]
fn budget_cost_exceeded_reports_reason() {
    let b = ExecutionBudget {
        max_cost_usd: Some(0.50),
        ..Default::default()
    };
    let view = b.start_root();
    view.record_cost(0.49);
    assert!(!view.is_exceeded());
    view.record_cost(0.02);
    assert!(view.is_exceeded());
    assert_eq!(view.first_exceeded_reason(), Some("max_cost_usd"));
}

#[test]
fn sub_budget_inherits_parent_caps_by_default() {
    let parent = ExecutionBudget {
        max_tokens_out: Some(100),
        ..Default::default()
    };
    let parent_view = parent.start_root();
    let child = parent_view.sub_budget(None);
    // Recording on child rolls up to parent.
    child.record_tokens(0, 101);
    assert!(parent_view.is_exceeded());
    assert!(child.is_exceeded());
}

#[test]
fn sub_budget_can_override_parent() {
    let parent = ExecutionBudget {
        max_tokens_out: Some(1_000),
        ..Default::default()
    };
    let parent_view = parent.start_root();
    let stricter = ExecutionBudget {
        max_tokens_out: Some(10),
        ..Default::default()
    };
    let child = parent_view.sub_budget(Some(stricter));
    child.record_tokens(0, 11);
    assert!(child.is_exceeded());
    assert_eq!(child.first_exceeded_reason(), Some("max_tokens_out"));
    // Parent receives the rollup but its own cap is 1_000 so it stays under.
    assert!(!parent_view.is_exceeded());
}

#[test]
fn nested_sub_budget_rolls_usage_to_every_ancestor() {
    let root = ExecutionBudget {
        max_tokens_out: Some(100),
        ..Default::default()
    }
    .start_root();
    let child = root.sub_budget(None);
    let grandchild = child.sub_budget(None);

    grandchild.record_tokens(0, 101);

    assert_eq!(root.first_exceeded_reason(), Some("max_tokens_out"));
    assert_eq!(child.first_exceeded_reason(), Some("max_tokens_out"));
    assert_eq!(grandchild.first_exceeded_reason(), Some("max_tokens_out"));
}

#[test]
fn descendant_reports_the_ancestor_cap_that_actually_stopped_it() {
    let root = ExecutionBudget {
        max_tokens_out: Some(100),
        ..Default::default()
    }
    .start_root();
    let child = root.sub_budget(Some(ExecutionBudget {
        max_tokens_out: Some(1_000),
        ..Default::default()
    }));

    child.record_tokens(0, 101);

    assert_eq!(child.first_exceeded_reason(), Some("max_tokens_out"));
    assert_eq!(child.observed_for("max_tokens_out"), "101");
    assert_eq!(child.limit_for("max_tokens_out"), "100");
}

#[test]
fn nested_sub_budget_guards_roll_up_and_release_every_ancestor() {
    let root = ExecutionBudget {
        max_processes: Some(1),
        ..Default::default()
    }
    .start_root();
    let child = root.sub_budget(None);
    let grandchild = child.sub_budget(None);

    let first = child.try_enter_process().expect("first slot is available");
    let error = match grandchild.try_enter_process() {
        Ok(_) => panic!("shared ancestor cap must reject the second process"),
        Err(error) => error,
    };
    assert_eq!(error.reason, "max_concurrent_process_tools");
    assert_eq!(error.limit, 1);
    assert_eq!(error.observed, 2);
    assert!(!root.is_exceeded(), "refused work must not oversubscribe");

    drop(first);
    assert!(!root.is_exceeded());
    let second = grandchild
        .try_enter_process()
        .expect("dropping the first guard releases every ancestor");
    drop(second);
}

#[test]
fn tool_run_guard_increments_and_decrements_processes() {
    let b = ExecutionBudget {
        max_processes: Some(1),
        ..Default::default()
    };
    let view = b.start_root();
    {
        let _g = view.try_enter_process().expect("first slot is available");
        assert!(!view.is_exceeded(), "exactly at the cap is allowed");
        assert!(view.try_enter_process().is_err());
        assert!(!view.is_exceeded(), "refused work never enters the counter");
    }
    // After the guard drops, the slot can be reserved again.
    let _g = view.try_enter_process().expect("released slot is reusable");
    assert!(!view.is_exceeded());
}

#[test]
fn concurrent_process_admission_never_oversubscribes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    const CAP: usize = 4;
    const CONTENDERS: usize = 32;

    let view = ExecutionBudget {
        max_processes: Some(CAP),
        ..Default::default()
    }
    .start_root();
    let release = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let mut threads = Vec::new();

    for _ in 0..CONTENDERS {
        let contender = view.clone();
        let release = Arc::clone(&release);
        let tx = tx.clone();
        threads.push(std::thread::spawn(move || {
            let guard = contender.try_enter_process().ok();
            tx.send(guard.is_some()).expect("receiver remains alive");
            while guard.is_some() && !release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            drop(guard);
        }));
    }
    drop(tx);

    let admitted = (0..CONTENDERS)
        .map(|_| rx.recv().expect("every contender reports admission"))
        .filter(|admitted| *admitted)
        .count();
    assert_eq!(admitted, CAP, "exactly the available slots are admitted");
    assert!(!view.is_exceeded(), "admission never creates an overshoot");

    release.store(true, Ordering::Release);
    for thread in threads {
        thread.join().expect("contender did not panic");
    }
    let guard = view
        .try_enter_process()
        .expect("all concurrent reservations were released");
    drop(guard);
}

#[test]
fn concurrent_tool_runtime_admission_never_multiplies_the_cap() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    const CAP: Duration = Duration::from_millis(100);
    const CONTENDERS: usize = 32;

    let view = ExecutionBudget {
        max_tool_runtime: Some(CAP),
        ..Default::default()
    }
    .start_root();
    let release = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let mut threads = Vec::new();

    for _ in 0..CONTENDERS {
        let contender = view.clone();
        let release = Arc::clone(&release);
        let tx = tx.clone();
        threads.push(std::thread::spawn(move || {
            let guard = contender.try_reserve_tool_runtime(CAP).ok();
            tx.send(guard.is_some()).expect("receiver remains alive");
            while guard.is_some() && !release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            drop(guard);
        }));
    }
    drop(tx);

    let admitted = (0..CONTENDERS)
        .map(|_| rx.recv().expect("every contender reports admission"))
        .filter(|admitted| *admitted)
        .count();
    assert_eq!(admitted, 1, "only one call may reserve the full cap");
    assert_eq!(view.remaining_tool_runtime(), Some(Duration::ZERO));
    assert!(!view.is_exceeded(), "reserved work is bounded at the cap");

    release.store(true, Ordering::Release);
    for thread in threads {
        thread.join().expect("contender did not panic");
    }
    assert_eq!(
        view.remaining_tool_runtime(),
        Some(Duration::ZERO),
        "an unsettled guard is conservatively charged on drop"
    );
}

#[test]
fn tool_runtime_settlement_refunds_unused_reservation() {
    let view = ExecutionBudget {
        max_tool_runtime: Some(Duration::from_millis(100)),
        ..Default::default()
    }
    .start_root();
    let mut reservation = view
        .try_reserve_tool_runtime(Duration::from_millis(80))
        .expect("runtime is available");
    assert_eq!(
        view.remaining_tool_runtime(),
        Some(Duration::from_millis(20))
    );

    reservation.settle(Duration::from_millis(10));
    drop(reservation);

    assert_eq!(
        view.remaining_tool_runtime(),
        Some(Duration::from_millis(90))
    );
}

#[test]
fn remaining_tool_runtime_honors_the_strictest_ancestor() {
    let root = ExecutionBudget {
        max_tool_runtime: Some(Duration::from_millis(100)),
        ..Default::default()
    }
    .start_root();
    let child = root.sub_budget(Some(ExecutionBudget {
        max_tool_runtime: Some(Duration::from_millis(40)),
        ..Default::default()
    }));
    let grandchild = child.sub_budget(None);

    grandchild.record_tool_runtime(Duration::from_millis(10));

    assert_eq!(
        root.remaining_tool_runtime(),
        Some(Duration::from_millis(90))
    );
    assert_eq!(
        grandchild.remaining_tool_runtime(),
        Some(Duration::from_millis(30))
    );
}

#[tokio::test]
async fn cancellation_token_propagates_to_children() {
    let parent = CancellationToken::new();
    let child = child_of(&parent);
    assert!(!child.is_cancelled());
    parent.cancel();
    assert!(child.is_cancelled());
}

#[tokio::test]
async fn budget_linked_cancel_fires_when_budget_exceeded() {
    let b = ExecutionBudget {
        max_wall_time: Some(Duration::from_millis(10)),
        ..Default::default()
    };
    let view = b.start_root();
    let root = CancellationToken::new();
    let linked = budget_linked(root.clone(), view);
    // Wait long enough for the watcher to observe the wall-time trip.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(linked.is_cancelled());
    // Root token also fired by the watcher (it cancels the linked token,
    // which is the root.child_token() pair below).
}

/// W8a A.5 — `BudgetConfig` (TOML seconds) → `ExecutionBudget` (Duration)
/// conversion. Lives in wcore-agent::budget because wcore-config sits
/// below wcore-agent in the dep graph.
#[test]
fn budget_config_into_execution_budget_translates_seconds_to_duration() {
    use wcore_config::budget::BudgetConfig;

    let cfg = BudgetConfig {
        max_wall_time_secs: Some(600),
        max_tool_runtime_secs: Some(30),
        max_processes: Some(4),
        max_agent_depth: Some(2),
        max_tokens_in: Some(100_000),
        max_tokens_out: Some(16_384),
        max_cost_usd: Some(0.50),
    };
    let exec: ExecutionBudget = (&cfg).into();
    assert_eq!(exec.max_wall_time, Some(Duration::from_secs(600)));
    assert_eq!(exec.max_tool_runtime, Some(Duration::from_secs(30)));
    assert_eq!(exec.max_processes, Some(4));
    assert_eq!(exec.max_agent_depth, Some(2));
    assert_eq!(exec.max_tokens_in, Some(100_000));
    assert_eq!(exec.max_tokens_out, Some(16_384));
    assert_eq!(exec.max_cost_usd, Some(0.50));
}

#[test]
fn budget_config_default_into_execution_budget_has_no_caps() {
    use wcore_config::budget::BudgetConfig;

    let cfg = BudgetConfig::default();
    let exec: ExecutionBudget = cfg.into();
    assert_eq!(exec, ExecutionBudget::default());
}
