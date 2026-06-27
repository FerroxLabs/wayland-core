//! Council spend accounting — per-provider + total token/cost rollup for a
//! council run, plus the pre-flight budget estimate.
//!
//! A council costs N× a single call, so cost transparency and a budget ceiling
//! are first-class. Pricing comes from the shared `wcore-pricing` catalog
//! (provider×model → $/Mtok) — NEVER hardcoded. A catalog miss contributes 0
//! cost (the council never fails over a missing price row) and is flagged via
//! `priced = false` so the operator can tell "free" from "unpriced".

use wcore_pricing::{DEFAULT_CATALOG, PricingCatalog};
use wcore_types::crucible::MICROCENTS_PER_USD;
use wcore_types::message::TokenUsage;

use super::aggregator::{AGGREGATOR_MAX_TOKENS, AGGREGATOR_MAX_TURNS};
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

/// A conservative judge-inclusive pre-flight cost estimate for an auto council.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreflightEstimate {
    /// Worst-case microcents: every proposer at its turn×token output ceiling
    /// plus the aggregator priced over ALL proposer outputs (the judge is the
    /// dominant, N-scaled cost). Unpriceable members contribute 0 here — read
    /// `fully_priced` before trusting this against a cap.
    pub microcents: u64,
    /// False if any proposer or the aggregator was unpriceable. A not-fully-
    /// priced estimate UNDERcounts and must never be certified under a budget
    /// cap — the auto Assembler excludes unpriceable members up front so a real
    /// auto roster is always fully priced.
    pub fully_priced: bool,
}

impl PreflightEstimate {
    /// The estimate in USD.
    pub fn usd(&self) -> f64 {
        self.microcents as f64 / MICROCENTS_PER_USD
    }

    /// The microcents estimate ONLY when every member was priced. `None` when any
    /// member was unpriceable — the estimate then undercounts, so a caller must
    /// NOT certify it under a cap. Use this (not the raw `microcents`) to enforce
    /// a budget ceiling so an undercount can never silently pass.
    pub fn certified_microcents(&self) -> Option<u64> {
        self.fully_priced.then_some(self.microcents)
    }
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

    /// Actual USD for one member's real `TokenUsage`, via the flux-aware resolved
    /// price × `markup`. Unpriceable ⇒ 0.0 (charging never fails over a missing
    /// price row; the pre-flight certification already gated the cap).
    pub fn usd_for_usage(
        provider: &str,
        model: Option<&str>,
        usage: &TokenUsage,
        markup: f64,
    ) -> f64 {
        model
            .and_then(|m| {
                DEFAULT_CATALOG.estimate_cost_microcents_resolved(
                    provider,
                    m,
                    usage.input_tokens,
                    usage.output_tokens,
                    markup,
                )
            })
            .map(|mc| mc as f64 / MICROCENTS_PER_USD)
            .unwrap_or(0.0)
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
        let out_worst = (max_turns.max(1) as u64).saturating_mul(max_tokens as u64);
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

    /// Judge-inclusive conservative pre-flight estimate for an AUTO council.
    ///
    /// Unlike the manual `estimate_worst_case_microcents` (proposers only, no
    /// judge, non-flux pricing), this counts the aggregator — the dominant,
    /// N-scaled cost — and prices every member through the flux-aware resolved
    /// path × `markup`, so flux-pinned members are counted instead of silently $0.
    ///
    /// Each proposer is bounded at `max_turns × max_tokens` output + `max_tokens`
    /// input. The aggregator reads EVERY proposal plus the task, so its input is
    /// `(proposer_count × proposer_output_ceiling) + max_tokens` and its output is
    /// `max_tokens`. An unpriceable member sets `fully_priced = false` (it never
    /// counts as $0), so the caller never certifies an undercount under a cap.
    pub fn estimate_preflight_microcents(
        catalog: &PricingCatalog,
        proposers: &[(&str, Option<&str>)],
        aggregator: Option<(&str, Option<&str>)>,
        max_turns: usize,
        max_tokens: u32,
        markup: f64,
    ) -> PreflightEstimate {
        let out_worst = (max_turns.max(1) as u64).saturating_mul(max_tokens as u64);
        // A multi-turn agentic proposer re-sends its growing context each turn, so
        // worst-case INPUT also scales with turns — not one prompt. Bound it at
        // max_turns × max_tokens so a tool-looping proposer cannot exceed the cap.
        let in_worst = (max_turns.max(1) as u64).saturating_mul(max_tokens as u64);
        let mut microcents = 0u64;
        let mut fully_priced = true;

        for (provider, model) in proposers {
            match model.and_then(|m| {
                catalog.estimate_cost_microcents_resolved(provider, m, in_worst, out_worst, markup)
            }) {
                Some(c) => microcents = microcents.saturating_add(c),
                None => fully_priced = false,
            }
        }

        if let Some((provider, model)) = aggregator {
            // The aggregator's input is every proposal (≈ Σ proposer outputs)
            // plus the task prompt. Its OUTPUT ceiling is the aggregator's OWN
            // budget (`AGGREGATOR_MAX_TURNS × AGGREGATOR_MAX_TOKENS`), NOT one
            // proposer `max_tokens` — the judge is the dominant cost line, so
            // undercounting it lets a council exceed its cap. These constants are
            // the executor's source of truth (aggregator.rs), shared here so the
            // cap bounds true worst-case spend.
            let judge_in = (proposers.len() as u64)
                .saturating_mul(out_worst)
                .saturating_add(in_worst);
            let judge_out =
                (AGGREGATOR_MAX_TURNS as u64).saturating_mul(AGGREGATOR_MAX_TOKENS as u64);
            match model.and_then(|m| {
                catalog.estimate_cost_microcents_resolved(provider, m, judge_in, judge_out, markup)
            }) {
                Some(c) => microcents = microcents.saturating_add(c),
                None => fully_priced = false,
            }
        }

        PreflightEstimate {
            microcents,
            fully_priced,
        }
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
    fn preflight_includes_judge_and_scales_with_proposer_count() {
        let cat = PricingCatalog::load_default().unwrap();
        let agg = Some(("anthropic", Some("claude-opus-4-7")));

        let p3: Vec<(&str, Option<&str>)> = vec![("deepseek", Some("deepseek-v4-pro")); 3];
        let pre3 = CouncilSpend::estimate_preflight_microcents(&cat, &p3, agg, 4, 4096, 1.0);
        assert!(pre3.fully_priced);

        // Proposer-only worst case (no judge) — the judge must add cost on top.
        let prop_only = CouncilSpend::estimate_worst_case_microcents(&p3, 4, 4096);
        assert!(
            pre3.microcents > prop_only,
            "judge cost must be included (pre {} vs proposer-only {})",
            pre3.microcents,
            prop_only
        );

        // 5 proposers: the judge reads more, so going 3→5 grows by MORE than two
        // proposers' own cost — proving the judge input scales with N.
        let p5: Vec<(&str, Option<&str>)> = vec![("deepseek", Some("deepseek-v4-pro")); 5];
        let pre5 = CouncilSpend::estimate_preflight_microcents(&cat, &p5, agg, 4, 4096, 1.0);
        let per_proposer = prop_only / 3;
        assert!(
            pre5.microcents - pre3.microcents > 2 * per_proposer,
            "judge input must scale with proposer count beyond proposer cost alone"
        );
    }

    #[test]
    fn preflight_input_scales_with_turns() {
        // A multi-turn proposer re-sends growing context each turn, so its INPUT
        // ceiling scales with turns (max_turns × max_tokens), not one prompt.
        // Hand-derive (like the judge-ceiling test below): turns=2 → input AND
        // output are each 2×1000.
        let cat = PricingCatalog::load_default().unwrap();
        let proposers: Vec<(&str, Option<&str>)> = vec![("deepseek", Some("deepseek-v4-pro"))];
        let two = CouncilSpend::estimate_preflight_microcents(&cat, &proposers, None, 2, 1000, 1.0)
            .microcents;
        // With the fix, input scales with turns → priced at (2000 in, 2000 out).
        let scaled_input = cat
            .estimate_cost_microcents("deepseek", "deepseek-v4-pro", 2000, 2000)
            .unwrap();
        // The pre-fix bug pinned input at one max_tokens → (1000 in, 2000 out).
        let fixed_input = cat
            .estimate_cost_microcents("deepseek", "deepseek-v4-pro", 1000, 2000)
            .unwrap();
        assert_eq!(
            two, scaled_input,
            "turns=2 must price input at max_turns×max_tokens, not one prompt"
        );
        assert!(
            two > fixed_input,
            "input did not scale with turns: {two} vs fixed-input {fixed_input}"
        );
    }

    #[test]
    fn preflight_prices_judge_output_at_aggregator_ceiling() {
        // The judge output must be priced at the aggregator's OWN ceiling
        // (AGGREGATOR_MAX_TURNS × AGGREGATOR_MAX_TOKENS), not one proposer
        // max_tokens — else the dominant cost line is undercounted and a council
        // can exceed its cap. Hand-derive and lock it.
        let cat = PricingCatalog::load_default().unwrap();
        let proposers: Vec<(&str, Option<&str>)> = vec![("deepseek", Some("deepseek-v4-pro"))];
        let agg = Some(("anthropic", Some("claude-opus-4-7")));
        let pre = CouncilSpend::estimate_preflight_microcents(&cat, &proposers, agg, 1, 1000, 1.0);

        let judge_in = 1000 + 1000; // proposers.len()(1) × out_worst(1000) + in_worst(1000)
        let judge_out = (AGGREGATOR_MAX_TURNS as u64) * (AGGREGATOR_MAX_TOKENS as u64); // 8192
        let judge = cat
            .estimate_cost_microcents("anthropic", "claude-opus-4-7", judge_in, judge_out)
            .unwrap();
        let proposer = cat
            .estimate_cost_microcents("deepseek", "deepseek-v4-pro", 1000, 1000)
            .unwrap();
        assert_eq!(
            pre.microcents,
            proposer + judge,
            "judge output priced at the aggregator ceiling, not one max_tokens"
        );
    }

    #[test]
    fn preflight_flags_unpriceable_member_never_zero() {
        let cat = PricingCatalog::load_default().unwrap();
        // An unknown model → not fully priced (must not pass a cap silently).
        let members: Vec<(&str, Option<&str>)> = vec![("openai", Some("made-up-model"))];
        let pre = CouncilSpend::estimate_preflight_microcents(&cat, &members, None, 4, 4096, 1.0);
        assert!(
            !pre.fully_priced,
            "an unpriceable member must flag the estimate"
        );
        // A flux-pinned member WITH a native row is counted (judge-inclusive
        // pricing sees flux-pinned, unlike the manual worst-case estimate).
        let flux: Vec<(&str, Option<&str>)> = vec![("flux-router", Some("flux-pinned-gpt-5")); 2];
        let pre_flux = CouncilSpend::estimate_preflight_microcents(
            &cat,
            &flux,
            Some(("flux-router", Some("flux-pinned-gpt-5"))),
            4,
            4096,
            1.0,
        );
        assert!(pre_flux.fully_priced && pre_flux.microcents > 0);
    }

    #[test]
    fn certified_microcents_only_when_fully_priced() {
        let cat = PricingCatalog::load_default().unwrap();
        // Fully priced → certified value present and equal to microcents.
        let priced: Vec<(&str, Option<&str>)> = vec![("deepseek", Some("deepseek-v4-pro")); 2];
        let pre = CouncilSpend::estimate_preflight_microcents(&cat, &priced, None, 4, 4096, 1.0);
        assert_eq!(pre.certified_microcents(), Some(pre.microcents));
        // Any unpriceable member → no certified value (cannot certify a cap).
        let mixed: Vec<(&str, Option<&str>)> = vec![
            ("deepseek", Some("deepseek-v4-pro")),
            ("openai", Some("made-up-model")),
        ];
        let pre2 = CouncilSpend::estimate_preflight_microcents(&cat, &mixed, None, 4, 4096, 1.0);
        assert!(pre2.certified_microcents().is_none());
    }

    #[test]
    fn manual_worst_case_is_unchanged_and_judge_exclusive() {
        // Regression lock: the MANUAL pre-flight estimate prices proposers only
        // (no judge) via the non-flux path, byte-identical to before Stage 3.
        let cat = PricingCatalog::load_default().unwrap();
        let members: Vec<(&str, Option<&str>)> = vec![("deepseek", Some("deepseek-v4-pro")); 3];
        let manual = CouncilSpend::estimate_worst_case_microcents(&members, 4, 4096);
        // Equals the sum of three priced proposers, no judge term.
        let one = cat
            .estimate_cost_microcents("deepseek", "deepseek-v4-pro", 4096, 4 * 4096)
            .unwrap();
        assert_eq!(manual, one * 3);
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
