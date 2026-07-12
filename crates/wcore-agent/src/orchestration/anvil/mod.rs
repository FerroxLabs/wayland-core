//! Anvil — native gated-forge engine (sibling of [`crate::orchestration::council`]).
//! A1 skeleton slice.
//!
//! Anvil forges a candidate that passes a REAL executable gate, then stamps a
//! `verified` receipt. It rides the DRIVER rail: [`drive_climb`] is the entry
//! point, mirroring `council::drive_council` — NOT the test-only `GraphConfig`
//! scaffolding (spec §3, v2 correction).
//!
//! This slice establishes the module seam and the shared terminal-state
//! vocabulary. The climb loop (probe → ensemble → surgical → escalate), gate
//! closure pinning, ledger, and the `AnvilReceipt` protocol event land in the
//! subsequent A1 PRs. The whole engine is kill-switched via
//! [`wcore_config::anvil::AnvilConfig::enabled`] (default `false`).
//!
//! Spec: `docs/design/2026-07-12-anvil-native-gated-forge-design.md` (v2).

/// Gate closure pinning, the pre-climb probe, injection fencing, flake policy.
pub mod gates;
/// Per-task cost ledger with atomic reservation-before-dispatch.
pub mod ledger;

use wcore_config::anvil::AnvilConfig;

/// Terminal state of a climb — the COMPLETE enum (spec §6.5). Every climb ends
/// in exactly one of these, and each maps to a published receipt. It lives here
/// because it is the shared vocabulary the whole A1 stack is built around; the
/// climb logic that actually produces each variant lands in the climb slice
/// (A1.5). There is no silent fourth exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalState {
    /// All checks green with the required stability on a real Tier-1 gate — the
    /// ONLY state that earns the reserved `verified` stamp (spec §2 vocabulary).
    Verified,
    /// User-confirmed derived criteria passed (Tier-2). Stamped
    /// `criteria-checked`, never `verified`.
    CriteriaChecked,
    /// Self-generated checks only (Tier-3) — correlated evidence, not truth.
    /// Stamped `self-checked`, visually quarantined.
    SelfChecked,
    /// Some checks remain uncracked; the user is offered escalate / show
    /// attempts / accept-partial.
    NeedsEscalation,
    /// Could not proceed, for a stated reason (e.g. the gate cannot execute).
    Blocked(String),
    /// The user or host cancelled; partial work is reported honestly.
    Cancelled,
    /// A time budget was exhausted mid-climb.
    TimedOut,
    /// Exec was refused (posture / permissions) before or during the climb.
    PermissionDenied,
    /// A crash was caught and the climb recovered from its journal.
    CrashedRecovered,
    /// A newer climb for the same task superseded this one.
    Superseded,
}

impl TerminalState {
    /// Whether this state earns the reserved `verified` stamp. ONLY a real
    /// Tier-1 gate passing with stability does — the honesty vocabulary of
    /// spec §2 hangs off this being a single, tight predicate.
    #[must_use]
    pub fn is_verified(&self) -> bool {
        matches!(self, TerminalState::Verified)
    }
}

/// Outcome of a climb. A1 skeleton carries only the terminal state; the receipt
/// payload, settled spend, and gate/artifact digests are added as their slices
/// land.
#[derive(Debug, Clone)]
pub struct ClimbResult {
    /// How the climb ended.
    pub terminal: TerminalState,
}

/// Errors that abort a climb before it can produce a terminal state.
#[derive(Debug, thiserror::Error)]
pub enum AnvilError {
    /// Anvil is disabled by its kill switch (`[anvil] enabled = false`).
    #[error("Anvil is disabled (set `enabled = true` under `[anvil]`)")]
    Disabled,
}

/// Drive a gated-forge climb for `task` (spec §6). Mirrors the
/// `council::drive_council` driver-rail entry.
///
/// A1 skeleton: enforces the kill-switch, then returns an honest `Blocked`
/// terminal — the probe → ensemble → surgical → escalate loop, gate machinery,
/// and receipt emission land in the later A1 slices. The kill-switch is checked
/// here as an invariant even though the CLI entry also checks it, because
/// `drive_climb` will gain a second (in-turn `/forge`) caller.
pub async fn drive_climb(task: &str, cfg: &AnvilConfig) -> Result<ClimbResult, AnvilError> {
    if !cfg.enabled {
        return Err(AnvilError::Disabled);
    }
    let _ = task;
    Ok(ClimbResult {
        terminal: TerminalState::Blocked(
            "Anvil A1 climb engine is not yet implemented (skeleton slice)".into(),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drive_climb_refuses_when_disabled() {
        let cfg = AnvilConfig { enabled: false };
        let err = drive_climb("task", &cfg).await.unwrap_err();
        assert!(matches!(err, AnvilError::Disabled));
    }

    #[tokio::test]
    async fn drive_climb_enabled_returns_blocked_skeleton() {
        let cfg = AnvilConfig { enabled: true };
        let res = drive_climb("task", &cfg).await.unwrap();
        assert!(matches!(res.terminal, TerminalState::Blocked(_)));
        // The skeleton must never claim verification.
        assert!(!res.terminal.is_verified());
    }

    #[test]
    fn only_verified_state_reports_verified() {
        assert!(TerminalState::Verified.is_verified());
        for s in [
            TerminalState::CriteriaChecked,
            TerminalState::SelfChecked,
            TerminalState::NeedsEscalation,
            TerminalState::Blocked("x".into()),
            TerminalState::Cancelled,
            TerminalState::TimedOut,
            TerminalState::PermissionDenied,
            TerminalState::CrashedRecovered,
            TerminalState::Superseded,
        ] {
            assert!(!s.is_verified(), "{s:?} must not report verified");
        }
    }
}
