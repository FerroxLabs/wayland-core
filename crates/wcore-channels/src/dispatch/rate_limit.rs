//! `AutoReplyRateLimiter` — per-conversation rolling-window throttle for
//! AUTONOMOUS channel sends (the agent's auto-replies to inbound messages).
//!
//! Two Wayland agents wired to the same channel (e.g. two email bots) can
//! auto-reply to each other indefinitely: A replies to B, B replies to A, and
//! so on forever. Neither existing guard catches it — the self/bot loop guard
//! ([`crate::dispatch::classify`]) only drops the channel's own / other bots'
//! messages, and the wayland#547 `Message-ID` echo guard only recognises a
//! channel's own outbound mail bouncing back. In a two-agent ping-pong every
//! message is genuinely new: not a self-echo, not a duplicate, and (from the
//! receiver's side) not flagged as a bot. So both guards pass and the loop runs.
//!
//! This limiter breaks that ping-pong by capping how many autonomous replies a
//! single conversation may emit within a rolling time window. Once a
//! conversation hits the cap, further autonomous sends are suppressed (and
//! logged by the caller) until enough of the window has elapsed for older sends
//! to age out.
//!
//! Only autonomous auto-replies are gated. Human/operator-initiated sends (the
//! `send_message` tool, cron, direct [`crate::ChannelManager::send_to`]) take a
//! different code path and never reach this limiter.
//!
//! Time is caller-supplied (`now: Instant`, monotonic) so the limiter is fully
//! deterministic under test — it never reads the wall clock. Production callers
//! pass `std::time::Instant::now()`.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Default cap on autonomous replies per conversation per window. This is a
/// runaway-RATE BACKSTOP, not a full loop terminator: a rolling window caps the
/// send *rate* (a sustained ping-pong is throttled, not stopped), which bounds
/// the runaway cost/spam explosion — the actual harm — even though a slow loop
/// can persist at the cap rate. The guard keys on the conversation and cannot
/// tell a human from a peer agent at the send site (exactly why the #547
/// self/dedupe guards miss a two-agent ping-pong), so the cap is set well ABOVE
/// any realistic conversation: a runaway agent-to-agent loop fires as fast as
/// turns complete (seconds apart → hundreds per window) and is caught, while a
/// person rapidly messaging their own agent stays under it. On the primary
/// threat channel (email) a human never approaches this rate. Suppression logs
/// at WARN (operator-visible); it does not surface a channel-side notice — that
/// and tool-driven-send coverage are tracked follow-ups.
pub const DEFAULT_MAX_AUTO_REPLIES: usize = 30;

/// Default rolling window for [`DEFAULT_MAX_AUTO_REPLIES`].
pub const DEFAULT_AUTO_REPLY_WINDOW: Duration = Duration::from_secs(600);

/// Default upper bound on the number of distinct conversations tracked at once.
/// Bounds memory under a flood of distinct conversation ids; least-recently
/// active conversations are evicted first (their history would age out anyway).
pub const DEFAULT_CONVERSATION_CAP: usize = 4096;

/// Per-conversation rolling-window rate limiter for autonomous sends.
///
/// State is a bounded map of `conversation id -> timestamps of recent
/// autonomous sends`. On each admitted send the conversation's history is
/// pruned to the window, then the send is allowed (and recorded) only if fewer
/// than `max_sends` remain. A suppressed send is NOT recorded — otherwise the
/// window would never drain.
#[derive(Debug, Clone)]
pub struct AutoReplyRateLimiter {
    /// Maximum autonomous sends permitted per conversation within `window`.
    max_sends: usize,
    /// Rolling window width. `Duration::ZERO` disables the limiter entirely
    /// (every send is allowed) — mirrors [`crate::DedupeCache`]'s `ttl == 0`
    /// "disabled" convention so an operator can turn the guard off.
    window: Duration,
    /// Upper bound on tracked conversations. `0` disables capping.
    cap: usize,
    /// conversation id -> ascending timestamps of its recent recorded sends.
    conversations: HashMap<String, VecDeque<Instant>>,
}

impl AutoReplyRateLimiter {
    /// Construct a limiter. See [`DEFAULT_MAX_AUTO_REPLIES`],
    /// [`DEFAULT_AUTO_REPLY_WINDOW`], and [`DEFAULT_CONVERSATION_CAP`] for the
    /// standard values. A `window` of [`Duration::ZERO`] disables limiting.
    pub fn new(max_sends: usize, window: Duration, cap: usize) -> Self {
        Self {
            max_sends,
            window,
            cap,
            conversations: HashMap::new(),
        }
    }

    /// Check whether an autonomous send for `conversation` is permitted at
    /// `now`, recording it if so.
    ///
    /// Returns `true` when the send is allowed (and the timestamp is recorded),
    /// `false` when the conversation has already emitted `max_sends` autonomous
    /// sends within the rolling `window` — in which case nothing is recorded and
    /// the caller must suppress the send. A disabled limiter (`window ==
    /// Duration::ZERO`) always returns `true`.
    pub fn check_and_record(&mut self, conversation: &str, now: Instant) -> bool {
        // Disabled: no window means no limiting.
        if self.window.is_zero() {
            return true;
        }

        let window = self.window;
        let max_sends = self.max_sends;
        let history = self
            .conversations
            .entry(conversation.to_string())
            .or_default();
        Self::prune(history, now, window);

        let allowed = if history.len() >= max_sends {
            false
        } else {
            history.push_back(now);
            true
        };

        // Bound the number of tracked conversations, never evicting the one we
        // just touched. Runs after recording so `conversation` is retained.
        self.enforce_cap(conversation);
        allowed
    }

    /// Drop timestamps at the front older than `window` (history is ascending,
    /// so once one is in-window every later one is too). Uses
    /// `saturating_duration_since` so a `now` that is somehow not after the
    /// stored instant yields zero elapsed rather than panicking.
    fn prune(history: &mut VecDeque<Instant>, now: Instant, window: Duration) {
        while let Some(front) = history.front() {
            if now.saturating_duration_since(*front) >= window {
                history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Enforce [`Self::cap`] on the number of tracked conversations, keeping
    /// `keep`. First drops conversations whose window has fully drained (empty
    /// history), then evicts the least-recently-active conversation (oldest most
    /// recent send) until within the cap. `cap == 0` disables capping.
    fn enforce_cap(&mut self, keep: &str) {
        if self.cap == 0 || self.conversations.len() <= self.cap {
            return;
        }
        // Empty histories carry no live rate state — reclaim them first.
        self.conversations.retain(|k, h| k == keep || !h.is_empty());
        while self.conversations.len() > self.cap {
            let victim = self
                .conversations
                .iter()
                .filter(|(k, _)| k.as_str() != keep)
                .min_by_key(|(_, h)| h.back().copied())
                .map(|(k, _)| k.clone());
            match victim {
                Some(v) => {
                    self.conversations.remove(&v);
                }
                None => break,
            }
        }
    }

    /// Number of conversations currently tracked. For tests / introspection.
    pub fn tracked_conversations(&self) -> usize {
        self.conversations.len()
    }
}

impl Default for AutoReplyRateLimiter {
    /// The standard guard: [`DEFAULT_MAX_AUTO_REPLIES`] per
    /// [`DEFAULT_AUTO_REPLY_WINDOW`], bounded to [`DEFAULT_CONVERSATION_CAP`]
    /// conversations.
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_AUTO_REPLIES,
            DEFAULT_AUTO_REPLY_WINDOW,
            DEFAULT_CONVERSATION_CAP,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed base instant plus helpers to advance it deterministically —
    /// no sleeping, no wall clock.
    fn base() -> Instant {
        Instant::now()
    }

    fn after(t: Instant, secs: u64) -> Instant {
        t.checked_add(Duration::from_secs(secs))
            .expect("test instant in range")
    }

    #[test]
    fn under_limit_passes() {
        let mut rl = AutoReplyRateLimiter::new(3, Duration::from_secs(600), 1024);
        let t = base();
        // Three sends within the window are all allowed.
        assert!(rl.check_and_record("conv", t));
        assert!(rl.check_and_record("conv", after(t, 1)));
        assert!(rl.check_and_record("conv", after(t, 2)));
    }

    #[test]
    fn over_limit_is_suppressed() {
        let mut rl = AutoReplyRateLimiter::new(3, Duration::from_secs(600), 1024);
        let t = base();
        assert!(rl.check_and_record("conv", t));
        assert!(rl.check_and_record("conv", after(t, 1)));
        assert!(rl.check_and_record("conv", after(t, 2)));
        // Fourth send within the window is suppressed.
        assert!(!rl.check_and_record("conv", after(t, 3)));
        // Still suppressed just before the window rolls over.
        assert!(!rl.check_and_record("conv", after(t, 599)));
    }

    #[test]
    fn window_rollover_reallows() {
        let mut rl = AutoReplyRateLimiter::new(2, Duration::from_secs(600), 1024);
        let t = base();
        assert!(rl.check_and_record("conv", t));
        assert!(rl.check_and_record("conv", after(t, 1)));
        // Over the cap while both are in-window.
        assert!(!rl.check_and_record("conv", after(t, 2)));
        // At t+600 the first send (at t) has aged out (elapsed == window is
        // NOT < window -> pruned), freeing one slot.
        assert!(rl.check_and_record("conv", after(t, 600)));
        // But the second send (at t+1) is still in-window, so the next is
        // suppressed again.
        assert!(!rl.check_and_record("conv", after(t, 600)));
        // Once the second also ages out, sends flow again.
        assert!(rl.check_and_record("conv", after(t, 601)));
    }

    #[test]
    fn distinct_conversations_are_independent() {
        let mut rl = AutoReplyRateLimiter::new(1, Duration::from_secs(600), 1024);
        let t = base();
        // Each conversation gets its own budget.
        assert!(rl.check_and_record("a", t));
        assert!(rl.check_and_record("b", t));
        // Second send for "a" is suppressed, but "b" is untouched.
        assert!(!rl.check_and_record("a", after(t, 1)));
        assert!(!rl.check_and_record("b", after(t, 1)));
        // A third, fresh conversation still passes.
        assert!(rl.check_and_record("c", after(t, 1)));
    }

    #[test]
    fn zero_window_disables_limiting() {
        let mut rl = AutoReplyRateLimiter::new(1, Duration::ZERO, 1024);
        let t = base();
        // With the guard disabled, an unbounded number of sends pass.
        for i in 0..100 {
            assert!(rl.check_and_record("conv", after(t, i)));
        }
        // No state is accumulated when disabled.
        assert_eq!(rl.tracked_conversations(), 0);
    }

    #[test]
    fn conversation_map_is_bounded_by_cap() {
        let mut rl = AutoReplyRateLimiter::new(3, Duration::from_secs(600), 2);
        let t = base();
        // Record for many distinct conversations; the map never exceeds the cap.
        for i in 0..50 {
            let conv = format!("conv-{i}");
            assert!(rl.check_and_record(&conv, after(t, i)));
            assert!(
                rl.tracked_conversations() <= 2,
                "tracked conversations must stay within the cap"
            );
        }
    }

    #[test]
    fn eviction_keeps_the_just_recorded_conversation() {
        // Cap of 1: each new conversation evicts the previous, but the one being
        // recorded is always retained (so its send was truly counted).
        let mut rl = AutoReplyRateLimiter::new(1, Duration::from_secs(600), 1);
        let t = base();
        assert!(rl.check_and_record("first", t));
        assert!(rl.check_and_record("second", after(t, 1)));
        assert_eq!(rl.tracked_conversations(), 1);
        // "first" was evicted, so it reads as fresh (allowed) again; recording
        // it now evicts "second".
        assert!(rl.check_and_record("first", after(t, 2)));
        assert_eq!(rl.tracked_conversations(), 1);
    }
}
