//! Bounded retry with backoff and a recorded terminal give-up.
//!
//! Phase 24 plan 24-02, Task 2.
//!
//! # Why the give-up is a STATE and not an absence
//!
//! Before this module a failing job simply kept its `last_fired` pinned and
//! was retried on every tick, forever. Two things were wrong with that. The
//! obvious one is the load: an unbounded retry against a failing target is
//! how a background runtime consumes a machine unattended (threat
//! T-24-02-03). The subtle one is the reporting: a job that has given up and
//! a job that is between attempts look identical from the outside, so an
//! operator cannot tell "this is still trying" from "this stopped trying an
//! hour ago". [`CronFireOutcome::GaveUp`] is therefore a named, recorded
//! outcome, not the absence of further records.
//!
//! # Why a KNOWN failure is not the same as an unknown one
//!
//! Retry here covers a dispatch that returned an error — a known, observed
//! failure. It deliberately does NOT cover a process that died mid-attempt;
//! that case has an unknown outcome and belongs to the delivery ledger's
//! `Attempted` state in `wcore-gateway`. Conflating them would either retry
//! deliveries that already landed or abandon ones that did not.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Default attempt cap: the first try plus two retries.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
/// Default first backoff, doubling from there.
pub const DEFAULT_BASE_BACKOFF_SECS: u64 = 60;
/// Default backoff ceiling. Doubling without a ceiling reaches days.
pub const DEFAULT_MAX_BACKOFF_SECS: u64 = 3600;
/// Hard ceiling on the attempt cap, whatever a persisted record asks for.
pub const CEILING_MAX_ATTEMPTS: u32 = 10;

/// How a job retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Total attempts including the first. `1` means never retry.
    pub max_attempts: u32,
    /// Backoff before the second attempt. Doubles each time.
    pub base_backoff_secs: u64,
    /// Ceiling on the doubling.
    pub max_backoff_secs: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            base_backoff_secs: DEFAULT_BASE_BACKOFF_SECS,
            max_backoff_secs: DEFAULT_MAX_BACKOFF_SECS,
        }
    }
}

impl RetryPolicy {
    /// Narrow a persisted policy so it can never be wider than the ceiling.
    ///
    /// One-way, for the same reason [`crate::trigger::TriggerBound::clamp_to`]
    /// is: a policy a hand-edited record can widen is not a bound. A record
    /// asking for a thousand attempts gets ten.
    pub fn clamped(self) -> Self {
        Self {
            max_attempts: self.max_attempts.clamp(1, CEILING_MAX_ATTEMPTS),
            base_backoff_secs: self.base_backoff_secs.max(1),
            max_backoff_secs: self
                .max_backoff_secs
                .max(self.base_backoff_secs.max(1))
                .min(24 * 3600),
        }
    }

    /// The backoff before attempt number `attempt` (1-based; attempt 1 is the
    /// first try and has no backoff).
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::zero();
        }
        // Doubling, saturating at the ceiling. `checked_shl`-style guard: the
        // exponent is bounded by CEILING_MAX_ATTEMPTS so this cannot overflow,
        // but the saturating form states that rather than relying on it.
        let steps = (attempt - 2).min(31);
        let secs = self
            .base_backoff_secs
            .saturating_mul(1u64 << steps)
            .min(self.max_backoff_secs);
        Duration::seconds(secs as i64)
    }
}

/// What the runner does next after a failed attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDecision {
    /// Try again, not before this instant.
    Retry {
        attempt: u32,
        not_before: DateTime<Utc>,
    },
    /// The cap is reached. Terminal, recorded, and never silently resumed.
    GiveUp { attempts: u32 },
}

/// Per-job retry bookkeeping, persisted alongside the job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryState {
    /// Attempts made against the CURRENT failure run. Reset by a success.
    #[serde(default)]
    pub attempts: u32,
    /// Earliest instant the next attempt may be made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    /// Whether this job has given up on its current failure run. A given-up
    /// job is NOT retried and NOT silently forgotten: it stays visible with
    /// this flag until it is edited or re-enabled.
    #[serde(default)]
    pub gave_up: bool,
}

impl RetryState {
    /// Whether an attempt may be made at `now`.
    pub fn may_attempt(&self, now: DateTime<Utc>) -> bool {
        if self.gave_up {
            return false;
        }
        match self.not_before {
            Some(t) => now >= t,
            None => true,
        }
    }

    /// Record a failed attempt and decide what happens next.
    pub fn record_failure(&mut self, policy: &RetryPolicy, now: DateTime<Utc>) -> RetryDecision {
        let policy = policy.clone().clamped();
        self.attempts = self.attempts.saturating_add(1);
        if self.attempts >= policy.max_attempts {
            self.gave_up = true;
            self.not_before = None;
            return RetryDecision::GiveUp {
                attempts: self.attempts,
            };
        }
        let next = self.attempts + 1;
        let at = now + policy.backoff_for(next);
        self.not_before = Some(at);
        RetryDecision::Retry {
            attempt: next,
            not_before: at,
        }
    }

    /// Clear the failure run. Called on a success, and on an operator edit.
    pub fn record_success(&mut self) {
        self.attempts = 0;
        self.not_before = None;
        self.gave_up = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 27, 12, 0, 0).unwrap()
    }

    #[test]
    fn retry_reaches_a_terminal_give_up_inside_the_cap() {
        let p = RetryPolicy {
            max_attempts: 3,
            base_backoff_secs: 60,
            max_backoff_secs: 3600,
        };
        let mut s = RetryState::default();
        assert!(matches!(
            s.record_failure(&p, t0()),
            RetryDecision::Retry { attempt: 2, .. }
        ));
        assert!(matches!(
            s.record_failure(&p, t0()),
            RetryDecision::Retry { attempt: 3, .. }
        ));
        assert_eq!(
            s.record_failure(&p, t0()),
            RetryDecision::GiveUp { attempts: 3 },
            "the cap must be reached, not approached forever"
        );
        assert!(s.gave_up);
        assert!(
            !s.may_attempt(t0() + Duration::days(365)),
            "a given-up job must not resume on its own, however long it waits"
        );
    }

    #[test]
    fn backoff_doubles_and_then_stops_at_the_ceiling() {
        let p = RetryPolicy {
            max_attempts: 10,
            base_backoff_secs: 60,
            max_backoff_secs: 300,
        };
        assert_eq!(p.backoff_for(1), Duration::zero());
        assert_eq!(p.backoff_for(2), Duration::seconds(60));
        assert_eq!(p.backoff_for(3), Duration::seconds(120));
        assert_eq!(p.backoff_for(4), Duration::seconds(240));
        assert_eq!(
            p.backoff_for(5),
            Duration::seconds(300),
            "doubling must stop at the ceiling rather than reaching days"
        );
        assert_eq!(p.backoff_for(9), Duration::seconds(300));
    }

    #[test]
    fn a_stored_policy_cannot_widen_the_cap() {
        let hostile = RetryPolicy {
            max_attempts: 100_000,
            base_backoff_secs: 0,
            max_backoff_secs: 0,
        };
        let c = hostile.clamped();
        assert_eq!(c.max_attempts, CEILING_MAX_ATTEMPTS);
        assert!(c.base_backoff_secs >= 1, "a zero backoff is a spin loop");
        assert!(c.max_backoff_secs >= c.base_backoff_secs);
    }

    #[test]
    fn the_backoff_window_actually_holds_off_the_next_attempt() {
        let p = RetryPolicy::default();
        let mut s = RetryState::default();
        let RetryDecision::Retry { not_before, .. } = s.record_failure(&p, t0()) else {
            panic!("first failure under the default cap must retry");
        };
        assert!(
            !s.may_attempt(t0()),
            "an attempt inside the backoff is refused"
        );
        assert!(s.may_attempt(not_before));
    }

    #[test]
    fn a_success_clears_the_failure_run() {
        let p = RetryPolicy::default();
        let mut s = RetryState::default();
        s.record_failure(&p, t0());
        s.record_success();
        assert_eq!(s, RetryState::default());
        assert!(s.may_attempt(t0()));
    }

    #[test]
    fn a_never_retry_policy_gives_up_on_the_first_failure() {
        let p = RetryPolicy {
            max_attempts: 1,
            ..RetryPolicy::default()
        };
        let mut s = RetryState::default();
        assert_eq!(
            s.record_failure(&p, t0()),
            RetryDecision::GiveUp { attempts: 1 }
        );
    }
}
