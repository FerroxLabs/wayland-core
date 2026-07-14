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
//! `AgentEngine::run` owns one monitor per run and consumes its decisions at
//! provider and tool-result boundaries. The graph walker remains a separate
//! execution surface; it shares the same budget view through its context.

use std::collections::VecDeque;

use crate::budget::ExecutionBudgetView;

/// How many recent error signatures the monitor remembers.
const WINDOW_LEN: usize = 8;

/// How many consecutive identical signatures trip
/// [`MonitorAction::ReplanRepeatedError`].
const REPEAT_THRESHOLD: usize = 3;

/// Consecutive failed provider attempts with a failed tool round and no output
/// before another identical full-context retry is considered wasteful.
const OUTPUT_STALL_THRESHOLD: u32 = 2;

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
    /// Repeated provider attempts produced no output while retrying a context
    /// whose latest tool round had already failed.
    StopOutputStall,
}

pub struct MidFlightMonitor {
    budget: ExecutionBudgetView,
    /// Most recent error signatures, oldest at front, newest at back.
    /// Capacity-bounded to `WINDOW_LEN`.
    recent_errors: VecDeque<String>,
    consecutive_output_stalls: u32,
}

impl MidFlightMonitor {
    /// Build a monitor bound to a budget view. The view is shared
    /// (cheap to clone); the monitor reads it via `is_exceeded` /
    /// `first_exceeded_reason`.
    pub fn new(budget: ExecutionBudgetView) -> Self {
        Self {
            budget,
            recent_errors: VecDeque::with_capacity(WINDOW_LEN),
            consecutive_output_stalls: 0,
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

    /// Record whether a provider attempt made progress. Only an attempt that
    /// both carried a failed tool round and produced no output counts as a
    /// stall; any output resets the consecutive streak.
    pub fn record_stream_attempt(&mut self, failed_tool_round: bool, produced_output: bool) {
        if failed_tool_round && !produced_output {
            self.consecutive_output_stalls = self.consecutive_output_stalls.saturating_add(1);
        } else {
            self.consecutive_output_stalls = 0;
        }
    }

    /// Inspect the current state and return the next action. Cheap;
    /// call once per turn / graph tick.
    pub fn tick(&mut self) -> MonitorAction {
        if let Some(reason) = self.budget.first_exceeded_reason() {
            return MonitorAction::CancelBudget { reason };
        }
        if self.consecutive_output_stalls >= OUTPUT_STALL_THRESHOLD {
            return MonitorAction::StopOutputStall;
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
                // The caller injects a changed-strategy instruction into the
                // next model turn. Consume this streak so a later unrelated
                // error does not immediately re-trigger the same replan.
                self.recent_errors.clear();
                return MonitorAction::ReplanRepeatedError;
            }
        }
        MonitorAction::Continue
    }

    /// Reduce a free-text error message to a stable root-cause signature.
    /// Directory prefixes and volatile counters are removed, while error/status
    /// codes and resource basenames remain distinct so unrelated failures do
    /// not collapse into one replan trigger.
    ///
    /// Two errors that differ only by volatile fields will produce the
    /// same signature.
    pub fn root_cause_signature(message: &str) -> String {
        let mut out = String::with_capacity(message.len());
        let mut previous_token = None::<String>;
        for raw_token in message.split_whitespace() {
            let trimmed = raw_token.trim_end_matches(|c: char| ",;:.".contains(c));
            let token = if trimmed.contains('/') || trimmed.contains('\\') {
                let parts = trimmed
                    .split(['/', '\\'])
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                match (parts.first(), parts.last()) {
                    (Some(scope), Some(resource)) if scope != resource => {
                        // Keep the stable scope and resource identity while
                        // dropping volatile intermediate directories.
                        // `/etc/config.json` and `/tmp/config.json` must not
                        // collapse into the same failure.
                        let resource = if scope.eq_ignore_ascii_case("tmp")
                            && resource.chars().any(|character| character.is_ascii_digit())
                        {
                            resource.rfind('.').map_or_else(
                                || "<volatile>".to_string(),
                                |dot| format!("<volatile>{}", &resource[dot..]),
                            )
                        } else {
                            (*resource).to_string()
                        };
                        format!("{scope}/{resource}")
                    }
                    (_, Some(resource)) => (*resource).to_string(),
                    _ => "<path>".to_string(),
                }
            } else {
                trimmed.to_string()
            };
            if token.chars().all(|c| c.is_ascii_digit()) {
                let is_contextual_status = token.len() == 3
                    && token
                        .parse::<u16>()
                        .is_ok_and(|status| (100..=599).contains(&status))
                    && previous_token.as_deref().is_some_and(|previous| {
                        matches!(previous, "http" | "status" | "status_code")
                    });
                if !is_contextual_status {
                    continue;
                }
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&token);
            previous_token = Some(token.to_ascii_lowercase());
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
        let a = MidFlightMonitor::root_cause_signature(
            "ENOENT at /tmp/run-abc/foo.txt line 12 byte 8192",
        );
        let b = MidFlightMonitor::root_cause_signature(
            "ENOENT at /tmp/run-def/foo.txt line 7 byte 4096",
        );
        assert_eq!(a, b);
    }

    #[test]
    fn signature_preserves_status_codes_and_resource_identity() {
        let unauthorized = MidFlightMonitor::root_cause_signature("HTTP 401 for /tmp/api.json");
        let server_error = MidFlightMonitor::root_cause_signature("HTTP 500 for /tmp/api.json");
        let other_resource = MidFlightMonitor::root_cause_signature("HTTP 401 for /tmp/auth.json");
        assert_ne!(unauthorized, server_error);
        assert_ne!(unauthorized, other_resource);
    }

    #[test]
    fn signature_does_not_confuse_line_numbers_with_http_statuses() {
        let a = MidFlightMonitor::root_cause_signature("parse failed at line 401");
        let b = MidFlightMonitor::root_cause_signature("parse failed at line 402");
        assert_eq!(a, b);

        let unauthorized = MidFlightMonitor::root_cause_signature("HTTP 401 from /tmp/api.json");
        let forbidden = MidFlightMonitor::root_cause_signature("HTTP 403 from /tmp/api.json");
        assert_ne!(unauthorized, forbidden);
    }

    #[test]
    fn signature_preserves_path_scope() {
        let system = MidFlightMonitor::root_cause_signature("denied /etc/config.json");
        let temporary = MidFlightMonitor::root_cause_signature("denied /tmp/config.json");
        assert_ne!(system, temporary);
    }

    #[test]
    fn output_stall_requires_two_consecutive_empty_failed_rounds() {
        let view = ExecutionBudget::default().start_root();
        let mut mon = MidFlightMonitor::new(view);
        mon.record_stream_attempt(true, false);
        assert_eq!(mon.tick(), MonitorAction::Continue);
        mon.record_stream_attempt(false, true);
        assert_eq!(mon.tick(), MonitorAction::Continue);
        mon.record_stream_attempt(true, false);
        mon.record_stream_attempt(true, false);
        assert_eq!(mon.tick(), MonitorAction::StopOutputStall);
    }

    #[test]
    fn repeated_error_replan_consumes_the_triggering_streak() {
        let view = ExecutionBudget::default().start_root();
        let mut mon = MidFlightMonitor::new(view);
        for _ in 0..REPEAT_THRESHOLD {
            mon.record_error("permission denied at /tmp/work item 42");
        }
        assert_eq!(mon.tick(), MonitorAction::ReplanRepeatedError);
        assert_eq!(mon.tick(), MonitorAction::Continue);
    }
}
