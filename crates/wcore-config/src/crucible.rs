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

/// How the council roster is chosen.
///
/// `Manual` (the default) is the shipped behavior: the roster comes verbatim
/// from `[crucible].proposers` / `aggregator`. `Auto` opts into the deterministic
/// `Assembler`, which selects a cost-effective, provider-diverse membership per
/// task from the live keyed pool. `Manual` must stay byte-identical to the
/// pre-assembler path, so this defaults to `Manual` and every auto-only code
/// path is gated behind `Auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AssemblyMode {
    /// Roster comes verbatim from config — the shipped path.
    #[default]
    Manual,
    /// Roster is chosen by the deterministic Assembler.
    Auto,
}

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
    /// Extra `"provider:model"` specs the AUTO Assembler may draw from, beyond
    /// `proposers`. Only consulted when `assembly = "auto"`; the Assembler picks a
    /// cost-effective, provider-diverse subset from `proposers` ∪ `candidate_pool`.
    /// Ignored on the manual path.
    pub candidate_pool: Vec<String>,
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
    /// Optional hard spend ceiling for the whole council, in USD. When set, the
    /// council refuses to run if its worst-case pre-flight estimate exceeds this
    /// (a council is N× the spend of one call, so a cap is the headline cost
    /// control). `None` ⇒ no cap.
    pub max_cost_usd: Option<f64>,
    /// Roster selection mode. `Manual` (default) keeps the shipped path; `Auto`
    /// enables the deterministic Assembler. Every assembler-only behavior is
    /// gated on this being `Auto`.
    pub assembly: AssemblyMode,
    /// Multiplier applied to the native-SKU price when pricing a
    /// `flux-pinned-*` model (Flux's flat-rate / markup is not in the catalog).
    /// `1.0` (default) prices flux-pinned models at their underlying native rate
    /// — a stopgap until Flux emits an authoritative cost (FerroxLabs/wayland#319).
    pub flux_markup: f64,
    /// Global wall-clock soft-deadline for the whole council, in seconds,
    /// measured from council start. Once `min_proposers` usable proposals are in,
    /// the run returns as soon as this deadline has passed, cancelling
    /// still-running stragglers. It binds only after quorum; `proposer_deadline_s`
    /// is the per-proposer hard backstop and is kept larger so the soft-deadline
    /// is the binding latency bound once quorum is met.
    pub global_deadline_s: u64,
    /// Auto-path spend cap (USD) for a Low-stakes council.
    pub cap_low_usd: f64,
    /// Auto-path spend cap (USD) for a Med-stakes council.
    pub cap_med_usd: f64,
    /// Auto-path spend cap (USD) for a High-stakes council.
    pub cap_high_usd: f64,
    /// Opt-in (default `false`): append a privacy-safe preference line per auto
    /// council to `crucible-assembly.jsonl` under the user config dir — stakes
    /// class + provider-family mix + est-vs-actual cost ONLY, never task text or
    /// model specs. The learning signal for a future BetaScorer; off until the
    /// operator opts in.
    pub log_assembly: bool,
}

impl Default for CrucibleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            proposers: Vec::new(),
            candidate_pool: Vec::new(),
            aggregator: None,
            min_proposers: 1,
            max_proposers: 5,
            proposer_max_turns: 4,
            proposer_deadline_s: 90,
            max_cost_usd: None,
            assembly: AssemblyMode::Manual,
            flux_markup: 1.0,
            global_deadline_s: 25,
            cap_low_usd: 0.02,
            cap_med_usd: 0.05,
            cap_high_usd: 0.15,
            log_assembly: false,
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

    #[test]
    fn assembly_defaults_to_manual() {
        let c = CrucibleConfig::default();
        assert_eq!(c.assembly, AssemblyMode::Manual);
        assert_eq!(c.flux_markup, 1.0);
        assert_eq!(c.global_deadline_s, 25);
        assert_eq!(
            (c.cap_low_usd, c.cap_med_usd, c.cap_high_usd),
            (0.02, 0.05, 0.15)
        );
    }

    #[test]
    fn assembly_absent_parses_as_manual_and_auto_parses() {
        // A table that never mentions `assembly` must default to Manual.
        let toml = r#"
enabled = true
proposers = ["openai", "anthropic"]
"#;
        let c: CrucibleConfig = toml::from_str(toml).expect("parse without assembly");
        assert_eq!(c.assembly, AssemblyMode::Manual);

        // The lowercase rename means `assembly = "auto"` parses.
        let c2: CrucibleConfig =
            toml::from_str("assembly = \"auto\"").expect("parse assembly=auto");
        assert_eq!(c2.assembly, AssemblyMode::Auto);
    }
}
