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
//! Usage from the dispatcher (call site to be wired by orchestration in
//! a follow-up; this module ships the API + tests):
//!
//! ```ignore
//! let tracker = ToolBudgetTracker::new();
//! let guard = tracker.start(tool_name);
//! let result = tool.execute_with_ctx(input, &ctx).await;
//! drop(guard);  // records elapsed
//! let usage = tracker.usage_for(tool_name);
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Per-tool runtime + call counts. Cheap to clone (Arc-backed).
#[derive(Clone, Default)]
pub struct ToolBudgetTracker {
    inner: Arc<Mutex<HashMap<String, ToolUsage>>>,
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
}

impl ToolBudgetTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking a tool invocation. Returns a guard that records
    /// the elapsed runtime on drop. The call-count is incremented
    /// immediately so `usage_for` shows the call as in-flight.
    pub fn start(&self, tool: impl Into<String>) -> ToolRunHandle {
        let tool = tool.into();
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

impl ToolRunHandle {
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
    use std::thread::sleep;

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
}
