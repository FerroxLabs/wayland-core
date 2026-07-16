//! Reusable circuit-breaker primitive (Closed → Open → HalfOpen).
//!
//! Lives in `wcore-config` so both `wcore-providers` and `wcore-tools`
//! can use it without either depending on the other.
//!
//! ## State machine
//!
//! ```text
//! Closed ──(K failures in window)──► Open
//!   ▲                                  │
//!   │       (cooldown elapsed)         │
//!   │  HalfOpen ◄─────────────────────┘
//!   │    │  │
//!   │  success  failure
//!   └────┘    └──► Open
//! ```
//!
//! ## Mutex choice
//!
//! Uses `parking_lot::Mutex` rather than `std::sync::Mutex`. The std
//! mutex poisons on panic-while-locked, causing every subsequent
//! `.lock().expect(...)` to cascade-panic. `parking_lot` has no
//! poisoning semantics; the short arithmetic in each critical section
//! cannot leave the state in an invalid shape, so resuming after a
//! panic is safe.

use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Configuration knobs for a `CircuitBreaker`.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures within `window` that trips the breaker.
    pub fail_threshold: usize,
    /// Rolling window for counting failures.
    pub window: Duration,
    /// How long the breaker stays Open before transitioning to HalfOpen.
    pub cooldown: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            fail_threshold: 3,
            window: Duration::from_secs(30),
            cooldown: Duration::from_secs(60),
        }
    }
}

/// Observable state of a `CircuitBreaker`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation — calls are allowed through.
    Closed,
    /// Too many recent failures — calls are blocked.
    Open,
    /// Cooldown elapsed — one trial call is allowed.
    HalfOpen,
}

impl BreakerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

struct Inner {
    /// Timestamps of failures within the current window.
    failures: Vec<Instant>,
    state: BreakerState,
    /// When the breaker transitioned to Open.
    opened_at: Option<Instant>,
    /// F10: whether the single HalfOpen trial permit is currently held. Set
    /// when admitting the one trial caller; while held, further callers are
    /// treated as Open (rejected) until the trial resolves (success → Closed,
    /// failure → Open). Prevents the post-cooldown stampede where every
    /// concurrent caller was admitted at once.
    half_open_trial_taken: bool,
}

impl Inner {
    fn new() -> Self {
        Self {
            failures: vec![],
            state: BreakerState::Closed,
            opened_at: None,
            half_open_trial_taken: false,
        }
    }
}

/// A thread-safe circuit breaker.
///
/// Callers drive the state machine with three methods:
///
/// 1. `is_open()` — check before making a call; returns `true` when
///    calls should be blocked.
/// 2. `record_success()` — call on a successful outcome.
/// 3. `record_failure()` — call on a failed outcome.
///
/// `is_open()` handles the HalfOpen → Closed transition automatically
/// (it returns `false` once the cooldown elapses, allowing one trial).
pub struct CircuitBreaker {
    cfg: CircuitBreakerConfig,
    inner: Mutex<Inner>,
}

impl CircuitBreaker {
    pub fn new(cfg: CircuitBreakerConfig) -> Self {
        Self {
            cfg,
            inner: Mutex::new(Inner::new()),
        }
    }

    /// Returns `true` when the caller MUST NOT proceed with the call.
    ///
    /// Side-effect: transitions Open → HalfOpen once `cooldown` elapses.
    pub fn is_open(&self) -> bool {
        let mut s = self.inner.lock();
        match s.state {
            BreakerState::Closed => false,
            // F10: in HalfOpen, admit exactly ONE trial. The first caller takes
            // the permit (returns false); any concurrent caller while the trial
            // is in flight is treated as Open until the trial resolves.
            BreakerState::HalfOpen => {
                if s.half_open_trial_taken {
                    true
                } else {
                    s.half_open_trial_taken = true;
                    false
                }
            }
            BreakerState::Open => {
                if let Some(opened) = s.opened_at
                    && opened.elapsed() >= self.cfg.cooldown
                {
                    s.state = BreakerState::HalfOpen;
                    s.half_open_trial_taken = true; // this caller takes the single trial permit
                    return false; // allow trial call
                }
                true
            }
        }
    }

    /// Returns the current observable state (read-only snapshot).
    pub fn state(&self) -> BreakerState {
        self.inner.lock().state
    }

    /// Whether this breaker carries live state that must survive a restart.
    ///
    /// A Closed breaker still requires conservative restoration when it has
    /// failures inside the rolling window: losing that history would let a
    /// restarted process admit calls that the original process was close to
    /// blocking. Open and HalfOpen always require restoration, even when the
    /// cooldown has elapsed, because observing this method must not advance
    /// the state machine or admit a trial call.
    pub fn requires_conservative_restore(&self) -> bool {
        let s = self.inner.lock();
        if s.state != BreakerState::Closed {
            return true;
        }

        let now = Instant::now();
        s.failures
            .iter()
            .any(|failure| now.saturating_duration_since(*failure) <= self.cfg.window)
    }

    /// Conservatively restore a breaker after a process restart.
    ///
    /// The original failure timestamps and in-flight HalfOpen trial cannot be
    /// reconstructed safely across processes. Restart the full cooldown from
    /// now and admit no trial as part of restoration. The next trial can only
    /// be acquired by a later [`Self::is_open`] call after that cooldown.
    pub fn restore_conservative(&self) {
        let mut s = self.inner.lock();
        s.state = BreakerState::Open;
        s.opened_at = Some(Instant::now());
        s.half_open_trial_taken = false;
    }

    /// Record a successful call outcome.
    ///
    /// HalfOpen → Closed (clears failure history).
    /// Closed → no-op (clears failure history as a hygiene step).
    pub fn record_success(&self) {
        let mut s = self.inner.lock();
        s.failures.clear();
        s.opened_at = None;
        s.state = BreakerState::Closed;
        s.half_open_trial_taken = false; // trial resolved; release the permit
    }

    /// Record a failed call outcome.
    ///
    /// May transition Closed → Open or HalfOpen → Open.
    /// Returns the new `BreakerState` if a transition occurred, else `None`.
    pub fn record_failure(&self) -> Option<BreakerState> {
        let mut s = self.inner.lock();
        let now = Instant::now();
        // Evict failures outside the rolling window.
        s.failures
            .retain(|t| now.duration_since(*t) <= self.cfg.window);
        s.failures.push(now);

        // HalfOpen trial failed → immediately re-open.
        if s.state == BreakerState::HalfOpen {
            s.state = BreakerState::Open;
            s.opened_at = Some(now);
            s.half_open_trial_taken = false; // trial resolved; a fresh trial may be admitted after the next cooldown
            return Some(BreakerState::Open);
        }

        // Closed: check threshold.
        if s.failures.len() >= self.cfg.fail_threshold && s.state == BreakerState::Closed {
            s.state = BreakerState::Open;
            s.opened_at = Some(now);
            return Some(BreakerState::Open);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(threshold: usize, window_secs: u64, cooldown_secs: u64) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            fail_threshold: threshold,
            window: Duration::from_secs(window_secs),
            cooldown: Duration::from_secs(cooldown_secs),
        }
    }

    #[test]
    fn starts_closed() {
        let b = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(!b.is_open());
    }

    #[test]
    fn stays_closed_below_threshold() {
        let b = CircuitBreaker::new(cfg(3, 30, 60));
        b.record_failure();
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(!b.is_open());
    }

    #[test]
    fn opens_at_threshold() {
        let b = CircuitBreaker::new(cfg(3, 30, 60));
        assert!(b.record_failure().is_none());
        assert!(b.record_failure().is_none());
        let t = b.record_failure();
        assert_eq!(t, Some(BreakerState::Open));
        assert_eq!(b.state(), BreakerState::Open);
        assert!(b.is_open());
    }

    #[test]
    fn success_resets_to_closed() {
        let b = CircuitBreaker::new(cfg(3, 30, 60));
        b.record_failure();
        b.record_failure();
        b.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(!b.is_open());
    }

    #[test]
    fn half_open_trial_success_closes() {
        // Open the breaker.
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 1,
            window: Duration::from_secs(30),
            cooldown: Duration::from_millis(1), // very short cooldown
        });
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Open);

        // Wait for cooldown.
        std::thread::sleep(Duration::from_millis(5));

        // is_open() should now return false (HalfOpen) and allow trial.
        assert!(!b.is_open());
        assert_eq!(b.state(), BreakerState::HalfOpen);

        // Trial succeeds → Closed.
        b.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(!b.is_open());
    }

    #[test]
    fn half_open_trial_failure_reopens() {
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 1,
            window: Duration::from_secs(30),
            cooldown: Duration::from_millis(1),
        });
        b.record_failure();
        std::thread::sleep(Duration::from_millis(5));
        assert!(!b.is_open()); // transitions to HalfOpen

        // Trial fails → re-Open.
        let t = b.record_failure();
        assert_eq!(t, Some(BreakerState::Open));
        assert_eq!(b.state(), BreakerState::Open);
        assert!(b.is_open());
    }

    #[test]
    fn half_open_admits_only_one_trial() {
        // F10: after cooldown elapses, exactly ONE caller is admitted in
        // HalfOpen; subsequent callers are rejected (treated as Open) until the
        // trial resolves.
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 1,
            window: Duration::from_secs(30),
            cooldown: Duration::from_millis(1),
        });
        b.record_failure();
        assert_eq!(b.state(), BreakerState::Open);
        std::thread::sleep(Duration::from_millis(5));

        // First post-cooldown caller takes the single trial permit.
        assert!(!b.is_open(), "first caller must be admitted as the trial");
        assert_eq!(b.state(), BreakerState::HalfOpen);

        // Every concurrent caller while the trial is in flight is rejected.
        assert!(b.is_open(), "second concurrent caller must be rejected");
        assert!(b.is_open(), "third concurrent caller must be rejected");
        assert_eq!(b.state(), BreakerState::HalfOpen);

        // Trial succeeds → Closed; the permit is released.
        b.record_success();
        assert_eq!(b.state(), BreakerState::Closed);
        assert!(!b.is_open());
    }

    #[test]
    fn half_open_trial_failure_allows_fresh_trial_after_cooldown() {
        // F10: after a failed trial re-opens the breaker, the next cooldown
        // must again admit exactly one fresh trial (permit was released).
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 1,
            window: Duration::from_secs(30),
            cooldown: Duration::from_millis(1),
        });
        b.record_failure();
        std::thread::sleep(Duration::from_millis(5));
        assert!(!b.is_open()); // first trial admitted
        assert!(b.is_open()); // concurrent caller rejected
        b.record_failure(); // trial fails → re-Open
        assert_eq!(b.state(), BreakerState::Open);

        std::thread::sleep(Duration::from_millis(5));
        assert!(
            !b.is_open(),
            "a fresh trial must be admitted after cooldown"
        );
        assert!(b.is_open(), "but still only one at a time");
    }

    #[test]
    fn failures_outside_window_do_not_count() {
        // Use a 0-second window: every prior failure is immediately stale.
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 2,
            window: Duration::from_millis(0),
            cooldown: Duration::from_secs(60),
        });
        // Record two failures with a tiny sleep so they fall outside the 0ms window.
        b.record_failure();
        std::thread::sleep(Duration::from_millis(1));
        // Each call retains only failures within window=0ms; the prior one is stale.
        let t = b.record_failure();
        // Should NOT have tripped because the first failure is outside the window.
        assert!(t.is_none(), "stale failure must not count toward threshold");
        assert_eq!(b.state(), BreakerState::Closed);
    }

    #[test]
    fn breaker_survives_panic_in_neighbouring_thread() {
        use std::sync::Arc;
        let b = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default()));
        let b2 = Arc::clone(&b);
        let _ = std::thread::spawn(move || {
            b2.record_failure();
            panic!("intentional");
        })
        .join();
        // Must not cascade-panic.
        let _ = b.is_open();
        b.record_success();
        b.record_failure();
    }

    #[test]
    fn live_closed_failure_history_requires_conservative_restore() {
        let b = CircuitBreaker::new(cfg(3, 30, 60));
        assert!(!b.requires_conservative_restore());

        b.record_failure();

        assert_eq!(b.state(), BreakerState::Closed);
        assert!(b.requires_conservative_restore());
    }

    #[test]
    fn stale_closed_failure_history_does_not_require_restore() {
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 3,
            window: Duration::from_millis(0),
            cooldown: Duration::from_secs(60),
        });
        b.record_failure();
        std::thread::sleep(Duration::from_millis(1));

        assert_eq!(b.state(), BreakerState::Closed);
        assert!(!b.requires_conservative_restore());
    }

    #[test]
    fn open_and_half_open_states_require_restore_without_advancing() {
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 1,
            window: Duration::from_secs(30),
            cooldown: Duration::from_millis(1),
        });
        b.record_failure();
        assert!(b.requires_conservative_restore());
        assert_eq!(b.state(), BreakerState::Open);

        std::thread::sleep(Duration::from_millis(5));
        assert!(b.requires_conservative_restore());
        assert_eq!(
            b.state(),
            BreakerState::Open,
            "read-only inspection must not admit the HalfOpen trial"
        );

        assert!(!b.is_open());
        assert_eq!(b.state(), BreakerState::HalfOpen);
        assert!(b.requires_conservative_restore());
    }

    #[test]
    fn conservative_restore_restarts_full_cooldown_without_admitting_trial() {
        let b = CircuitBreaker::new(CircuitBreakerConfig {
            fail_threshold: 3,
            window: Duration::from_secs(30),
            cooldown: Duration::from_millis(20),
        });

        b.restore_conservative();

        assert_eq!(b.state(), BreakerState::Open);
        assert!(b.is_open(), "restoration must not admit an immediate trial");
        std::thread::sleep(Duration::from_millis(25));
        assert!(!b.is_open(), "one trial is admitted after the new cooldown");
        assert!(b.is_open(), "the HalfOpen trial remains single-admission");
    }
}
