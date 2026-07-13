//! Anvil (native gated-forge engine) configuration — the on-disk `[anvil]`
//! block. ON by default: the forge is invocation-only and structurally refuses
//! without a real executable gate, so availability is safe. `enabled = false`
//! is the kill-switch for operators who want the rail inert regardless.
//!
//! Anvil is the checkable-reward sibling of Crucible: when a task has a REAL
//! executable gate, Anvil forges a candidate that passes it and stamps a
//! `verified` receipt. See
//! `docs/design/2026-07-12-anvil-native-gated-forge-design.md`.

use serde::{Deserialize, Serialize};

/// Top-level `[anvil]` configuration.
///
/// `enabled` defaults to `true`: availability, not activity. The forge only
/// ever runs when explicitly invoked AND a gate exists (configured or
/// auto-detected); with neither it refuses (fails safe).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnvilConfig {
    /// Kill-switch. `true` (the default) makes the forge available; it still
    /// only runs when invoked and only against a real gate. `false` keeps
    /// Anvil inert: the `forge` subcommand (and the `/forge` verb) refuse.
    pub enabled: bool,
    /// The Tier-1 gate command as an argv (e.g. `["cargo", "test"]`). The forge
    /// runs this against each candidate; a `0` exit means the candidate passes.
    /// EMPTY (the default) means auto-detect: the forge probes the workspace
    /// for its native suite (Cargo, npm, go, pytest, just, make). If nothing is
    /// configured AND nothing is detected, the forge refuses — a gated-forge
    /// with no gate can verify nothing (fails safe).
    pub gate: Vec<String>,
}

impl Default for AnvilConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gate: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_on() {
        // ON by default: availability, not activity — the forge is still
        // invocation-only and refuses without a gate.
        assert!(AnvilConfig::default().enabled);
    }

    #[test]
    fn absent_table_deserializes_to_enabled() {
        let cfg: AnvilConfig = toml::from_str("").unwrap();
        assert!(cfg.enabled);
        assert!(cfg.gate.is_empty());
    }

    #[test]
    fn kill_switch_round_trips() {
        // `enabled = false` must survive parse — it is the only way to make
        // the rail inert now that the default is on.
        let cfg: AnvilConfig = toml::from_str("enabled = false\n").unwrap();
        assert!(!cfg.enabled);
    }
}
