//! Anvil (native gated-forge engine) configuration — the on-disk `[anvil]`
//! block. OFF by default (`enabled = false`), mirroring `[crucible]`.
//!
//! Anvil is the checkable-reward sibling of Crucible: when a task has a REAL
//! executable gate, Anvil forges a candidate that passes it and stamps a
//! `verified` receipt. A1 is the engine slice — kill-switched here until the
//! climb loop, gate machinery, ledger, receipt event, and adversarial tests
//! all land. See `docs/design/2026-07-12-anvil-native-gated-forge-design.md`.

use serde::{Deserialize, Serialize};

/// Top-level `[anvil]` configuration.
///
/// `Default` is derived: every field's default is its type default, and the
/// kill-switch `enabled` therefore defaults to `false` — Anvil is off unless a
/// `[anvil]` block explicitly turns it on.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AnvilConfig {
    /// Kill-switch. `false` (the default) keeps Anvil inert: the `forge`
    /// subcommand (and, later, the `/forge` verb and auto-detector) refuse.
    /// Every A1 PR lands behind this flag until the slice is complete.
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off() {
        assert!(!AnvilConfig::default().enabled);
    }

    #[test]
    fn absent_table_deserializes_to_disabled() {
        // An absent/empty `[anvil]` table must yield the disabled default —
        // the kill-switch fails safe when the block is omitted.
        let cfg: AnvilConfig = toml::from_str("").unwrap();
        assert!(!cfg.enabled);
    }

    #[test]
    fn enabled_round_trips() {
        let cfg: AnvilConfig = toml::from_str("enabled = true\n").unwrap();
        assert!(cfg.enabled);
    }
}
