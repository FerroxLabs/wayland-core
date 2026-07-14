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

const ROUTE_REPEAT_THRESHOLD: usize = 3;
const MAX_ROUTE_CYCLE_LEN: usize = 4;
const ROUTE_WINDOW_LEN: usize = ROUTE_REPEAT_THRESHOLD * MAX_ROUTE_CYCLE_LEN;

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
    /// A normalized tool/outcome route is cycling; inject a changed-strategy
    /// directive before allowing another provider turn.
    ReplanRepeatedRoute,
    /// The same normalized route repeated after a replan directive.
    StopRepeatedRoute,
    /// Repeated provider attempts produced no output while retrying a context
    /// whose latest tool round had already failed.
    StopOutputStall,
}

pub struct MidFlightMonitor {
    budget: ExecutionBudgetView,
    /// Most recent error signatures, oldest at front, newest at back.
    /// Capacity-bounded to `WINDOW_LEN`.
    recent_errors: VecDeque<String>,
    recent_tool_outcomes: VecDeque<String>,
    last_replanned_route: Option<Vec<String>>,
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
            recent_tool_outcomes: VecDeque::with_capacity(ROUTE_WINDOW_LEN),
            last_replanned_route: None,
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

    /// Record one tool action and normalized outcome. Semantic input values
    /// remain exact, while explicitly volatile fields (timestamps, request
    /// IDs, nonces, and similar correlation data) are replaced. This exposes
    /// repeated routes without collapsing productive argument changes.
    pub fn record_tool_outcome(
        &mut self,
        tool_name: &str,
        input: &serde_json::Value,
        is_error: bool,
        outcome: &str,
    ) {
        let signature = format!(
            "{tool_name}|{}|{}|{}",
            normalized_action_json(input, None),
            if is_error { "error" } else { "success" },
            Self::root_cause_signature(outcome)
        );
        if let Some(route) = &self.last_replanned_route {
            let expected = &route[self.recent_tool_outcomes.len() % route.len()];
            if &signature != expected {
                // The agent materially deviated from the route after the
                // replan. A later return to the old route is a new incident,
                // not proof that the earlier directive was ignored.
                self.last_replanned_route = None;
            }
        }
        if self.recent_tool_outcomes.len() == ROUTE_WINDOW_LEN {
            self.recent_tool_outcomes.pop_front();
        }
        self.recent_tool_outcomes.push_back(signature);
    }

    /// Provider-retry boundaries consume only budget/stall signals. Route and
    /// repeated-error decisions belong to the committed tool-result boundary;
    /// consuming them during a provider retry would misattribute ownership.
    pub fn tick_provider(&mut self) -> MonitorAction {
        if let Some(reason) = self.budget.first_exceeded_reason() {
            return MonitorAction::CancelBudget { reason };
        }
        if self.consecutive_output_stalls >= OUTPUT_STALL_THRESHOLD {
            return MonitorAction::StopOutputStall;
        }
        MonitorAction::Continue
    }

    /// Inspect the current state and return the next action. Cheap;
    /// call once per turn / graph tick.
    pub fn tick(&mut self) -> MonitorAction {
        let provider_action = self.tick_provider();
        if provider_action != MonitorAction::Continue {
            return provider_action;
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
        if let Some(route) = self.repeated_route() {
            self.recent_tool_outcomes.clear();
            if self.last_replanned_route.as_ref() == Some(&route) {
                self.last_replanned_route = None;
                return MonitorAction::StopRepeatedRoute;
            }
            self.last_replanned_route = Some(route);
            return MonitorAction::ReplanRepeatedRoute;
        }
        MonitorAction::Continue
    }

    fn repeated_route(&self) -> Option<Vec<String>> {
        // A one-step "cycle" is ambiguous: a tool may legitimately iterate
        // while returning changing progress. Existing LoopGuard already owns
        // exact one-call repetition. Route detection therefore starts at two
        // distinct normalized steps and targets A/B (or longer) oscillation.
        for cycle_len in 2..=MAX_ROUTE_CYCLE_LEN {
            let repeated_len = cycle_len * ROUTE_REPEAT_THRESHOLD;
            if self.recent_tool_outcomes.len() < repeated_len {
                continue;
            }
            let tail = self
                .recent_tool_outcomes
                .iter()
                .skip(self.recent_tool_outcomes.len() - repeated_len)
                .cloned()
                .collect::<Vec<_>>();
            let cycle = &tail[..cycle_len];
            if cycle.windows(2).any(|pair| pair[0] != pair[1])
                && tail.chunks_exact(cycle_len).all(|chunk| chunk == cycle)
            {
                return Some(cycle.to_vec());
            }
        }
        None
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
            let token = if let Some((scheme, remainder)) = trimmed.split_once("://") {
                let authority = remainder.split('/').next().unwrap_or(remainder);
                let resource = remainder.rsplit('/').next().unwrap_or(remainder);
                format!("{scheme}://{authority}/{resource}")
            } else if trimmed.contains('/') || trimmed.contains('\\') {
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
                            && looks_like_volatile_temp_resource(resource)
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
                            || previous.starts_with("http/")
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

fn looks_like_volatile_temp_resource(resource: &str) -> bool {
    let stem = resource.rsplit_once('.').map_or(resource, |(stem, _)| stem);
    stem.rsplit_once('-').is_some_and(|(_, suffix)| {
        suffix.len() >= 4 && suffix.chars().all(|character| character.is_ascii_digit())
    })
}

fn normalized_action_json(value: &serde_json::Value, key: Option<&str>) -> String {
    if key.is_some_and(is_volatile_action_key) {
        return "\"<volatile>\"".to_string();
    }
    match value {
        serde_json::Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let body = entries
                .into_iter()
                .map(|(key, value)| format!("{key}:{}", normalized_action_json(value, Some(key))))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| normalized_action_json(value, key))
                .collect::<Vec<_>>()
                .join(",")
        ),
        serde_json::Value::String(value) if looks_like_uuid(value) => "\"<uuid>\"".to_string(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn is_volatile_action_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace('-', "_").as_str(),
        "timestamp"
            | "timestamp_ms"
            | "request_id"
            | "correlation_id"
            | "trace_id"
            | "span_id"
            | "nonce"
            | "pid"
            | "created_at"
            | "updated_at"
    )
}

fn looks_like_uuid(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .enumerate()
            .all(|(index, character)| match index {
                8 | 13 | 18 | 23 => character == '-',
                _ => character.is_ascii_hexdigit(),
            })
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

        let http_unauthorized =
            MidFlightMonitor::root_cause_signature("HTTP/1.1 401 from /tmp/api.json");
        let http_server_error =
            MidFlightMonitor::root_cause_signature("HTTP/1.1 500 from /tmp/api.json");
        assert_ne!(http_unauthorized, http_server_error);
    }

    #[test]
    fn signature_preserves_path_scope() {
        let system = MidFlightMonitor::root_cause_signature("denied /etc/config.json");
        let temporary = MidFlightMonitor::root_cause_signature("denied /tmp/config.json");
        assert_ne!(system, temporary);

        let schema_v1 = MidFlightMonitor::root_cause_signature("invalid /tmp/schema-v1.json");
        let schema_v2 = MidFlightMonitor::root_cause_signature("invalid /tmp/schema-v2.json");
        assert_ne!(schema_v1, schema_v2);
    }

    #[test]
    fn signature_preserves_url_authority() {
        let api_a = MidFlightMonitor::root_cause_signature(
            "request failed at https://api-a.example/v1/messages",
        );
        let api_b = MidFlightMonitor::root_cause_signature(
            "request failed at https://api-b.example/v1/messages",
        );
        assert_ne!(api_a, api_b);
        assert!(api_a.contains("https://api-a.example/messages"));
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

    #[test]
    fn repeated_alternating_route_replans_then_stops_if_ignored() {
        let view = ExecutionBudget::default().start_root();
        let mut mon = MidFlightMonitor::new(view);
        let input_a = serde_json::json!({"path": "a"});
        let input_b = serde_json::json!({"path": "b"});

        for pass in 0..ROUTE_REPEAT_THRESHOLD {
            mon.record_tool_outcome(
                "Read",
                &input_a,
                false,
                &format!("unchanged at line {}", 100 + pass),
            );
            mon.record_tool_outcome(
                "Grep",
                &input_b,
                false,
                &format!("no match at byte {}", 200 + pass),
            );
        }
        assert_eq!(mon.tick(), MonitorAction::ReplanRepeatedRoute);

        for pass in 0..ROUTE_REPEAT_THRESHOLD {
            mon.record_tool_outcome(
                "Read",
                &input_a,
                false,
                &format!("unchanged at line {}", 300 + pass),
            );
            mon.record_tool_outcome(
                "Grep",
                &input_b,
                false,
                &format!("no match at byte {}", 400 + pass),
            );
        }
        assert_eq!(mon.tick(), MonitorAction::StopRepeatedRoute);
    }

    #[test]
    fn progress_expires_pending_route_before_a_later_recurrence() {
        let view = ExecutionBudget::default().start_root();
        let mut mon = MidFlightMonitor::new(view);
        let input = serde_json::json!({"path": "stable"});
        for _ in 0..ROUTE_REPEAT_THRESHOLD {
            mon.record_tool_outcome("Read", &input, false, "same read");
            mon.record_tool_outcome("Grep", &input, false, "same grep");
        }
        assert_eq!(mon.tick(), MonitorAction::ReplanRepeatedRoute);

        mon.record_tool_outcome("Edit", &input, false, "materially changed file");
        for _ in 0..ROUTE_REPEAT_THRESHOLD {
            mon.record_tool_outcome("Read", &input, false, "same read");
            mon.record_tool_outcome("Grep", &input, false, "same grep");
        }
        assert_eq!(
            mon.tick(),
            MonitorAction::ReplanRepeatedRoute,
            "productive deviation makes a later recurrence a new replan, not an immediate stop"
        );
    }

    #[test]
    fn volatile_action_fields_normalize_but_semantic_arguments_do_not() {
        let view = ExecutionBudget::default().start_root();
        let mut mon = MidFlightMonitor::new(view);
        for pass in 0..ROUTE_REPEAT_THRESHOLD {
            mon.record_tool_outcome(
                "Read",
                &serde_json::json!({"path": "a", "request_id": format!("req-{pass}")}),
                false,
                "same read",
            );
            mon.record_tool_outcome(
                "Grep",
                &serde_json::json!({"query": "needle", "timestamp_ms": 1000 + pass}),
                false,
                "same grep",
            );
        }
        assert_eq!(mon.tick(), MonitorAction::ReplanRepeatedRoute);

        let view = ExecutionBudget::default().start_root();
        let mut productive = MidFlightMonitor::new(view);
        for pass in 0..ROUTE_REPEAT_THRESHOLD {
            productive.record_tool_outcome(
                "Read",
                &serde_json::json!({"path": format!("file-{pass}.rs")}),
                false,
                "ok",
            );
            productive.record_tool_outcome(
                "Grep",
                &serde_json::json!({"query": format!("needle-{pass}")}),
                false,
                "ok",
            );
        }
        assert_eq!(productive.tick(), MonitorAction::Continue);
    }

    #[test]
    fn different_arguments_do_not_collapse_into_one_route() {
        let view = ExecutionBudget::default().start_root();
        let mut mon = MidFlightMonitor::new(view);
        for line in 1..=ROUTE_REPEAT_THRESHOLD {
            mon.record_tool_outcome("Read", &serde_json::json!({"line": line}), false, "ok");
        }
        assert_eq!(mon.tick(), MonitorAction::Continue);
    }
}
