//! Deterministic, reason-specific provider cooldown authority.
//!
//! A tracker owns one provider/model candidate. Transient failures cool for an
//! exponentially increasing interval (respecting a longer server Retry-After),
//! permanent failures stay unavailable until an explicit success/reset, and
//! semantic failures do not poison provider health. Expired transient entries
//! admit exactly one half-open probe.

use crate::FailoverReason;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_PROBE_LEASE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownClass {
    Transient,
    Permanent,
    Semantic,
}

impl FailoverReason {
    pub fn cooldown_class(&self) -> CooldownClass {
        match self {
            Self::RateLimit | Self::Overloaded | Self::Timeout | Self::Auth | Self::Unknown => {
                CooldownClass::Transient
            }
            Self::AuthPermanent | Self::Billing | Self::SessionExpired | Self::ModelNotFound => {
                CooldownClass::Permanent
            }
            Self::Format | Self::ContextOverflow => CooldownClass::Semantic,
        }
    }
}

/// Monotonic clock used by the dispatch authority. Production uses
/// [`SystemCooldownClock`]; tests inject a manual clock and never sleep.
pub trait CooldownClock: Send + Sync {
    fn now(&self) -> Duration;
}

#[derive(Debug)]
pub struct SystemCooldownClock {
    origin: Instant,
}

impl Default for SystemCooldownClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl CooldownClock for SystemCooldownClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CooldownState {
    #[default]
    Ready,
    Cooling {
        /// `None` is a permanent/manual-recovery cooldown.
        retry_at: Option<Duration>,
        reason: FailoverReason,
    },
    HalfOpen {
        reason: FailoverReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownPermit {
    Ready,
    HalfOpen,
}

#[derive(Debug)]
struct CooldownInner {
    state: CooldownState,
    failure_count: u32,
    probe_lease_until: Option<Duration>,
}

pub struct CooldownTracker {
    inner: Mutex<CooldownInner>,
    clock: Arc<dyn CooldownClock>,
    transient_base: Duration,
    failure_threshold: u32,
    probe_lease: Duration,
}

impl std::fmt::Debug for CooldownTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CooldownTracker")
            .field("inner", &self.inner)
            .field("transient_base", &self.transient_base)
            .field("failure_threshold", &self.failure_threshold)
            .field("probe_lease", &self.probe_lease)
            .finish_non_exhaustive()
    }
}

impl Default for CooldownTracker {
    fn default() -> Self {
        Self::with_clock(
            Arc::new(SystemCooldownClock::default()),
            Duration::from_secs(5),
            1,
        )
    }
}

impl CooldownTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_failure_threshold(failure_threshold: u32) -> Self {
        Self::with_failure_threshold_and_base(failure_threshold, Duration::from_secs(5))
    }

    pub fn with_failure_threshold_and_base(
        failure_threshold: u32,
        transient_base: Duration,
    ) -> Self {
        Self::with_clock(
            Arc::new(SystemCooldownClock::default()),
            transient_base,
            failure_threshold,
        )
    }

    pub fn with_clock(
        clock: Arc<dyn CooldownClock>,
        transient_base: Duration,
        failure_threshold: u32,
    ) -> Self {
        Self::with_clock_and_probe_lease(
            clock,
            transient_base,
            failure_threshold,
            DEFAULT_PROBE_LEASE,
        )
    }

    /// Construct a tracker with an explicit half-open probe lease.
    ///
    /// If the probe owner disappears without recording an outcome, another
    /// caller may probe after this lease expires. A zero lease is normalized
    /// to one millisecond so concurrent callers cannot all acquire it.
    pub fn with_clock_and_probe_lease(
        clock: Arc<dyn CooldownClock>,
        transient_base: Duration,
        failure_threshold: u32,
        probe_lease: Duration,
    ) -> Self {
        Self {
            inner: Mutex::new(CooldownInner {
                state: CooldownState::Ready,
                failure_count: 0,
                probe_lease_until: None,
            }),
            clock,
            transient_base,
            failure_threshold: failure_threshold.max(1),
            probe_lease: probe_lease.max(Duration::from_millis(1)),
        }
    }

    fn refresh_expiry(&self, inner: &mut CooldownInner) {
        if let CooldownState::Cooling {
            retry_at: Some(retry_at),
            reason,
        } = inner.state
            && self.clock.now() >= retry_at
        {
            inner.state = CooldownState::HalfOpen { reason };
            inner.probe_lease_until = None;
        }
        if matches!(inner.state, CooldownState::HalfOpen { .. })
            && inner
                .probe_lease_until
                .is_some_and(|lease_until| self.clock.now() >= lease_until)
        {
            inner.probe_lease_until = None;
        }
    }

    pub fn state(&self) -> CooldownState {
        let mut inner = self.inner.lock();
        self.refresh_expiry(&mut inner);
        inner.state
    }

    /// Acquire dispatch permission. A half-open candidate grants one probe;
    /// concurrent callers remain denied until that probe records an outcome
    /// or its lease expires.
    pub fn try_acquire(&self) -> Option<CooldownPermit> {
        let mut inner = self.inner.lock();
        self.refresh_expiry(&mut inner);
        match inner.state {
            CooldownState::Ready => Some(CooldownPermit::Ready),
            CooldownState::HalfOpen { .. } if inner.probe_lease_until.is_none() => {
                inner.probe_lease_until = Some(self.clock.now().saturating_add(self.probe_lease));
                Some(CooldownPermit::HalfOpen)
            }
            CooldownState::HalfOpen { .. } | CooldownState::Cooling { .. } => None,
        }
    }

    pub fn record_failure(&self, reason: FailoverReason, retry_after: Option<Duration>) {
        let mut inner = self.inner.lock();
        inner.probe_lease_until = None;
        if reason.cooldown_class() == CooldownClass::Semantic {
            inner.state = CooldownState::Ready;
            return;
        }

        inner.failure_count = inner.failure_count.saturating_add(1);
        if reason.cooldown_class() == CooldownClass::Transient
            && inner.failure_count < self.failure_threshold
        {
            inner.state = CooldownState::Ready;
            return;
        }

        let retry_at = match reason.cooldown_class() {
            CooldownClass::Permanent => None,
            CooldownClass::Semantic => unreachable!("semantic failures return above"),
            CooldownClass::Transient => {
                let exponent = inner
                    .failure_count
                    .saturating_sub(self.failure_threshold)
                    .min(6);
                let multiplier = 1u32 << exponent;
                let local = self
                    .transient_base
                    .saturating_mul(multiplier)
                    .min(Duration::from_secs(5 * 60));
                Some(
                    self.clock
                        .now()
                        .saturating_add(retry_after.unwrap_or(Duration::ZERO).max(local)),
                )
            }
        };
        inner.state = CooldownState::Cooling { retry_at, reason };
    }

    pub fn record_success(&self) {
        let mut inner = self.inner.lock();
        inner.state = CooldownState::Ready;
        inner.failure_count = 0;
        inner.probe_lease_until = None;
    }

    pub fn reset(&self) {
        self.record_success();
    }

    pub fn is_available(&self) -> bool {
        let mut inner = self.inner.lock();
        self.refresh_expiry(&mut inner);
        matches!(inner.state, CooldownState::Ready)
            || matches!(inner.state, CooldownState::HalfOpen { .. })
                && inner.probe_lease_until.is_none()
    }

    pub fn failure_count(&self) -> u32 {
        self.inner.lock().failure_count
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self.state() {
            CooldownState::Cooling {
                retry_at: Some(retry_at),
                ..
            } => Some(retry_at.saturating_sub(self.clock.now())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Default)]
    struct ManualClock(AtomicU64);

    impl ManualClock {
        fn advance(&self, duration: Duration) {
            self.0.fetch_add(
                u64::try_from(duration.as_millis()).unwrap(),
                Ordering::SeqCst,
            );
        }
    }

    impl CooldownClock for ManualClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst))
        }
    }

    fn tracker(clock: Arc<ManualClock>, threshold: u32) -> CooldownTracker {
        CooldownTracker::with_clock(clock, Duration::from_secs(5), threshold)
    }

    fn tracker_with_probe_lease(
        clock: Arc<ManualClock>,
        threshold: u32,
        probe_lease: Duration,
    ) -> CooldownTracker {
        CooldownTracker::with_clock_and_probe_lease(
            clock,
            Duration::from_secs(5),
            threshold,
            probe_lease,
        )
    }

    #[test]
    fn transient_uses_threshold_fake_time_and_one_half_open_probe() {
        let clock = Arc::new(ManualClock::default());
        let tracker = tracker(clock.clone(), 2);
        tracker.record_failure(FailoverReason::RateLimit, None);
        assert_eq!(tracker.state(), CooldownState::Ready);
        tracker.record_failure(FailoverReason::RateLimit, None);
        assert!(matches!(tracker.state(), CooldownState::Cooling { .. }));
        clock.advance(Duration::from_secs(5));
        assert!(matches!(tracker.state(), CooldownState::HalfOpen { .. }));
        assert_eq!(tracker.try_acquire(), Some(CooldownPermit::HalfOpen));
        assert_eq!(tracker.try_acquire(), None);
    }

    #[test]
    fn retry_after_wins_over_local_backoff() {
        let clock = Arc::new(ManualClock::default());
        let tracker = tracker(clock.clone(), 1);
        tracker.record_failure(FailoverReason::RateLimit, Some(Duration::from_secs(30)));
        clock.advance(Duration::from_secs(29));
        assert_eq!(tracker.try_acquire(), None);
        clock.advance(Duration::from_secs(1));
        assert_eq!(tracker.try_acquire(), Some(CooldownPermit::HalfOpen));
    }

    #[test]
    fn abandoned_half_open_probe_is_recoverable_after_lease_expiry() {
        let clock = Arc::new(ManualClock::default());
        let tracker = tracker_with_probe_lease(clock.clone(), 1, Duration::from_secs(10));
        tracker.record_failure(FailoverReason::Timeout, None);
        clock.advance(Duration::from_secs(5));

        assert_eq!(tracker.try_acquire(), Some(CooldownPermit::HalfOpen));
        assert!(!tracker.is_available());
        clock.advance(Duration::from_secs(9));
        assert_eq!(tracker.try_acquire(), None);

        clock.advance(Duration::from_secs(1));
        assert!(tracker.is_available());
        assert_eq!(tracker.try_acquire(), Some(CooldownPermit::HalfOpen));
        assert_eq!(tracker.try_acquire(), None);
    }

    #[test]
    fn permanent_failure_never_auto_probes() {
        let clock = Arc::new(ManualClock::default());
        let tracker = tracker(clock.clone(), 1);
        tracker.record_failure(FailoverReason::AuthPermanent, None);
        clock.advance(Duration::from_secs(24 * 60 * 60));
        assert_eq!(tracker.try_acquire(), None);
        assert!(matches!(
            tracker.state(),
            CooldownState::Cooling {
                retry_at: None,
                reason: FailoverReason::AuthPermanent
            }
        ));
        tracker.reset();
        assert_eq!(tracker.try_acquire(), Some(CooldownPermit::Ready));
    }

    #[test]
    fn semantic_failure_does_not_poison_health() {
        let tracker = CooldownTracker::new();
        tracker.record_failure(FailoverReason::Format, None);
        assert_eq!(tracker.state(), CooldownState::Ready);
        tracker.record_failure(FailoverReason::ContextOverflow, None);
        assert_eq!(tracker.state(), CooldownState::Ready);
    }

    #[test]
    fn success_resets_backoff_and_probe_lease() {
        let clock = Arc::new(ManualClock::default());
        let tracker = tracker(clock.clone(), 1);
        tracker.record_failure(FailoverReason::Timeout, None);
        clock.advance(Duration::from_secs(5));
        assert_eq!(tracker.try_acquire(), Some(CooldownPermit::HalfOpen));
        tracker.record_success();
        assert_eq!(tracker.failure_count(), 0);
        assert_eq!(tracker.try_acquire(), Some(CooldownPermit::Ready));
    }

    #[test]
    fn classifies_all_reasons() {
        let table = [
            (FailoverReason::Auth, CooldownClass::Transient),
            (FailoverReason::AuthPermanent, CooldownClass::Permanent),
            (FailoverReason::Format, CooldownClass::Semantic),
            (FailoverReason::RateLimit, CooldownClass::Transient),
            (FailoverReason::Overloaded, CooldownClass::Transient),
            (FailoverReason::Billing, CooldownClass::Permanent),
            (FailoverReason::Timeout, CooldownClass::Transient),
            (FailoverReason::ModelNotFound, CooldownClass::Permanent),
            (FailoverReason::SessionExpired, CooldownClass::Permanent),
            (FailoverReason::ContextOverflow, CooldownClass::Semantic),
            (FailoverReason::Unknown, CooldownClass::Transient),
        ];
        assert_eq!(table.len(), 11);
        for (reason, class) in table {
            assert_eq!(reason.cooldown_class(), class);
        }
    }
}
