//! v0.9.3 W6 — stale-watchdog (Sec-H2 mitigation for ChannelSink try_send drop-on-full).
//!
//! v1.1 LOW-1 pin: `check` is STATELESS — it inspects `App::agent_last_event`
//! plus `App::session.sub_agents` and returns the TOTAL current count of
//! Running agents whose last-event-at exceeds the staleness window. No
//! mutation of `App` is required (the off-band map is the only state). This
//! keeps the SubAgentView contract FROZEN per app.rs:582-584.

use std::time::{Duration, Instant};

use crate::tui::app::{App, SubAgentStatus};

/// Threshold above which a Running sub-agent is considered stale (10 min).
const STALE_THRESHOLD: Duration = Duration::from_secs(600);

pub struct StaleWatchdog;

impl StaleWatchdog {
    /// Returns the current total count of stale Running sub-agents.
    ///
    /// Stateless — recomputed on demand from `App::agent_last_event`. An
    /// agent is stale iff:
    ///   - its `SubAgentStatus` is `Running`, AND
    ///   - its last-event timestamp is older than `STALE_THRESHOLD`.
    ///
    /// Agents with no recorded last-event entry are NOT counted as stale
    /// (they have not yet produced any event — likely just spawned).
    pub fn check(app: &App, now: Instant) -> usize {
        app.session
            .sub_agents
            .iter()
            .filter(|view| view.status == SubAgentStatus::Running)
            .filter(|view| {
                app.agent_last_event
                    .get(&view.id)
                    .is_some_and(|t| now.duration_since(*t) > STALE_THRESHOLD)
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, SubAgentStatus, SubAgentView};

    fn mk_agent(id: &str, status: SubAgentStatus) -> SubAgentView {
        SubAgentView {
            id: id.into(),
            name: id.into(),
            status,
            turns: 0,
            tokens: 0,
            feed: Vec::new(),
        }
    }

    #[test]
    fn running_agent_within_threshold_is_not_stale_v093() {
        let mut app = App::default();
        let t0 = Instant::now();
        app.session
            .sub_agents
            .push(mk_agent("a", SubAgentStatus::Running));
        // Last event 9 min ago — under the 10-min threshold.
        app.agent_last_event
            .insert("a".into(), t0 - Duration::from_secs(9 * 60));
        assert_eq!(StaleWatchdog::check(&app, t0), 0);
    }

    #[test]
    fn running_agent_past_threshold_is_stale_v093() {
        let mut app = App::default();
        let t0 = Instant::now();
        app.session
            .sub_agents
            .push(mk_agent("a", SubAgentStatus::Running));
        // Last event 11 min ago — over the 10-min threshold.
        app.agent_last_event
            .insert("a".into(), t0 - Duration::from_secs(11 * 60));
        assert_eq!(StaleWatchdog::check(&app, t0), 1);
    }

    #[test]
    fn done_agent_with_old_event_is_not_stale_v093() {
        let mut app = App::default();
        let t0 = Instant::now();
        // Done agent with a very old last-event must NOT be counted.
        app.session
            .sub_agents
            .push(mk_agent("a", SubAgentStatus::Done));
        app.agent_last_event
            .insert("a".into(), t0 - Duration::from_secs(60 * 60));
        assert_eq!(StaleWatchdog::check(&app, t0), 0);
    }

    #[test]
    fn failed_agent_with_old_event_is_not_stale_v093() {
        let mut app = App::default();
        let t0 = Instant::now();
        app.session
            .sub_agents
            .push(mk_agent("a", SubAgentStatus::Failed));
        app.agent_last_event
            .insert("a".into(), t0 - Duration::from_secs(60 * 60));
        assert_eq!(StaleWatchdog::check(&app, t0), 0);
    }

    #[test]
    fn running_agent_without_last_event_is_not_stale_v093() {
        // Freshly spawned agent — Running but no event recorded yet.
        // It has not yet had a chance to be silent for 10 min.
        let mut app = App::default();
        app.session
            .sub_agents
            .push(mk_agent("a", SubAgentStatus::Running));
        assert_eq!(StaleWatchdog::check(&app, Instant::now()), 0);
    }

    #[test]
    fn check_counts_multiple_stale_agents_v093() {
        let mut app = App::default();
        let t0 = Instant::now();
        // Two stale Running agents.
        app.session
            .sub_agents
            .push(mk_agent("a", SubAgentStatus::Running));
        app.session
            .sub_agents
            .push(mk_agent("b", SubAgentStatus::Running));
        // One healthy Running agent (last event was just now).
        app.session
            .sub_agents
            .push(mk_agent("c", SubAgentStatus::Running));
        // One stale-timestamped but Done agent (excluded).
        app.session
            .sub_agents
            .push(mk_agent("d", SubAgentStatus::Done));

        app.agent_last_event
            .insert("a".into(), t0 - Duration::from_secs(11 * 60));
        app.agent_last_event
            .insert("b".into(), t0 - Duration::from_secs(30 * 60));
        app.agent_last_event.insert("c".into(), t0);
        app.agent_last_event
            .insert("d".into(), t0 - Duration::from_secs(60 * 60));

        assert_eq!(StaleWatchdog::check(&app, t0), 2);
    }

    /// LOW-1 pin: the watchdog is STATELESS — calling `check` twice in a row
    /// for the same `now` returns the same total count (not a delta). The
    /// second tick still sees the stale agent.
    #[test]
    fn stateless_repeated_check_returns_running_total_not_delta_v093() {
        let mut app = App::default();
        let t0 = Instant::now();
        app.session
            .sub_agents
            .push(mk_agent("a", SubAgentStatus::Running));
        app.agent_last_event
            .insert("a".into(), t0 - Duration::from_secs(11 * 60));

        assert_eq!(StaleWatchdog::check(&app, t0), 1);
        // Second tick — same agent still stale, same count returned.
        // NOT 0 (which would imply newly-transitioned counting).
        assert_eq!(StaleWatchdog::check(&app, t0), 1);
        assert_eq!(StaleWatchdog::check(&app, t0 + Duration::from_secs(1)), 1);
    }
}
