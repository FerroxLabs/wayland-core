//! v0.9.3 W6 — 30s done-glow fader for sub-agent rows + strip ✓.
//!
//! Records the terminal (Done/Failed) timestamp per agent id and exposes a
//! linear alpha curve from 0.55 → 0.0 over 30s. `prune` drops expired entries
//! and signals when the last one fades out (so the render layer can
//! unsubscribe the TerminalGlow animation tick — see surfaces/mod.rs:779).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Total duration of the done-glow fade in seconds.
const GLOW_DURATION_SECS: f32 = 30.0;
/// Glow duration as a `Duration` (for time comparisons).
const GLOW_DURATION: Duration = Duration::from_secs(30);
/// Peak alpha at t = terminal moment (linearly decays to 0.0).
const GLOW_PEAK_ALPHA: f32 = 0.55;

#[derive(Default)]
pub struct GlowFader {
    pub terminals: HashMap<String, Instant>,
}

impl GlowFader {
    /// Record (or refresh) the terminal moment for an agent. Idempotent:
    /// last write wins, so re-recording the same id replaces the timestamp.
    pub fn record_terminal(&mut self, agent_id: String, now: Instant) {
        self.terminals.insert(agent_id, now);
    }

    /// Linear interpolation from `GLOW_PEAK_ALPHA` at the terminal moment
    /// down to 0.0 after `GLOW_DURATION`. Unknown id → 0.0.
    pub fn alpha_for(&self, agent_id: &str, now: Instant) -> f32 {
        let Some(started) = self.terminals.get(agent_id) else {
            return 0.0;
        };
        let elapsed = now.duration_since(*started);
        if elapsed >= GLOW_DURATION {
            0.0
        } else {
            GLOW_PEAK_ALPHA * (1.0 - elapsed.as_secs_f32() / GLOW_DURATION_SECS)
        }
    }

    /// True iff at least one recorded entry is still within the fade window.
    pub fn any_active(&self, now: Instant) -> bool {
        self.terminals
            .values()
            .any(|t| now.duration_since(*t) < GLOW_DURATION)
    }

    /// Drop expired (≥30s old) entries.
    ///
    /// Returns `true` IFF the prune dropped the last remaining entry — this
    /// is the signal for the render layer to unsubscribe the TerminalGlow
    /// animation clock (S0.11 wiring at surfaces/mod.rs:779).
    pub fn prune(&mut self, now: Instant) -> bool {
        let before = self.terminals.len();
        self.terminals
            .retain(|_, t| now.duration_since(*t) < GLOW_DURATION);
        let after = self.terminals.len();
        before > 0 && after == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_terminal_is_idempotent_overwrites_timestamp_v093() {
        let mut glow = GlowFader::default();
        let t0 = Instant::now();
        glow.record_terminal("a".into(), t0);
        // Re-record 5s later — last write wins.
        let t1 = t0 + Duration::from_secs(5);
        glow.record_terminal("a".into(), t1);
        assert_eq!(glow.terminals.len(), 1);
        assert_eq!(glow.terminals.get("a"), Some(&t1));
    }

    #[test]
    fn alpha_for_starts_at_peak_v093() {
        let mut glow = GlowFader::default();
        let t0 = Instant::now();
        glow.record_terminal("a".into(), t0);
        // Exactly at terminal moment → peak alpha.
        assert!((glow.alpha_for("a", t0) - GLOW_PEAK_ALPHA).abs() < 1e-6);
    }

    #[test]
    fn alpha_for_interpolates_linearly_to_zero_over_30s_v093() {
        let mut glow = GlowFader::default();
        let t0 = Instant::now();
        glow.record_terminal("a".into(), t0);
        // At halfway (15s) → 0.275 (half of 0.55).
        let mid = glow.alpha_for("a", t0 + Duration::from_secs(15));
        assert!((mid - (GLOW_PEAK_ALPHA * 0.5)).abs() < 1e-3, "mid={mid}");
        // At 30s exactly → 0.0 (boundary is inclusive of zero).
        assert_eq!(glow.alpha_for("a", t0 + Duration::from_secs(30)), 0.0);
        // Past 30s → still 0.0.
        assert_eq!(glow.alpha_for("a", t0 + Duration::from_secs(60)), 0.0);
    }

    #[test]
    fn alpha_for_unknown_id_is_zero_v093() {
        let glow = GlowFader::default();
        assert_eq!(glow.alpha_for("nope", Instant::now()), 0.0);
    }

    #[test]
    fn any_active_true_while_entry_within_window_v093() {
        let mut glow = GlowFader::default();
        let t0 = Instant::now();
        glow.record_terminal("a".into(), t0);
        assert!(glow.any_active(t0));
        assert!(glow.any_active(t0 + Duration::from_secs(29)));
        assert!(!glow.any_active(t0 + Duration::from_secs(30)));
        assert!(!glow.any_active(t0 + Duration::from_secs(60)));
    }

    #[test]
    fn any_active_false_when_empty_v093() {
        let glow = GlowFader::default();
        assert!(!glow.any_active(Instant::now()));
    }

    #[test]
    fn prune_drops_expired_entries_v093() {
        let mut glow = GlowFader::default();
        let t0 = Instant::now();
        glow.record_terminal("old".into(), t0);
        glow.record_terminal("new".into(), t0 + Duration::from_secs(20));
        // 31s after t0: "old" is expired, "new" still has ~19s left.
        glow.prune(t0 + Duration::from_secs(31));
        assert_eq!(glow.terminals.len(), 1);
        assert!(glow.terminals.contains_key("new"));
    }

    #[test]
    fn prune_returns_true_only_on_last_drop_v093() {
        let mut glow = GlowFader::default();
        let t0 = Instant::now();
        glow.record_terminal("a".into(), t0);
        glow.record_terminal("b".into(), t0 + Duration::from_secs(10));

        // 5s in: nothing expires.
        assert!(!glow.prune(t0 + Duration::from_secs(5)));
        // 31s in: "a" expires but "b" survives → not last-drop.
        assert!(!glow.prune(t0 + Duration::from_secs(31)));
        assert_eq!(glow.terminals.len(), 1);
        // 41s in: "b" expires too → last-drop fires.
        assert!(glow.prune(t0 + Duration::from_secs(41)));
        assert!(glow.terminals.is_empty());
        // Already empty → prune returns false (nothing to drop).
        assert!(!glow.prune(t0 + Duration::from_secs(50)));
    }

    /// W6.2 — end-to-end glow integration test (H1 closure).
    ///
    /// Walks the full record → tick → fade → prune lifecycle through the
    /// public surface only, matching PLAN line 1561-1572.
    #[test]
    fn glow_records_then_prunes_after_30s_v093() {
        let mut glow = GlowFader::default();
        let t0 = Instant::now();
        glow.record_terminal("a".into(), t0);
        assert!(glow.any_active(t0));
        assert!(!glow.prune(t0 + Duration::from_secs(29)));
        assert!(glow.any_active(t0 + Duration::from_secs(29)));
        assert!(glow.prune(t0 + Duration::from_secs(30) + Duration::from_millis(1)));
        assert!(!glow.any_active(t0 + Duration::from_secs(31)));
    }
}
