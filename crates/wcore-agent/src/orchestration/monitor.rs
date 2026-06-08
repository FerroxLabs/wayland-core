//! W8b.2.B Task C.4 — `MidFlightMonitor`.
//!
//! Watches two signals during a graph run:
//!
//! 1. **Budget exhaustion** via [`ExecutionBudgetView::is_exceeded`]. On
//!    exceed the monitor reports [`MonitorAction::CancelBudget`] with
//!    the first-exceeded reason; the graph walker (or its parent loop)
//!    is expected to cancel the [`tokio_util::sync::CancellationToken`]
//!    on the [`super::graph::GraphContext`].
//! 2. **Repeated tool errors** — a sliding window of the last
//!    `WINDOW_LEN` error signatures. If the most recent
//!    `REPEAT_THRESHOLD` entries share the same root-cause signature,
//!    the monitor reports [`MonitorAction::ReplanRepeatedError`].
//!
//! The walker for this sub-wave doesn't yet plug into the monitor —
//! that wiring lands when the main loop calls `ExecutionGraph::execute`
//! (Task C.5 / W8b.2.B.1). This module is otherwise standalone and
//! testable in isolation.

use std::collections::VecDeque;

use crate::budget::ExecutionBudgetView;

/// How many recent error signatures the monitor remembers.
const WINDOW_LEN: usize = 8;

/// How many consecutive identical signatures trip
/// [`MonitorAction::ReplanRepeatedError`].
const REPEAT_THRESHOLD: usize = 3;

/// Decision emitted by [`MidFlightMonitor::tick`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorAction {
    /// Nothing to do; keep running.
    Continue,
    /// Budget has exceeded a cap; the walker should cancel.
    CancelBudget {
        /// Which cap tripped first (`"max_tokens_in"`, `"max_wall_time"`, …).
        reason: &'static str,
    },
    /// The last `REPEAT_THRESHOLD` errors collapsed to the same root
    /// cause. The walker should stop and ask its parent to replan.
    ReplanRepeatedError,
}

pub struct MidFlightMonitor {
    budget: ExecutionBudgetView,
    /// Most recent error signatures, oldest at front, newest at back.
    /// Capacity-bounded to `WINDOW_LEN`.
    recent_errors: VecDeque<String>,
}

impl MidFlightMonitor {
    /// Build a monitor bound to a budget view. The view is shared
    /// (cheap to clone); the monitor reads it via `is_exceeded` /
    /// `first_exceeded_reason`.
    pub fn new(budget: ExecutionBudgetView) -> Self {
        Self {
            budget,
            recent_errors: VecDeque::with_capacity(WINDOW_LEN),
        }
    }

    /// Record an error from a tool/agent call. Only the root-cause
    /// signature is retained; volatile fields (paths, byte offsets,
    /// line numbers, PIDs, timestamps) are stripped via
    /// [`Self::root_cause_signature`].
    pub fn record_error(&mut self, message: &str) {
        let sig = Self::root_cause_signature(message);
        if self.recent_errors.len() == WINDOW_LEN {
            self.recent_errors.pop_front();
        }
        self.recent_errors.push_back(sig);
    }

    /// Inspect the current state and return the next action. Cheap;
    /// call once per turn / graph tick.
    pub fn tick(&mut self) -> MonitorAction {
        if let Some(reason) = self.budget.first_exceeded_reason() {
            return MonitorAction::CancelBudget { reason };
        }
        // Replan when the last REPEAT_THRESHOLD entries collapse to a
        // single signature (i.e. the agent is hitting the same wall
        // repeatedly).
        if self.recent_errors.len() >= REPEAT_THRESHOLD {
            let tail = self
                .recent_errors
                .iter()
                .rev()
                .take(REPEAT_THRESHOLD)
                .collect::<Vec<_>>();
            if tail.windows(2).all(|w| w[0] == w[1]) {
                return MonitorAction::ReplanRepeatedError;
            }
        }
        MonitorAction::Continue
    }

    /// Reduce a free-text error message to a stable root-cause
    /// signature. Strips:
    ///   * absolute and relative paths (any token containing a `/`)
    ///   * decimal numbers (line/byte/PID/timestamp counters)
    ///   * trailing punctuation
    ///
    /// Two errors that differ only by volatile fields will produce the
    /// same signature.
    pub fn root_cause_signature(message: &str) -> String {
        let mut out = String::with_capacity(message.len());
        for token in message.split_whitespace() {
            // Drop path-like tokens (contain `/` or `\`).
            if token.contains('/') || token.contains('\\') {
                continue;
            }
            // Drop pure-number tokens (line/byte counts, PIDs).
            if token
                .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.')
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit())
            {
                // Skip if the token *starts* with a digit.
                continue;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(token.trim_end_matches(|c: char| ",;:.".contains(c)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::ExecutionBudget;

    #[test]
    fn record_error_caps_window_at_capacity() {
        let view = ExecutionBudget::default().start_root();
        let mut mon = MidFlightMonitor::new(view);
        for i in 0..(WINDOW_LEN + 5) {
            mon.record_error(&format!("err {i}"));
        }
        assert_eq!(mon.recent_errors.len(), WINDOW_LEN);
    }

    #[test]
    fn signature_collapses_paths_and_numbers() {
        let a = MidFlightMonitor::root_cause_signature("ENOENT at /tmp/foo.txt line 12 byte 8192");
        let b = MidFlightMonitor::root_cause_signature("ENOENT at /var/bar.log line 7 byte 4096");
        assert_eq!(a, b);
    }
}
