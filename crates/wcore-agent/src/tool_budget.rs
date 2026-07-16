//! W8b — per-tool ExecutionBudget tracking helpers.
//!
//! The W8a `ExecutionBudgetView` already tracks wall-time, tokens, cost,
//! tool_runtime, processes, and agent depth at the session level. W8b
//! adds an additional aggregation layer that records per-tool runtime
//! and call-count, so the orchestration layer can answer "did Bash
//! consume our entire budget?" without re-walking the trace.
//!
//! The struct lives alongside `ExecutionBudgetView` rather than inside
//! it because per-tool charging is a different concern (the existing
//! view rolls counters up to ancestors; per-tool tracking is flat).
//!
//! Production orchestration classifies each call and admits it through this
//! tracker before invoking the tool:
//!
//! ```ignore
//! let tracker = ToolBudgetTracker::new();
//! let guard = tracker.try_start(
//!     tool_name,
//!     tool.execution_class_for(&input),
//!     category_timeout,
//! )?;
//! let result = tool.execute_with_ctx(input, &ctx).await;
//! drop(guard);  // records elapsed
//! let usage = tracker.usage_for(tool_name);
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
pub use wcore_tools::ToolExecutionClass;

/// Per-tool runtime + call counts. Cheap to clone (Arc-backed).
#[derive(Clone, Default)]
pub struct ToolBudgetTracker {
    inner: Arc<Mutex<HashMap<String, ToolUsage>>>,
    execution_budget: Option<crate::budget::ExecutionBudgetView>,
    budget_authority: Option<crate::budget_authority::SharedBudgetAuthorityCoordinator>,
}

/// Aggregated usage for a single tool name across a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolUsage {
    pub calls: u64,
    pub total_runtime: Duration,
}

/// RAII guard returned by `ToolBudgetTracker::start`. On drop, records
/// the elapsed runtime back into the tracker. Cancel-safe — if the tool
/// call is aborted, the partial runtime is still recorded so budget
/// reports reflect real wall-time consumed.
pub struct ToolRunHandle {
    tracker: ToolBudgetTracker,
    tool: String,
    started: Instant,
    committed: bool,
    process_guard: Option<crate::budget::ToolRunGuard>,
    runtime_guard: Option<wcore_budget::execution::ToolRuntimeGuard>,
    dispatch_time_limit: Option<Duration>,
}

/// Admission failure for either the process or aggregate tool-runtime axis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAdmissionError {
    pub reason: &'static str,
    pub observed: String,
    pub limit: String,
}

type ToolBudgetAdmission = (
    Option<crate::budget::ToolRunGuard>,
    Option<wcore_budget::execution::ToolRuntimeGuard>,
    Option<Duration>,
);

impl std::fmt::Display for ToolAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "budget cap '{}' would be exceeded (limit {}, observed {})",
            self.reason, self.limit, self.observed
        )
    }
}

impl std::error::Error for ToolAdmissionError {}

impl ToolBudgetTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the session/turn execution envelope used by production dispatch.
    pub fn with_execution_budget(budget: crate::budget::ExecutionBudgetView) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            execution_budget: Some(budget),
            budget_authority: None,
        }
    }

    /// Attach the sole production budget owner. Process/runtime admission and
    /// guard settlement are journaled before returning to the dispatcher.
    pub fn with_budget_authority(
        authority: crate::budget_authority::SharedBudgetAuthorityCoordinator,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            execution_budget: None,
            budget_authority: Some(authority),
        }
    }

    /// Start tracking an in-process tool invocation. Process-spawning tools
    /// must use [`Self::try_start`] so admission can fail before dispatch.
    pub fn start(&self, tool: impl Into<String>) -> ToolRunHandle {
        self.start_admitted(tool.into(), None, None, None)
    }

    /// Start tracking a classified invocation, atomically reserving a process
    /// slot and the call's maximum runtime before dispatch. A final call may
    /// receive a shorter runtime slice when that is all the aggregate envelope
    /// has left.
    pub fn try_start(
        &self,
        tool: impl Into<String>,
        class: ToolExecutionClass,
        requested_runtime: Duration,
    ) -> Result<ToolRunHandle, ToolAdmissionError> {
        let (process_guard, runtime_guard, dispatch_time_limit) =
            if let Some(authority) = self.budget_authority.as_ref() {
                authority
                    .lock()
                    .transaction(|mutation| {
                        admit_tool_run(mutation.execution(), class, requested_runtime)
                    })
                    .map_err(|error| ToolAdmissionError {
                        reason: "budget_authority",
                        observed: error.to_string(),
                        limit: "durable authority".to_owned(),
                    })??
            } else if let Some(budget) = self.execution_budget.as_ref() {
                admit_tool_run(budget, class, requested_runtime)?
            } else {
                (None, None, Some(requested_runtime))
            };
        Ok(self.start_admitted(
            tool.into(),
            process_guard,
            runtime_guard,
            dispatch_time_limit,
        ))
    }

    /// Remaining time the dispatcher may give a new tool future before the
    /// session's tool-runtime or wall-time envelope is exhausted.
    pub fn remaining_dispatch_time(&self) -> Option<Duration> {
        if let Some(authority) = self.budget_authority.as_ref() {
            return authority
                .lock()
                .inspect(|_, budget| budget.remaining_tool_dispatch_time())
                .ok()
                .flatten();
        }
        self.execution_budget
            .as_ref()
            .and_then(|budget| budget.remaining_tool_dispatch_time())
    }

    /// Monotonic deadline; convert with `tokio::time::Instant::from_std`
    /// before passing it to `tokio::time::timeout_at`.
    pub fn dispatch_deadline(&self) -> Option<Instant> {
        if let Some(authority) = self.budget_authority.as_ref() {
            return authority
                .lock()
                .inspect(|_, budget| budget.tool_dispatch_deadline())
                .ok()
                .flatten();
        }
        self.execution_budget
            .as_ref()
            .and_then(|budget| budget.tool_dispatch_deadline())
    }

    fn start_admitted(
        &self,
        tool: String,
        process_guard: Option<crate::budget::ToolRunGuard>,
        runtime_guard: Option<wcore_budget::execution::ToolRuntimeGuard>,
        dispatch_time_limit: Option<Duration>,
    ) -> ToolRunHandle {
        {
            let mut inner = self.inner.lock();
            let entry = inner.entry(tool.clone()).or_default();
            entry.calls = entry.calls.saturating_add(1);
        }
        ToolRunHandle {
            tracker: self.clone(),
            tool,
            started: Instant::now(),
            committed: false,
            process_guard,
            runtime_guard,
            dispatch_time_limit,
        }
    }

    /// Snapshot of usage for `tool`. Returns `ToolUsage::default()` if
    /// the tool has never been seen.
    pub fn usage_for(&self, tool: &str) -> ToolUsage {
        self.inner.lock().get(tool).copied().unwrap_or_default()
    }

    /// Aggregate snapshot across every tool seen so far.
    pub fn all_usage(&self) -> HashMap<String, ToolUsage> {
        self.inner.lock().clone()
    }

    /// Total runtime across every recorded tool call.
    pub fn total_runtime(&self) -> Duration {
        self.inner
            .lock()
            .values()
            .map(|u| u.total_runtime)
            .sum::<Duration>()
    }
}

fn admit_tool_run(
    budget: &crate::budget::ExecutionBudgetView,
    class: ToolExecutionClass,
    requested_runtime: Duration,
) -> Result<ToolBudgetAdmission, ToolAdmissionError> {
    let process_guard = match class {
        ToolExecutionClass::ProcessSpawning => Some(budget.try_enter_process().map_err(
            |error| ToolAdmissionError {
                reason: error.reason,
                observed: error.observed.to_string(),
                limit: error.limit.to_string(),
            },
        )?),
        ToolExecutionClass::InProcess => None,
    };
    let requested_runtime = budget
        .remaining_wall_time()
        .map_or(requested_runtime, |remaining| {
            requested_runtime.min(remaining)
        });
    let runtime_guard = budget
        .try_reserve_tool_runtime(requested_runtime)
        .map_err(|error| ToolAdmissionError {
            reason: error.reason,
            observed: format!("{:.3}s", error.observed.as_secs_f64()),
            limit: format!("{:.3}s", error.limit.as_secs_f64()),
        })?;
    let admitted = runtime_guard.admitted_runtime();
    Ok((process_guard, Some(runtime_guard), Some(admitted)))
}

impl ToolRunHandle {
    /// Maximum wall-clock time admitted for this invocation after applying
    /// category, session wall-time, and aggregate tool-runtime limits.
    pub fn dispatch_time_limit(&self) -> Option<Duration> {
        self.dispatch_time_limit
    }

    /// Explicitly commit the elapsed runtime. Idempotent — repeated calls
    /// are no-ops. Useful when the caller wants the runtime accounted
    /// for *before* the guard goes out of scope.
    pub fn commit(&mut self) {
        if self.committed {
            return;
        }
        let elapsed = self.started.elapsed();
        let mut inner = self.tracker.inner.lock();
        let entry = inner.entry(self.tool.clone()).or_default();
        entry.total_runtime = entry.total_runtime.saturating_add(elapsed);
        drop(inner);
        if let Some(authority) = self.tracker.budget_authority.as_ref() {
            let mut runtime_guard = self.runtime_guard.take();
            let process_guard = self.process_guard.take();
            if let Err(error) = authority.lock().transaction(|_| {
                if let Some(runtime_guard) = runtime_guard.as_mut() {
                    runtime_guard.settle(elapsed);
                }
                drop(process_guard);
            }) {
                tracing::error!(
                    error = %error,
                    tool = %self.tool,
                    "durable tool budget settlement failed"
                );
            }
        } else if let Some(runtime_guard) = self.runtime_guard.as_mut() {
            runtime_guard.settle(elapsed);
        } else if let Some(budget) = self.tracker.execution_budget.as_ref() {
            budget.record_tool_runtime(elapsed);
        }
        self.runtime_guard.take();
        self.process_guard.take();
        self.committed = true;
    }
}

impl Drop for ToolRunHandle {
    fn drop(&mut self) {
        self.commit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::thread::sleep;

    fn durable_tracker() -> (
        tempfile::TempDir,
        crate::session_journal::SessionJournal,
        crate::budget_authority::SharedBudgetAuthorityCoordinator,
        ToolBudgetTracker,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let journal = crate::session_journal::SessionJournal::open(
            dir.path().join("session.journal"),
            "session",
        )
        .unwrap();
        let session = json!({
            "id": "session",
            "schema_version": 1,
            "messages": [],
        });
        journal
            .append(crate::session_journal::SessionEvent::SessionImported {
                source_schema_version: 1,
                session_digest: crate::session_journal::state_payload_digest(&session).unwrap(),
                session,
            })
            .unwrap();
        let authority = crate::budget_authority::BudgetAuthorityCoordinator::bind(
            crate::budget_authority::BudgetAuthorityConfig {
                journal: Some(journal.clone()),
                budget_session_id: "session-budget".to_owned(),
                provider_caps: wcore_budget::BudgetCap::default(),
                execution_policy: crate::budget::ExecutionBudget {
                    max_tool_runtime: Some(Duration::from_secs(1)),
                    max_processes: Some(1),
                    ..Default::default()
                },
                wall_clock: crate::session_journal::BudgetWallClockAuthority::ActiveRuntime,
                process_cleanup_proof: None,
            },
        )
        .unwrap()
        .into_shared();
        let tracker = ToolBudgetTracker::with_budget_authority(Arc::clone(&authority));
        (dir, journal, authority, tracker)
    }

    #[test]
    fn empty_tracker_returns_default_usage() {
        let t = ToolBudgetTracker::new();
        let u = t.usage_for("Read");
        assert_eq!(u.calls, 0);
        assert_eq!(u.total_runtime, Duration::ZERO);
    }

    #[test]
    fn start_increments_call_count_immediately() {
        let t = ToolBudgetTracker::new();
        let _h = t.start("Read");
        assert_eq!(t.usage_for("Read").calls, 1);
    }

    #[test]
    fn drop_commits_runtime() {
        let t = ToolBudgetTracker::new();
        {
            let _h = t.start("Bash");
            sleep(Duration::from_millis(10));
        }
        let u = t.usage_for("Bash");
        assert_eq!(u.calls, 1);
        assert!(
            u.total_runtime >= Duration::from_millis(8),
            "expected ≈10ms runtime, got: {:?}",
            u.total_runtime
        );
    }

    #[test]
    fn explicit_commit_is_idempotent() {
        let t = ToolBudgetTracker::new();
        let mut h = t.start("Write");
        sleep(Duration::from_millis(5));
        h.commit();
        let first = t.usage_for("Write").total_runtime;
        sleep(Duration::from_millis(5));
        h.commit(); // no-op
        drop(h);
        let second = t.usage_for("Write").total_runtime;
        assert_eq!(first, second, "commit after first must be idempotent");
    }

    #[test]
    fn multiple_calls_aggregate_per_tool() {
        let t = ToolBudgetTracker::new();
        for _ in 0..3 {
            let _h = t.start("Grep");
        }
        assert_eq!(t.usage_for("Grep").calls, 3);
    }

    #[test]
    fn all_usage_returns_every_recorded_tool() {
        let t = ToolBudgetTracker::new();
        let _a = t.start("Read");
        let _b = t.start("Write");
        let snapshot = t.all_usage();
        assert!(snapshot.contains_key("Read"));
        assert!(snapshot.contains_key("Write"));
    }

    #[test]
    fn in_process_tools_do_not_consume_process_slots() {
        let budget = crate::budget::ExecutionBudget {
            max_processes: Some(0),
            ..Default::default()
        }
        .start_root();
        let tracker = ToolBudgetTracker::with_execution_budget(budget.clone());

        let handle = tracker.start("Read");
        assert!(!budget.is_exceeded());
        drop(handle);
        assert_eq!(tracker.usage_for("Read").calls, 1);
    }

    #[test]
    fn process_admission_refuses_before_counting_the_call() {
        let budget = crate::budget::ExecutionBudget {
            max_processes: Some(0),
            ..Default::default()
        }
        .start_root();
        let tracker = ToolBudgetTracker::with_execution_budget(budget);

        let error = match tracker.try_start(
            "Bash",
            ToolExecutionClass::ProcessSpawning,
            Duration::from_secs(1),
        ) {
            Ok(_) => panic!("cap zero must reject process-spawning tools"),
            Err(error) => error,
        };
        assert_eq!(error.reason, "max_concurrent_process_tools");
        assert_eq!(tracker.usage_for("Bash").calls, 0);
    }

    #[test]
    fn process_guard_releases_slot_on_drop() {
        let budget = crate::budget::ExecutionBudget {
            max_processes: Some(1),
            ..Default::default()
        }
        .start_root();
        let tracker = ToolBudgetTracker::with_execution_budget(budget);

        let first = tracker
            .try_start(
                "Bash",
                ToolExecutionClass::ProcessSpawning,
                Duration::from_secs(1),
            )
            .expect("first slot is available");
        assert!(
            tracker
                .try_start(
                    "Script",
                    ToolExecutionClass::ProcessSpawning,
                    Duration::from_secs(1),
                )
                .is_err()
        );
        drop(first);
        let second = tracker
            .try_start(
                "Script",
                ToolExecutionClass::ProcessSpawning,
                Duration::from_secs(1),
            )
            .expect("dropping the guard releases the slot");
        drop(second);
    }

    #[test]
    fn durable_process_and_runtime_guards_commit_admission_and_release() {
        let (_dir, journal, authority, tracker) = durable_tracker();
        let initial = journal.state().unwrap().budget_authority.unwrap();

        let handle = tracker
            .try_start(
                "Bash",
                ToolExecutionClass::ProcessSpawning,
                Duration::from_millis(100),
            )
            .expect("durable authority admits the first process");
        let admitted = journal.state().unwrap().budget_authority.unwrap();
        assert_eq!(admitted.authority_epoch, initial.authority_epoch + 1);
        let admitted_view =
            crate::budget::ExecutionBudgetView::from_snapshot(admitted.execution_root).unwrap();
        assert_eq!(
            admitted_view.observed_for("max_concurrent_process_tools"),
            "1"
        );

        sleep(Duration::from_millis(5));
        drop(handle);
        let settled = journal.state().unwrap().budget_authority.unwrap();
        assert_eq!(settled.authority_epoch, initial.authority_epoch + 2);
        let settled_view =
            crate::budget::ExecutionBudgetView::from_snapshot(settled.execution_root.clone())
                .unwrap();
        assert_eq!(
            settled_view.observed_for("max_concurrent_process_tools"),
            "0"
        );
        assert_ne!(settled.execution_root, initial.execution_root);
        assert_eq!(authority.lock().authority_epoch(), settled.authority_epoch);
    }

    #[test]
    fn concurrent_runtime_reservation_limits_each_dispatch() {
        let budget = crate::budget::ExecutionBudget {
            max_tool_runtime: Some(Duration::from_millis(50)),
            ..Default::default()
        }
        .start_root();
        let tracker = ToolBudgetTracker::with_execution_budget(budget.clone());

        let first = tracker
            .try_start(
                "Read",
                ToolExecutionClass::InProcess,
                Duration::from_secs(30),
            )
            .expect("first call reserves the remaining aggregate runtime");
        assert_eq!(first.dispatch_time_limit(), Some(Duration::from_millis(50)));
        let error = match tracker.try_start(
            "Grep",
            ToolExecutionClass::InProcess,
            Duration::from_secs(30),
        ) {
            Ok(_) => panic!("a sibling cannot reserve the same remaining runtime"),
            Err(error) => error,
        };
        assert_eq!(error.reason, "max_tool_runtime");
        assert_eq!(tracker.usage_for("Grep").calls, 0);

        drop(first);
        let second = tracker
            .try_start(
                "Grep",
                ToolExecutionClass::InProcess,
                Duration::from_secs(30),
            )
            .expect("settling the first call refunds its unused reservation");
        drop(second);
    }

    #[test]
    fn tracker_exposes_preemptive_runtime_deadline() {
        let budget = crate::budget::ExecutionBudget {
            max_wall_time: Some(Duration::from_secs(1)),
            max_tool_runtime: Some(Duration::from_millis(50)),
            ..Default::default()
        }
        .start_root();
        budget.record_tool_runtime(Duration::from_millis(20));
        let tracker = ToolBudgetTracker::with_execution_budget(budget);

        assert_eq!(
            tracker.remaining_dispatch_time(),
            Some(Duration::from_millis(30))
        );
        let before = Instant::now();
        let deadline = tracker.dispatch_deadline().expect("deadline is capped");
        assert!(deadline >= before + Duration::from_millis(30));
    }
}
