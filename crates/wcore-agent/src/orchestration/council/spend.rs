//! Council spend accounting — per-provider + total token/cost rollup for a
//! council run, plus the pre-flight budget estimate.
//!
//! A council costs N× a single call, so cost transparency and a budget ceiling
//! are first-class. Pricing comes from the shared `wcore-pricing` catalog
//! (provider×model → $/Mtok) — NEVER hardcoded. A catalog miss contributes 0
//! cost (the council never fails over a missing price row) and is flagged via
//! `priced = false` so the operator can tell "free" from "unpriced".

use wcore_pricing::{DEFAULT_CATALOG, PricingCatalog};
use wcore_types::message::TokenUsage;

use super::proposal::Proposal;

/// Whether a `(provider, model)` resolves to a catalog price — either a literal
/// key or, for a `flux-pinned-*` model, an exact native SKU (× `markup`). A
/// member with no model is never priceable.
///
/// This is an ELIGIBILITY predicate, not a billing path. The Assembler (Stage 6)
/// uses it to exclude unpriceable members from an *auto* roster, and the auto
/// pre-flight estimate (Stage 3, `estimate_preflight_microcents`) prices the
/// chosen members through the same resolved path — together those enforce the
/// auto cap. It does NOT change the *manual* path: a manually-listed flux-pinned
/// proposer still prices through the documented `price_one` soft-guard (unpriced
/// ⇒ 0) until Flux emits an authoritative cost (FerroxLabs/wayland#319).
pub fn is_priceable(
    catalog: &PricingCatalog,
    provider: &str,
    model: Option<&str>,
    markup: f64,
) -> bool {
    match model {
        Some(m) => catalog
            .estimate_cost_microcents_resolved(provider, m, 1, 1, markup)
            .is_some(),
        None => false,
    }
}

/// The pricing catalog returns microcents; 1 USD = 100¢ = 100_000_000 µ¢.
pub(crate) const MICROCENTS_PER_USD: f64 = 100_000_000.0;

/// One member's (or the aggregator's) token + cost spend.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSpend {
    pub provider: String,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cost in microcents (0 when the catalog has no price for provider×model).
    pub cost_microcents: u64,
    /// Whether a catalog price was found (false ⇒ cost is an un-priced 0).
    pub priced: bool,
}

/// Total + per-provider spend for a council run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CouncilSpend {
    /// One entry per proposer (errored included — a failed proposer still
    /// burned tokens) plus a final entry for the aggregator when present.
    pub per_provider: Vec<ProviderSpend>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_microcents: u64,
}

/// Price a single (provider, model, usage) via the bundled catalog.
fn price_one(provider: &str, model: Option<&str>, usage: &TokenUsage) -> ProviderSpend {
    let (cost_microcents, priced) = match model {
        Some(m) => match DEFAULT_CATALOG.estimate_cost_microcents(
            provider,
            m,
            usage.input_tokens,
            usage.output_tokens,
        ) {
            Ok(c) => (c, true),
            Err(_) => (0, false),
        },
        None => (0, false),
    };
    ProviderSpend {
        provider: provider.to_string(),
        model: model.map(str::to_string),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cost_microcents,
        priced,
    }
}

impl CouncilSpend {
    /// Total spend in USD.
    pub fn total_cost_usd(&self) -> f64 {
        self.total_cost_microcents as f64 / MICROCENTS_PER_USD
    }

    /// Roll up spend from every proposal (errored ones count — they still burned
    /// tokens) plus the optional aggregator `(provider, model, usage)`.
    pub fn from_run(
        proposals: &[Proposal],
        aggregator: Option<(&str, Option<&str>, &TokenUsage)>,
    ) -> Self {
        let mut spend = CouncilSpend::default();
        let mut push = |ps: ProviderSpend| {
            spend.total_input_tokens += ps.input_tokens;
            spend.total_output_tokens += ps.output_tokens;
            spend.total_cost_microcents += ps.cost_microcents;
            spend.per_provider.push(ps);
        };
        for p in proposals {
            push(price_one(&p.provider, p.model.as_deref(), &p.usage));
        }
        if let Some((prov, model, usage)) = aggregator {
            push(price_one(prov, model, usage));
        }
        spend
    }

    /// Worst-case pre-flight cost estimate (microcents) for a roster: each
    /// member bounded by `max_turns × max_tokens` output + one prompt's worth of
    /// input. Used to show the ceiling before spawning and to enforce a
    /// `max_cost_usd` cap. Unpriced members contribute 0 (a soft guard — the cap
    /// only binds on models the catalog knows).
    pub fn estimate_worst_case_microcents(
        members: &[(&str, Option<&str>)],
        max_turns: usize,
        max_tokens: u32,
    ) -> u64 {
        let out_worst = (max_turns.max(1) as u64) * (max_tokens as u64);
        let in_worst = max_tokens as u64;
        members
            .iter()
            .map(|(provider, model)| {
                model
                    .and_then(|m| {
                        DEFAULT_CATALOG
                            .estimate_cost_microcents(provider, m, in_worst, out_worst)
                            .ok()
                    })
                    .unwrap_or(0)
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(
        provider: &str,
        model: Option<&str>,
        input: u64,
        output: u64,
        is_error: bool,
    ) -> Proposal {
        Proposal {
            provider: provider.to_string(),
            model: model.map(str::to_string),
            text: if is_error {
                String::new()
            } else {
                "ans".to_string()
            },
            is_error,
            usage: TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
            latency_ms: 0,
        }
    }

    #[test]
    fn rolls_up_tokens_across_all_proposals_including_errors() {
        let proposals = vec![
            proposal("openai", None, 100, 50, false),
            proposal("anthropic", None, 200, 80, true), // errored — tokens still counted
        ];
        let spend = CouncilSpend::from_run(&proposals, None);
        assert_eq!(spend.total_input_tokens, 300);
        assert_eq!(spend.total_output_tokens, 130);
        assert_eq!(spend.per_provider.len(), 2);
    }

    #[test]
    fn unpriced_model_contributes_zero_and_flags_priced_false() {
        // A model the catalog doesn't know → cost 0, priced=false (never errors).
        let proposals = vec![proposal(
            "openai",
            Some("totally-made-up-model"),
            1000,
            1000,
            false,
        )];
        let spend = CouncilSpend::from_run(&proposals, None);
        assert_eq!(spend.total_cost_microcents, 0);
        assert!(!spend.per_provider[0].priced);
    }

    #[test]
    fn no_model_is_unpriced() {
        let ps = price_one("openai", None, &TokenUsage::default());
        assert!(!ps.priced);
        assert_eq!(ps.cost_microcents, 0);
    }

    #[test]
    fn aggregator_usage_is_included() {
        let proposals = vec![proposal("openai", None, 100, 50, false)];
        let agg_usage = TokenUsage {
            input_tokens: 500,
            output_tokens: 200,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        let spend = CouncilSpend::from_run(&proposals, Some(("anthropic", None, &agg_usage)));
        assert_eq!(spend.total_input_tokens, 600);
        assert_eq!(spend.total_output_tokens, 250);
        assert_eq!(spend.per_provider.len(), 2);
    }

    #[test]
    fn worst_case_estimate_is_zero_for_unpriced_members() {
        let members = [("openai", Some("made-up")), ("anthropic", None)];
        assert_eq!(
            CouncilSpend::estimate_worst_case_microcents(&members, 4, 4096),
            0
        );
    }

    #[test]
    fn is_priceable_distinguishes_known_from_unpriced() {
        let cat = PricingCatalog::load_default().unwrap();
        // Literal native key + flux-pinned with a native row are priceable.
        assert!(is_priceable(&cat, "openai", Some("gpt-5"), 1.0));
        assert!(is_priceable(
            &cat,
            "flux-router",
            Some("flux-pinned-gpt-5"),
            1.0
        ));
        // Unknown native SKU, no row, and no model are all unpriceable.
        assert!(!is_priceable(
            &cat,
            "flux-router",
            Some("flux-pinned-glm-5-2"),
            1.0
        ));
        assert!(!is_priceable(&cat, "openai", Some("made-up-model"), 1.0));
        assert!(!is_priceable(&cat, "openai", None, 1.0));
    }
}
