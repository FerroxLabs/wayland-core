//! v0.9.3 W7 — one-shot onboarding hints (currently: first-spawn expanded-strip hint).

use std::time::Instant;

#[derive(Default)]
pub struct OnboardingState {
    /// Set by protocol_bridge on the first-ever SubAgentEvent::Spawned.
    /// Drives the 5s expanded `⌥A — open agent list · ⏎ open · ⎋ back` hint.
    pub first_spawn_seen: Option<Instant>,
}
