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
//! Three seams consult this limiter, all of them AUTONOMOUS: the `run_turn`
//! auto-reply in `wcore_agent::channel_inbound`; and — since wayland#585 —
//! both `MessageTransport` implementations the LLM-driven `send_message`
//! tool can actually deliver through:
//! `wcore_agent::channel_send_transport::ChannelManagerTransport` (the
//! engine-owned channel table) and
//! `wcore_agent::host_send_transport::HostDelegatedTransport` (the desktop,
//! `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`). Each keeps its own limiter
//! instance, so an agent cannot spend one budget to exhaust another.
//!
//! Those two are the only production transports that deliver at all
//! (`NullMessageTransport` always errors), so the tool seam IS covered
//! today. The check still does not live in `SendMessageTool` itself,
//! however, so a THIRD transport added later would start unthrottled.
//!
//! On suppression the `run_turn` seam also emits a single channel-side
//! notice ([`RATE_LIMIT_NOTICE`]) so a human on an interactive channel is not
//! left staring at silence (wayland#585 criterion 1). The notice is claimed
//! through [`AutoReplyRateLimiter::check_and_record_with_notice`], which hands
//! it out at most once per conversation per window — so the notice itself can
//! never become the runaway it is reporting.
//!
//! Human/operator-initiated sends are NOT gated: cron and direct
//! [`crate::ChannelManager::send_to`] take a different code path and never
//! reach this limiter. That is deliberate — `send_to` is shared with the
//! operator path, which is why the tool check lives one layer above it.
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
/// at WARN (operator-visible) on both seams, and on the tool seam it is also
/// returned to the model as an `is_error` tool result — a `warn!` alone reaches
/// nobody with `RUST_LOG` unset and so can never end a model-driven loop. The `run_turn` seam additionally
/// emits one [`RATE_LIMIT_NOTICE`] per conversation per window on the channel
/// itself, so a human never just sees silence.
pub const DEFAULT_MAX_AUTO_REPLIES: usize = 30;

/// Default rolling window for [`DEFAULT_MAX_AUTO_REPLIES`].
pub const DEFAULT_AUTO_REPLY_WINDOW: Duration = Duration::from_secs(600);

/// Default upper bound on the number of distinct conversations tracked at once.
/// Bounds memory under a flood of distinct conversation ids; least-recently
/// active conversations are evicted first (their history would age out anyway).
pub const DEFAULT_CONVERSATION_CAP: usize = 4096;

/// Channel-side text delivered when a conversation's autonomous replies start
/// being suppressed. A `tracing::warn!` reaches the operator, not the person in
/// the chat — on an interactive channel (Slack/Discord DM) the human would
/// otherwise watch the agent go silent with no signal at all (wayland#585).
///
/// Emission is rationed by [`AutoReplyRateLimiter::check_and_record_with_notice`]
/// to at most once per conversation per window, so this adds at most ONE extra
/// outbound message per window — it cannot itself sustain a ping-pong.
pub const RATE_LIMIT_NOTICE: &str = "Auto-replies are rate-limited on this conversation, so I am pausing \
automatic replies for a few minutes. Your messages are still being received.";

/// What the limiter decided for one autonomous send.
///
/// The `notify` flag exists because "suppressed" and "tell the human we are
/// suppressing" are different rates: suppression is per send, the notice is
/// once per window. Callers that only need the boolean keep using
/// [`AutoReplyRateLimiter::check_and_record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoReplyOutcome {
    /// The send is permitted and has been recorded against the window.
    Allowed,
    /// The send must be dropped. `notify` is `true` on the FIRST suppression in
    /// this conversation's window — and only then — meaning the caller should
    /// deliver [`RATE_LIMIT_NOTICE`] on the channel.
    Suppressed {
        /// Emit the one-per-window channel notice for this suppression.
        notify: bool,
    },
}

/// Per-conversation limiter state: the rolling send history plus when this
/// conversation last had a channel notice claimed for it.
#[derive(Debug, Clone, Default)]
struct ConversationState {
    /// Ascending timestamps of recent recorded autonomous sends.
    sends: VecDeque<Instant>,
    /// When the last [`RATE_LIMIT_NOTICE`] was claimed, if ever. Never cleared
    /// by an allowed send: rationing is strictly per window, so an alternating
    /// allow/suppress pattern at the cap still yields at most one notice.
    notified_at: Option<Instant>,
}

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
    /// conversation id -> rolling send history + notice bookkeeping.
    conversations: HashMap<String, ConversationState>,
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
    ///
    /// This form never claims a channel notice; use
    /// [`Self::check_and_record_with_notice`] on a seam that can deliver one.
    pub fn check_and_record(&mut self, conversation: &str, now: Instant) -> bool {
        matches!(
            self.evaluate(conversation, now, false),
            AutoReplyOutcome::Allowed
        )
    }

    /// As [`Self::check_and_record`], but a suppressed send also reports
    /// whether the caller should deliver [`RATE_LIMIT_NOTICE`] on the channel.
    ///
    /// `notify` is `true` at most once per conversation per `window`: the first
    /// suppression claims the notice and stamps the conversation, and every
    /// later suppression within that window reports `notify: false`. That bound
    /// is what keeps the notice from becoming a second ping-pong.
    pub fn check_and_record_with_notice(
        &mut self,
        conversation: &str,
        now: Instant,
    ) -> AutoReplyOutcome {
        self.evaluate(conversation, now, true)
    }

    /// Shared body of the two public entry points. `claim_notice` gates BOTH
    /// the returned flag and the state write, so the bool-only caller can never
    /// silently burn a notice the notice-capable caller was owed.
    fn evaluate(
        &mut self,
        conversation: &str,
        now: Instant,
        claim_notice: bool,
    ) -> AutoReplyOutcome {
        // Disabled: no window means no limiting.
        if self.window.is_zero() {
            return AutoReplyOutcome::Allowed;
        }

        let window = self.window;
        let max_sends = self.max_sends;
        let state = self
            .conversations
            .entry(conversation.to_string())
            .or_default();
        Self::prune(&mut state.sends, now, window);

        let outcome = if state.sends.len() >= max_sends {
            let notify = claim_notice
                && match state.notified_at {
                    Some(at) => now.saturating_duration_since(at) >= window,
                    None => true,
                };
            if notify {
                state.notified_at = Some(now);
            }
            AutoReplyOutcome::Suppressed { notify }
        } else {
            state.sends.push_back(now);
            AutoReplyOutcome::Allowed
        };

        // Bound the number of tracked conversations, never evicting the one we
        // just touched. Runs after recording so `conversation` is retained.
        self.enforce_cap(conversation);
        outcome
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
        // Fully drained conversations carry no live state (no in-window
        // sends AND no notice stamp to honour) — reclaim those first.
        self.conversations
            .retain(|k, s| k == keep || !s.sends.is_empty() || s.notified_at.is_some());
        while self.conversations.len() > self.cap {
            let victim = self
                .conversations
                .iter()
                .filter(|(k, _)| k.as_str() != keep)
                .min_by_key(|(_, s)| s.sends.back().copied().or(s.notified_at))
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
