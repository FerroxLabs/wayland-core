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
fn tool_run_guard_increments_and_decrements_processes() {
    let b = ExecutionBudget {
        max_processes: Some(1),
        ..Default::default()
    };
    let view = b.start_root();
    {
        let _g = view.enter_tool_run();
        assert!(!view.is_exceeded(), "exactly at the cap is allowed");
        let _g2 = view.enter_tool_run();
        assert!(
            view.is_exceeded(),
            "second concurrent tool run exceeds cap=1"
        );
        assert_eq!(view.first_exceeded_reason(), Some("max_processes"));
    }
    // After both guards drop, counters return to zero.
    assert!(!view.is_exceeded());
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
