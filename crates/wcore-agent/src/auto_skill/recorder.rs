//! v0.8.1 U6 — turn trajectory recorder. Captures
//! `(user_input, picked_skill, outcome, summary)` per turn so the
//! `Bucketer` can find N-consecutive-success patterns worth crystallizing
//! as a new skill.
//!
//! Cheap POD — no I/O, no locks. The engine builds one of these at the
//! end of every `run()` call and hands it to the bucketer.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct TurnTrajectory {
    /// Raw user input the turn was launched on. The bucketer normalizes
    /// this into a task signature.
    pub user_input: String,
    /// Which catalog skill (if any) the v0.8.1 U1 `SkillRouter` chose for
    /// this turn. Recorded for telemetry / future use (e.g. weighting
    /// signatures that frequently bypass the router).
    pub picked_skill: Option<String>,
    /// Terminal verdict — `Success` for natural `EndTurn` / `ToolUse`,
    /// `Failure` for anything else (MaxTurns, error, abort).
    pub outcome: TurnOutcome,
    /// Short one-line description of what happened (e.g. "3 turns"). Used
    /// as evidence in the auto-drafted skill body.
    pub summary: String,
    /// UTC timestamp captured at the end of the turn.
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Success,
    Failure,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trajectory_is_clone_and_debug() {
        let t = TurnTrajectory {
            user_input: "refactor this".into(),
            picked_skill: Some("refactor".into()),
            outcome: TurnOutcome::Success,
            summary: "1 turn".into(),
            timestamp: Utc::now(),
        };
        let cloned = t.clone();
        assert_eq!(cloned.outcome, TurnOutcome::Success);
        // Debug must not panic on the carrier types.
        let _ = format!("{cloned:?}");
    }

    #[test]
    fn outcome_equality_distinguishes_variants() {
        assert_eq!(TurnOutcome::Success, TurnOutcome::Success);
        assert_ne!(TurnOutcome::Success, TurnOutcome::Failure);
    }
}
