//! The single shared animation clock (v0.9.2 W1).
//!
//! SPEC §1A: Wayland has exactly ONE render loop (`mod.rs:run_loop`,
//! `TICK = 33ms`) and one monotonic `App::frame_tick`. The win here is
//! narrow: make that one loop stop ticking when nothing needs animating,
//! and pause on terminal blur / resize-to-zero. `wants_tick()` is the
//! idle-CPU lever — the loop polls input on the 33ms TICK only while it
//! returns `true`; otherwise it dwells long and wakes on real events.

use std::collections::HashMap;
use std::time::Instant;

/// Stable identity for each animated element. Subscribers register on
/// demand and unsubscribe when their animation ends. FROZEN S0 surface
/// (`app.rs` and the protocol bridge reference these variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimId {
    /// The "working" spinner shown while a turn is in flight.
    Spinner,
    /// The streaming-status verb / elapsed clock widget.
    StreamingStatus,
    /// The Bash auto-approve classifier shimmer (Wave 3 consumer).
    ClassifierShimmer,
    /// The RGB color-lerp on the spinner glyph after a no-delta stall
    /// (Wave 6 consumer).
    StallLerp,
    /// The composer cursor blink.
    CursorBlink,
    /// v0.9.3 — drives 30s done-glow fade for sub-agent rows + strip ✓.
    TerminalGlow,
}

/// One subscriber's bookkeeping. `keep_alive` subscribers keep the clock
/// ticking even between visible state changes (e.g. a blinking cursor);
/// `last_tick` records the last advance a subscriber saw (reserved for
/// future per-subscriber cadence — not yet read in W1).
#[derive(Debug, Clone, Copy)]
pub struct Subscription {
    /// Keep the clock alive even when no visible state is changing.
    pub keep_alive: bool,
    /// Instant of the last advance observed for this subscriber.
    pub last_tick: Instant,
}

/// The single shared clock. `wants_tick()` is the idle-CPU lever: the
/// render loop polls input on the 33ms TICK only when it returns `true`.
#[derive(Debug, Default)]
pub struct AnimationClock {
    subscribers: HashMap<AnimId, Subscription>,
    paused: bool,
    tick: u64,
}

impl AnimationClock {
    /// Register `id` as an active animation. Idempotent — re-subscribing an
    /// already-active id just refreshes its bookkeeping. The clock starts
    /// wanting ticks as soon as the first subscriber registers (unless
    /// paused).
    pub fn subscribe(&mut self, id: AnimId, keep_alive: bool) {
        self.subscribers.insert(
            id,
            Subscription {
                keep_alive,
                last_tick: Instant::now(),
            },
        );
    }

    /// Drop `id`'s subscription. Once the last subscriber unsubscribes,
    /// `wants_tick()` goes false and the loop drops to the idle long-dwell.
    /// A no-op if `id` was never subscribed.
    pub fn unsubscribe(&mut self, id: AnimId) {
        self.subscribers.remove(&id);
    }

    /// Pause/resume the clock as a whole. Set on terminal focus-lost and
    /// resize-to-zero (offscreen proxy); cleared on focus-gained / a
    /// non-zero resize. While paused, `wants_tick()` is false no matter how
    /// many subscribers are active — animation halts while the user is in
    /// another window.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Whether the clock is currently paused (focus-lost / offscreen).
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The render loop ticks at 33ms only when this is `true`. It is
    /// `false` at an idle prompt (no subscribers) and while blurred or
    /// offscreen (paused). FROZEN signature — `app.rs` / `run_loop` depend
    /// on `wants_tick(&self) -> bool`.
    pub fn wants_tick(&self) -> bool {
        !self.paused && !self.subscribers.is_empty()
    }

    /// Bump and return the monotonic tick — feeds `App::frame_tick`.
    /// Wraps on overflow (a wrap is cosmetically harmless: animation reads
    /// are modular). FROZEN signature — `app.rs` routes `frame_tick`
    /// through this `advance(&mut self) -> u64`.
    pub fn advance(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_clock_does_not_want_a_tick() {
        let c = AnimationClock::default();
        assert!(!c.wants_tick());
    }

    #[test]
    fn subscribing_makes_the_clock_want_ticks_and_unsubscribing_stops_it() {
        let mut c = AnimationClock::default();
        c.subscribe(AnimId::Spinner, false);
        assert!(c.wants_tick());
        c.unsubscribe(AnimId::Spinner);
        assert!(!c.wants_tick());
    }

    #[test]
    fn paused_clock_never_wants_a_tick_even_with_subscribers() {
        let mut c = AnimationClock::default();
        c.subscribe(AnimId::Spinner, false);
        c.set_paused(true);
        assert!(!c.wants_tick());
        // ...and resumes once unpaused while subscribers remain.
        c.set_paused(false);
        assert!(c.wants_tick());
    }

    #[test]
    fn multiple_subscribers_must_all_release_before_idle() {
        let mut c = AnimationClock::default();
        c.subscribe(AnimId::Spinner, false);
        c.subscribe(AnimId::StreamingStatus, false);
        c.unsubscribe(AnimId::Spinner);
        // StreamingStatus still pins the clock.
        assert!(c.wants_tick());
        c.unsubscribe(AnimId::StreamingStatus);
        assert!(!c.wants_tick());
    }

    #[test]
    fn subscribe_is_idempotent() {
        let mut c = AnimationClock::default();
        c.subscribe(AnimId::Spinner, false);
        c.subscribe(AnimId::Spinner, true);
        // One unsubscribe is enough to clear a re-subscribed id.
        c.unsubscribe(AnimId::Spinner);
        assert!(!c.wants_tick());
    }

    #[test]
    fn advance_is_monotonic_and_returns_the_new_tick() {
        let mut c = AnimationClock::default();
        assert_eq!(c.advance(), 1);
        assert_eq!(c.advance(), 2);
        assert_eq!(c.advance(), 3);
    }

    #[test]
    fn is_paused_reflects_set_paused() {
        let mut c = AnimationClock::default();
        assert!(!c.is_paused());
        c.set_paused(true);
        assert!(c.is_paused());
    }
}
