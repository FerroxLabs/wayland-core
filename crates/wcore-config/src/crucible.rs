//! `[crucible]` config block — the Mixture-of-Providers council roster + bounds.
//!
//! Opt-in and OFF by default (`enabled = false`): an absent `[crucible]` table,
//! or one without `enabled = true`, leaves the council inert — no cross-provider
//! fan-out happens. The roster + numeric bounds are validated into a runnable
//! `Roster` by `wcore_agent::orchestration::council::roster::validate_and_build`
//! (which lives in `wcore-agent` so it can reach the provider resolver).
//!
//! `max_proposers` is a cost / blast-radius cap enforced at validation time;
//! the council must never fan out wider than it.

use serde::{Deserialize, Serialize};

/// The `[crucible]` configuration block.
///
/// `#[serde(default)]` at the container level means any omitted field falls
/// back to the corresponding value in [`CrucibleConfig::default`] — so a
/// partial table (e.g. only `enabled` + `proposers`) still gets the sane
/// numeric bounds below.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CrucibleConfig {
    /// Kill-switch. `false` (the default) keeps the council inert regardless of
    /// the rest of the block.
    pub enabled: bool,
    /// Provider specs (`"provider"` or `"provider:model"`), one per council
    /// proposer. Each runs the task on its own provider.
    pub proposers: Vec<String>,
    /// Provider spec for the aggregator that fuses the proposals. `None` ⇒ the
    /// caller falls back to a default (e.g. the first non-error proposal).
    pub aggregator: Option<String>,
    /// Minimum non-error proposals required for a valid council result.
    pub min_proposers: usize,
    /// Upper bound on roster size — a cost / blast-radius cap. The roster
    /// builder rejects a proposer list longer than this.
    pub max_proposers: usize,
    /// Per-proposer turn budget.
    pub proposer_max_turns: usize,
    /// Per-proposer wall-clock deadline, in seconds.
    pub proposer_deadline_s: u64,
}

impl Default for CrucibleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            proposers: Vec::new(),
            aggregator: None,
            min_proposers: 1,
            max_proposers: 5,
            proposer_max_turns: 4,
            proposer_deadline_s: 90,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off_and_sane() {
        let c = CrucibleConfig::default();
        assert!(!c.enabled, "council must be OFF by default");
        assert!(c.proposers.is_empty());
        assert!(c.aggregator.is_none());
        assert_eq!(c.min_proposers, 1);
        assert_eq!(c.max_proposers, 5);
        assert_eq!(c.proposer_max_turns, 4);
        assert_eq!(c.proposer_deadline_s, 90);
    }

    #[test]
    fn partial_table_fills_omitted_fields_from_default() {
        // Only `enabled` + `proposers` set; the numeric bounds must fall back to
        // the Default values via container-level #[serde(default)].
        let toml = r#"
enabled = true
proposers = ["openai", "anthropic"]
"#;
        let c: CrucibleConfig = toml::from_str(toml).expect("parse partial table");
        assert!(c.enabled);
        assert_eq!(c.proposers.len(), 2);
        assert_eq!(c.min_proposers, 1);
        assert_eq!(c.max_proposers, 5);
        assert_eq!(c.proposer_deadline_s, 90);
    }

    #[test]
    fn empty_document_is_disabled() {
        let c: CrucibleConfig = toml::from_str("").expect("parse empty");
        assert!(!c.enabled);
        assert!(c.proposers.is_empty());
    }
}
