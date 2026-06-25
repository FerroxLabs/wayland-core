//! Crucible roster — validate the `[crucible]` config into a runnable council.
//!
//! `validate_and_build` is a HARD gate at load time (NOT runtime): an invalid
//! roster — empty, bounds violated, malformed spec, unknown aggregator — is an
//! error the caller surfaces immediately, rather than discovering it mid-run
//! after spending tokens. `max_proposers` is the cost / blast-radius cap; the
//! roster can never exceed it.
//!
//! This lives in `wcore-agent` (not `wcore-config`) so a later wave can extend
//! it to resolve proposer/aggregator specs against the keyed provider resolver
//! (skipping keyless members). Slice-1 validation is structural: spec shape,
//! dedupe, count bounds, and a built-in check on the aggregator provider.

use wcore_config::config::provider_type_from_slug;
use wcore_config::crucible::CrucibleConfig;

/// A single validated proposer: the original spec plus its parsed parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposerSpec {
    /// The original `"provider"` / `"provider:model"` string.
    pub spec: String,
    /// Provider id (the part before the first `:`).
    pub provider: String,
    /// Pinned model (the part after the first `:`), if any.
    pub model: Option<String>,
}

/// A validated council roster, ready to lower into a ForgeFlow (T9).
#[derive(Debug, Clone, PartialEq)]
pub struct Roster {
    pub proposers: Vec<ProposerSpec>,
    pub aggregator: Option<String>,
    pub min_proposers: usize,
    pub proposer_max_turns: usize,
    /// Per-proposer wall-clock deadline (seconds) — the hard backstop that cuts a
    /// single hung proposer even before quorum is reached.
    pub proposer_deadline_s: u64,
    /// Council-wide wall-clock soft-deadline (seconds), measured from council
    /// start. Once `min_proposers` usable proposals are in, the run returns as
    /// soon as this deadline has passed, cancelling still-running stragglers.
    /// It binds only after quorum; before quorum each proposer is waited out to
    /// `proposer_deadline_s`. Keep it below `proposer_deadline_s` (the hard
    /// backstop) for the soft-deadline to have effect.
    pub global_deadline_s: u64,
    /// Optional council-wide spend ceiling in USD (pre-flight cap).
    pub max_cost_usd: Option<f64>,
}

/// Why a `[crucible]` roster failed validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CrucibleConfigError {
    #[error("crucible.proposers is empty")]
    Empty,
    #[error("min_proposers {0} exceeds proposer count {1}")]
    MinTooHigh(usize, usize),
    #[error("proposer count {0} exceeds max_proposers {1}")]
    TooMany(usize, usize),
    #[error("malformed proposer spec '{0}'")]
    Malformed(String),
    #[error("unknown aggregator provider '{0}'")]
    UnknownAggregator(String),
}

/// Parse a `"provider"` / `"provider:model"` spec into `(provider, model)`.
/// Rejects an empty provider, an empty model after a trailing `:`, and any
/// extra `:` (e.g. `"a:b:c"`) so specs stay unambiguous.
fn parse_spec(spec: &str) -> Option<(String, Option<String>)> {
    let mut parts = spec.splitn(2, ':');
    let provider = parts.next().unwrap_or("").trim();
    if provider.is_empty() {
        return None;
    }
    let model = match parts.next() {
        Some(m) => {
            let m = m.trim();
            // Empty model (trailing ':') or a model carrying another ':'
            // (`a:b:c`) is malformed.
            if m.is_empty() || m.contains(':') {
                return None;
            }
            Some(m.to_string())
        }
        None => None,
    };
    Some((provider.to_string(), model))
}

/// Validate a `[crucible]` config into a runnable [`Roster`]. Hard error at
/// load time — never defers to runtime.
pub fn validate_and_build(cfg: &CrucibleConfig) -> Result<Roster, CrucibleConfigError> {
    if cfg.proposers.is_empty() {
        return Err(CrucibleConfigError::Empty);
    }

    // Parse + dedupe proposer specs (by the original spec string, preserving
    // first-seen order). A malformed spec is a hard error.
    let mut seen = std::collections::HashSet::new();
    let mut proposers = Vec::new();
    for spec in &cfg.proposers {
        let (provider, model) =
            parse_spec(spec).ok_or_else(|| CrucibleConfigError::Malformed(spec.clone()))?;
        if seen.insert(spec.clone()) {
            proposers.push(ProposerSpec {
                spec: spec.clone(),
                provider,
                model,
            });
        }
    }

    // Count bounds apply to the DEDUPED roster. Check the cap first so an
    // over-long roster is rejected as TooMany rather than a min mismatch.
    let n = proposers.len();
    if n > cfg.max_proposers {
        return Err(CrucibleConfigError::TooMany(n, cfg.max_proposers));
    }
    if cfg.min_proposers > n {
        return Err(CrucibleConfigError::MinTooHigh(cfg.min_proposers, n));
    }

    // The aggregator (if set) must parse and name a known built-in provider.
    // NOTE (Slice-1): catalog ids and user `[providers]` aliases are not
    // recognized here — only built-ins + their aliases via
    // `provider_type_from_slug`. A later wave can validate against the resolver.
    if let Some(agg) = &cfg.aggregator {
        let (provider, _model) =
            parse_spec(agg).ok_or_else(|| CrucibleConfigError::Malformed(agg.clone()))?;
        if provider_type_from_slug(&provider).is_none() {
            return Err(CrucibleConfigError::UnknownAggregator(provider));
        }
    }

    Ok(Roster {
        proposers,
        aggregator: cfg.aggregator.clone(),
        min_proposers: cfg.min_proposers,
        proposer_max_turns: cfg.proposer_max_turns,
        proposer_deadline_s: cfg.proposer_deadline_s,
        global_deadline_s: cfg.global_deadline_s,
        max_cost_usd: cfg.max_cost_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(proposers: &[&str]) -> CrucibleConfig {
        CrucibleConfig {
            enabled: true,
            proposers: proposers.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_proposers_errors() {
        assert_eq!(
            validate_and_build(&cfg(&[])),
            Err(CrucibleConfigError::Empty)
        );
    }

    #[test]
    fn valid_roster_parses_provider_and_model() {
        let r = validate_and_build(&cfg(&["openai", "anthropic:claude-opus-4-8"])).expect("ok");
        assert_eq!(r.proposers.len(), 2);
        assert_eq!(r.proposers[0].provider, "openai");
        assert!(r.proposers[0].model.is_none());
        assert_eq!(r.proposers[1].provider, "anthropic");
        assert_eq!(r.proposers[1].model.as_deref(), Some("claude-opus-4-8"));
    }

    #[test]
    fn malformed_spec_errors() {
        assert_eq!(
            validate_and_build(&cfg(&["a:b:c"])),
            Err(CrucibleConfigError::Malformed("a:b:c".into()))
        );
        assert_eq!(
            validate_and_build(&cfg(&["openai:"])),
            Err(CrucibleConfigError::Malformed("openai:".into()))
        );
        assert_eq!(
            validate_and_build(&cfg(&[":model"])),
            Err(CrucibleConfigError::Malformed(":model".into()))
        );
    }

    #[test]
    fn too_many_proposers_errors() {
        let mut c = cfg(&["openai", "anthropic", "gemini"]);
        c.max_proposers = 2;
        assert_eq!(
            validate_and_build(&c),
            Err(CrucibleConfigError::TooMany(3, 2))
        );
    }

    #[test]
    fn min_higher_than_count_errors() {
        let mut c = cfg(&["openai"]);
        c.min_proposers = 2;
        assert_eq!(
            validate_and_build(&c),
            Err(CrucibleConfigError::MinTooHigh(2, 1))
        );
    }

    #[test]
    fn duplicate_specs_deduped() {
        let r = validate_and_build(&cfg(&["openai", "openai", "anthropic"])).expect("ok");
        assert_eq!(r.proposers.len(), 2, "duplicate specs must collapse");
    }

    #[test]
    fn unknown_aggregator_errors() {
        let mut c = cfg(&["openai"]);
        c.aggregator = Some("nope-xyz".into());
        assert_eq!(
            validate_and_build(&c),
            Err(CrucibleConfigError::UnknownAggregator("nope-xyz".into()))
        );
    }

    #[test]
    fn known_aggregator_ok() {
        let mut c = cfg(&["openai", "anthropic"]);
        c.aggregator = Some("anthropic".into());
        let r = validate_and_build(&c).expect("ok");
        assert_eq!(r.aggregator.as_deref(), Some("anthropic"));
    }

    #[test]
    fn aggregator_alias_resolves() {
        // `provider_type_from_slug` covers aliases (e.g. grok → xai).
        let mut c = cfg(&["openai"]);
        c.aggregator = Some("grok".into());
        assert!(validate_and_build(&c).is_ok());
    }
}
