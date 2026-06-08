//! Per-FailoverReason cooldown state machine — ported from openclaw MIT (c) Peter Steinberger 2025.
//!
//! When a provider fails with reason R, it enters cooldown for a duration
//! derived from R's classification:
//!   - PERMANENT reasons (AuthPermanent, Billing): long cooldown, no probe
//!   - TRANSIENT reasons (RateLimit, Overloaded, Timeout): short cooldown, probe-on-expiry
//!   - SEMANTIC reasons (ContextOverflow, Format, ModelNotFound): no cooldown
//!     (these aren't retried — caller must change inputs)

use crate::FailoverReason;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownClass {
    /// Transient — short cooldown, probe-on-expiry
    Transient,
    /// Permanent — long cooldown, no probe (require human intervention)
    Permanent,
    /// Semantic — no cooldown (caller must change inputs to retry)
    Semantic,
}

impl FailoverReason {
    pub fn cooldown_class(&self) -> CooldownClass {
        match self {
            // Transient
            Self::RateLimit | Self::Overloaded | Self::Timeout => CooldownClass::Transient,
            // Permanent (manual recovery)
            Self::AuthPermanent | Self::Billing | Self::SessionExpired => CooldownClass::Permanent,
            // Semantic (caller responsibility)
            Self::ContextOverflow | Self::Format | Self::ModelNotFound => CooldownClass::Semantic,
            // Default: treat unclassified Auth/Unknown as Transient (probe quickly)
            Self::Auth | Self::Unknown => CooldownClass::Transient,
        }
    }

    /// Per-reason base cooldown duration. Permanent reasons use a long base;
    /// Transient use a short base scaled by failure count later.
    pub fn base_cooldown(&self) -> Duration {
        match self.cooldown_class() {
            CooldownClass::Transient => Duration::from_secs(5),
            CooldownClass::Permanent => Duration::from_secs(15 * 60), // 15 minutes
            CooldownClass::Semantic => Duration::from_secs(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CooldownState {
    /// Available for use.
    #[default]
    Ready,
    /// In cooldown until the given Instant.
    Cooling {
        until: Instant,
        reason: FailoverReason,
    },
    /// One probe is allowed; if it succeeds, return to Ready.
    HalfOpen { reason: FailoverReason },
}

/// Per-provider cooldown tracker. Caller invokes [`CooldownTracker::record_failure`]
/// on each failure and [`CooldownTracker::record_success`] on each success;
/// [`CooldownTracker::state`] returns the current decision.
#[derive(Debug)]
pub struct CooldownTracker {
    state: CooldownState,
    failure_count: u32,
    /// Test-only override for the transient base cooldown so expiry tests
    /// don't have to sleep for whole seconds. None = use FailoverReason::base_cooldown.
    transient_base_override: Option<Duration>,
}

impl Default for CooldownTracker {
    fn default() -> Self {
        Self {
            state: CooldownState::Ready,
            failure_count: 0,
            transient_base_override: None,
        }
    }
}

impl CooldownTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test-only: construct a tracker whose transient cooldowns use the given
    /// short base duration. Permanent and semantic classes are unaffected.
    #[cfg(test)]
    fn with_transient_base(base: Duration) -> Self {
        Self {
            state: CooldownState::Ready,
            failure_count: 0,
            transient_base_override: Some(base),
        }
    }

    /// Current state. Auto-transitions Cooling -> HalfOpen if expiry has passed.
    pub fn state(&mut self) -> &CooldownState {
        if let CooldownState::Cooling { until, reason } = self.state
            && Instant::now() >= until
        {
            self.state = CooldownState::HalfOpen { reason };
        }
        &self.state
    }

    /// Record a failure with classified reason. Bumps failure count and sets
    /// Cooling state (exponential backoff for transient).
    pub fn record_failure(&mut self, reason: FailoverReason) {
        self.failure_count = self.failure_count.saturating_add(1);
        let class = reason.cooldown_class();
        let base = match class {
            CooldownClass::Transient => self
                .transient_base_override
                .unwrap_or_else(|| reason.base_cooldown()),
            _ => reason.base_cooldown(),
        };
        let scaled = match class {
            CooldownClass::Transient => {
                // exponential backoff capped at 5 minutes
                let mult = 1u32 << self.failure_count.min(6); // 2^1..2^6 = 2..64
                base.saturating_mul(mult).min(Duration::from_secs(5 * 60))
            }
            CooldownClass::Permanent => base,
            CooldownClass::Semantic => Duration::from_secs(0),
        };
        if scaled.is_zero() {
            self.state = CooldownState::Ready;
        } else {
            self.state = CooldownState::Cooling {
                until: Instant::now() + scaled,
                reason,
            };
        }
    }

    /// Record a success. Returns to Ready and clears failure count.
    pub fn record_success(&mut self) {
        self.state = CooldownState::Ready;
        self.failure_count = 0;
    }

    pub fn is_available(&mut self) -> bool {
        matches!(
            self.state(),
            CooldownState::Ready | CooldownState::HalfOpen { .. }
        )
    }

    pub fn failure_count(&self) -> u32 {
        self.failure_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_is_ready() {
        let mut t = CooldownTracker::new();
        assert_eq!(*t.state(), CooldownState::Ready);
        assert_eq!(t.failure_count(), 0);
        assert!(t.is_available());
    }

    #[test]
    fn record_transient_failure_enters_cooling() {
        let mut t = CooldownTracker::new();
        t.record_failure(FailoverReason::RateLimit);
        assert!(matches!(
            *t.state(),
            CooldownState::Cooling {
                reason: FailoverReason::RateLimit,
                ..
            }
        ));
        assert_eq!(t.failure_count(), 1);
    }

    #[test]
    fn record_permanent_failure_enters_cooling_long_duration() {
        let mut t = CooldownTracker::new();
        t.record_failure(FailoverReason::AuthPermanent);
        match *t.state() {
            CooldownState::Cooling { until, reason } => {
                assert_eq!(reason, FailoverReason::AuthPermanent);
                let remaining = until.saturating_duration_since(Instant::now());
                // 15 minutes minus a tiny slop for clock drift
                assert!(
                    remaining >= Duration::from_secs(14 * 60 + 50),
                    "remaining={:?}",
                    remaining
                );
            }
            ref s => panic!("expected Cooling, got {:?}", s),
        }
    }

    #[test]
    fn record_semantic_failure_stays_ready() {
        for reason in [
            FailoverReason::ContextOverflow,
            FailoverReason::Format,
            FailoverReason::ModelNotFound,
        ] {
            let mut t = CooldownTracker::new();
            t.record_failure(reason);
            assert_eq!(
                *t.state(),
                CooldownState::Ready,
                "semantic reason {:?} should not cool",
                reason
            );
        }
    }

    #[test]
    fn cooling_transitions_to_half_open_on_expiry() {
        let mut t = CooldownTracker::with_transient_base(Duration::from_millis(5));
        t.record_failure(FailoverReason::RateLimit);
        // base 5ms * 2^1 = 10ms — wait 30ms to be safe
        std::thread::sleep(Duration::from_millis(30));
        assert!(matches!(
            *t.state(),
            CooldownState::HalfOpen {
                reason: FailoverReason::RateLimit
            }
        ));
    }

    #[test]
    fn record_success_clears_state_and_count() {
        let mut t = CooldownTracker::new();
        t.record_failure(FailoverReason::RateLimit);
        t.record_failure(FailoverReason::RateLimit);
        assert_eq!(t.failure_count(), 2);
        t.record_success();
        assert_eq!(*t.state(), CooldownState::Ready);
        assert_eq!(t.failure_count(), 0);
    }

    #[test]
    fn exponential_backoff_doubles_then_caps_at_5min() {
        // Use real base (5s) so the cap math is exercised against the production constants.
        let mut t = CooldownTracker::new();
        // Drive the failure count up — at counts ≥6, mult = 2^6 = 64, so
        // 5s * 64 = 320s which exceeds the 300s (5min) cap.
        for _ in 0..10 {
            t.record_failure(FailoverReason::RateLimit);
        }
        match *t.state() {
            CooldownState::Cooling { until, .. } => {
                let remaining = until.saturating_duration_since(Instant::now());
                // Must be exactly capped at 5min (minus tiny slop).
                assert!(
                    remaining <= Duration::from_secs(5 * 60),
                    "remaining must be ≤5min cap, got {:?}",
                    remaining
                );
                assert!(
                    remaining >= Duration::from_secs(5 * 60 - 1),
                    "remaining must hit the 5min cap, got {:?}",
                    remaining
                );
            }
            ref s => panic!("expected Cooling, got {:?}", s),
        }
    }

    #[test]
    fn half_open_after_success_returns_to_ready() {
        let mut t = CooldownTracker::with_transient_base(Duration::from_millis(5));
        t.record_failure(FailoverReason::Overloaded);
        std::thread::sleep(Duration::from_millis(30));
        // Force transition by reading state.
        assert!(matches!(*t.state(), CooldownState::HalfOpen { .. }));
        t.record_success();
        assert_eq!(*t.state(), CooldownState::Ready);
        assert_eq!(t.failure_count(), 0);
    }

    #[test]
    fn half_open_after_failure_returns_to_cooling() {
        let mut t = CooldownTracker::with_transient_base(Duration::from_millis(5));
        t.record_failure(FailoverReason::Timeout);
        std::thread::sleep(Duration::from_millis(30));
        assert!(matches!(*t.state(), CooldownState::HalfOpen { .. }));
        // A failure during probe should re-enter Cooling.
        t.record_failure(FailoverReason::Timeout);
        assert!(matches!(*t.state(), CooldownState::Cooling { .. }));
        assert_eq!(t.failure_count(), 2);
    }

    #[test]
    fn cooldown_class_for_all_11_variants() {
        // Exhaustive classification table — must stay in sync with the enum.
        let table = [
            (FailoverReason::Auth, CooldownClass::Transient),
            (FailoverReason::AuthPermanent, CooldownClass::Permanent),
            (FailoverReason::Format, CooldownClass::Semantic),
            (FailoverReason::RateLimit, CooldownClass::Transient),
            (FailoverReason::Overloaded, CooldownClass::Transient),
            (FailoverReason::Billing, CooldownClass::Permanent),
            (FailoverReason::Timeout, CooldownClass::Transient),
            (FailoverReason::ModelNotFound, CooldownClass::Semantic),
            (FailoverReason::SessionExpired, CooldownClass::Permanent),
            (FailoverReason::ContextOverflow, CooldownClass::Semantic),
            (FailoverReason::Unknown, CooldownClass::Transient),
        ];
        assert_eq!(table.len(), 11, "must cover all 11 FailoverReason variants");
        for (reason, expected) in table {
            assert_eq!(
                reason.cooldown_class(),
                expected,
                "wrong class for {:?}",
                reason
            );
        }
    }

    #[test]
    fn failure_count_saturates() {
        let mut t = CooldownTracker::new();
        // Manually push failure_count near u32::MAX to verify saturation does not overflow.
        t.failure_count = u32::MAX - 1;
        t.record_failure(FailoverReason::RateLimit);
        assert_eq!(t.failure_count(), u32::MAX);
        t.record_failure(FailoverReason::RateLimit);
        assert_eq!(t.failure_count(), u32::MAX, "must saturate, not overflow");
    }

    #[test]
    fn is_available_true_when_ready_or_half_open_false_when_cooling() {
        let mut t = CooldownTracker::with_transient_base(Duration::from_millis(5));
        // Ready
        assert!(t.is_available());
        // Cooling
        t.record_failure(FailoverReason::RateLimit);
        assert!(!t.is_available(), "Cooling must be unavailable");
        // HalfOpen
        std::thread::sleep(Duration::from_millis(30));
        assert!(t.is_available(), "HalfOpen must be available");
    }
}
